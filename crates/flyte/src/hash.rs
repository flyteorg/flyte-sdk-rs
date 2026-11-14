//! Deterministic naming/hashing, byte-compatible with the Python SDK.
//!
//! Python references:
//! - `flyte/_utils/helpers.py::base36_encode`
//! - `flyte/_internal/runtime/convert.py::hash_data / generate_inputs_hash_from_proto`
//! - `flyte/models.py::ActionID.new_sub_action_from`
//! - `flyte/_internal/runtime/convert.py::generate_interface_hash /
//!   generate_task_identity_hash / generate_filtered_inputs_hash /
//!   generate_cache_key_hash`

use base64::Engine;
use md5::{Digest as _, Md5};
use sha2::Sha256;

use crate::idl::{Inputs, Message as _, TaskTemplate, TypedInterface};

const BASE36_ALPHABET: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";

/// base64_std(sha256(data)) with padding — Python's `convert.hash_data`.
pub fn hash_data(data: &[u8]) -> String {
    let digest = Sha256::digest(data);
    base64::engine::general_purpose::STANDARD.encode(digest)
}

/// Hash of an `Inputs` envelope — Python's `generate_inputs_hash_from_proto`.
/// Empty inputs hash to the empty string.
pub fn inputs_hash(inputs: &Inputs) -> String {
    if inputs.literals.is_empty() {
        return String::new();
    }
    let mut combined: Vec<u8> = Vec::new();
    for named in &inputs.literals {
        combined.extend_from_slice(named.name.as_bytes());
        combined.push(b':');
        // Scalar literals: deterministic proto serialization (prost encodes in tag
        // order, matching Python's SerializeToString(deterministic=True) for our
        // scalar/binary literals — no proto maps are populated in v0 types).
        if let Some(value) = &named.value {
            combined.extend_from_slice(&value.encode_to_vec());
        }
        combined.push(b';');
    }
    hash_data(&combined)
}

/// [`inputs_hash`] over the inputs not named in `ignored` — Python's
/// `generate_filtered_inputs_hash`. Used for both the action name and the cache
/// key when a task declares `cache_ignore_input_vars`.
pub fn filtered_inputs_hash(inputs: &Inputs, ignored: &[String]) -> String {
    let filtered = Inputs {
        literals: inputs
            .literals
            .iter()
            .filter(|l| !ignored.contains(&l.name))
            .cloned()
            .collect(),
        ..Default::default()
    };
    inputs_hash(&filtered)
}

/// Hash of a task interface with its variables sorted by key — Python's
/// `generate_interface_hash`.
///
/// `raw` is the interface exactly as serialized by whoever built the spec, when
/// it is available. Python serializes deterministically (map keys sorted), but
/// prost encodes proto maps (`TypeStructure.dataclass_type`) in `HashMap` order,
/// so re-encoding a decoded interface is only byte-stable when no map has two
/// or more entries. Interfaces are serialized already sorted, so the raw bytes
/// are what Python would hash, and they are used whenever the variables are in
/// order. Otherwise the interface is sorted and re-encoded, as Python does.
pub fn interface_hash(iface: &TypedInterface, raw: Option<&[u8]>) -> String {
    fn sorted(vars: &Option<crate::idl::VariableMap>) -> bool {
        vars.as_ref()
            .is_none_or(|m| m.variables.is_sorted_by(|a, b| a.key <= b.key))
    }
    if let Some(raw) = raw
        && sorted(&iface.inputs)
        && sorted(&iface.outputs)
    {
        return hash_data(raw);
    }
    let mut iface = iface.clone();
    for vars in [&mut iface.inputs, &mut iface.outputs]
        .into_iter()
        .flatten()
    {
        vars.variables.sort_by(|a, b| a.key.cmp(&b.key));
    }
    hash_data(&iface.encode_to_vec())
}

/// Run-independent identity of a task — Python's `generate_task_identity_hash`:
/// fully-qualified name, interface, and `metadata.discovery_version`. Excludes
/// image, code version, resources and env, so action names survive code-only
/// redeploys and recovery still matches the previous run's actions.
pub fn task_identity_hash(template: &TaskTemplate, interface_hash: &str) -> String {
    let name = template.id.as_ref().map_or("", |id| id.name.as_str());
    let version = template
        .metadata
        .as_ref()
        .map_or("", |m| m.discovery_version.as_str());
    hash_data(format!("{name}-{interface_hash}-{version}").as_bytes())
}

/// Cache key of a task call — Python's `generate_cache_key_hash`. `inputs_hash`
/// is over all inputs; it is replaced by the filtered hash when `ignored` is
/// non-empty.
pub fn cache_key_hash(
    task_name: &str,
    inputs: &Inputs,
    inputs_hash: &str,
    interface_hash: &str,
    cache_version: &str,
    ignored: &[String],
) -> String {
    let final_inputs = if ignored.is_empty() {
        inputs_hash.to_string()
    } else {
        filtered_inputs_hash(inputs, ignored)
    };
    hash_data(format!("{final_inputs}{task_name}{interface_hash}{cache_version}").as_bytes())
}

/// Big-endian md5 digest → base36 (alphabet 0-9a-z) — Python's `base36_encode`.
pub fn base36_encode(digest: [u8; 16]) -> String {
    let mut num = u128::from_be_bytes(digest);
    if num == 0 {
        return "0".to_string();
    }
    let mut out = Vec::new();
    while num > 0 {
        out.push(BASE36_ALPHABET[(num % 36) as usize]);
        num /= 36;
    }
    out.reverse();
    String::from_utf8(out).expect("base36 alphabet is ascii")
}

/// Deterministic sub-action name — Python's `ActionID.new_sub_action_from`.
/// All components must be stable across attempts: recovery matches previously
/// recorded actions by this name.
///
/// `seq` is what keeps repeated *identical* calls distinct, so N calls to a step
/// are N steps rather than one memoized action (see [`crate::context::Sequencer`]
/// for the ordering guarantees). It is also part of Python's component string, so
/// dropping it here would diverge from the names Python generates — the golden
/// test `sub_action_name_matches_python` pins that.
///
/// `group` (from [`crate::context::group`]) is appended as `-{group}` when set,
/// again as Python does, so ungrouped names are unchanged.
pub fn sub_action_name(
    parent: &str,
    input_hash: &str,
    identity: &str,
    seq: u32,
    group: Option<&str>,
) -> String {
    let mut components = format!("{parent}-{input_hash}-{identity}-{seq}");
    if let Some(g) = group {
        components.push('-');
        components.push_str(g);
    }
    let digest: [u8; 16] = Md5::digest(components.as_bytes()).into();
    base36_encode(digest)
}
