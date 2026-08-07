//! Deterministic naming/hashing, byte-compatible with the Python SDK.
//!
//! Python references:
//! - `flyte/_utils/helpers.py::base36_encode`
//! - `flyte/_internal/runtime/convert.py::hash_data / generate_inputs_hash_from_proto`
//! - `flyte/models.py::ActionID.new_sub_action_from`

use base64::Engine;
use md5::{Digest as _, Md5};
use sha2::Sha256;

use crate::idl::{Inputs, Message as _};

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
pub fn sub_action_name(parent: &str, input_hash: &str, identity: &str, seq: u32) -> String {
    let components = format!("{parent}-{input_hash}-{identity}-{seq}");
    let digest: [u8; 16] = Md5::digest(components.as_bytes()).into();
    base36_encode(digest)
}
