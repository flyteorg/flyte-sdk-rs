//! Cross-language golden tests: values generated from the Python SDK at
//! ../flyte-sdk (see the `uv run python` snippets in each test) and pinned here
//! so the Rust implementation stays byte-compatible.

use flyte::hash;
use flyte::idl::Message as _;
use flyte::types;
use flyte::FlyteType as _;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, flyte::FlyteStruct)]
struct Stats {
    mean: f64,
    count: i64,
    label: String,
}

/// Python: base36_encode(hashlib.md5(b"a0-IH-TH-1").digest()) == "ape1kkafckt4ekjb0537lcq3u"
#[test]
fn sub_action_name_matches_python() {
    assert_eq!(
        hash::sub_action_name("a0", "IH", "TH", 1),
        "ape1kkafckt4ekjb0537lcq3u"
    );
}

/// Python: generate_inputs_hash_from_proto(Inputs[a=42, b="hi"])
///         == "dtUuRhyBlE9ABcxCjKH0XnafQA378BrguHhlFiUXcDs="
/// Also pins the deterministic proto encoding of the int literal (0a040a02082a).
#[test]
fn inputs_hash_matches_python() {
    let lit_a = 42i64.to_literal().unwrap();
    let lit_b = "hi".to_string().to_literal().unwrap();
    assert_eq!(hex(&lit_a.encode_to_vec()), "0a040a02082a");

    let inputs = types::build_inputs(vec![("a", lit_a), ("b", lit_b)]);
    assert_eq!(
        hash::inputs_hash(&inputs),
        "dtUuRhyBlE9ABcxCjKH0XnafQA378BrguHhlFiUXcDs="
    );
}

#[test]
fn empty_inputs_hash_is_empty_string() {
    assert_eq!(hash::inputs_hash(&types::build_inputs(vec![])), "");
}

/// Python: MessagePackEncoder(Stats).encode(Stats(mean=1.5, count=3, label="demo")).hex()
/// mashumaro and rmp_serde::to_vec_named both write string-keyed maps in field
/// order, so the bytes must match exactly — proving both directions round-trip.
#[test]
fn msgpack_struct_matches_mashumaro() {
    const GOLDEN: &str = "83a46d65616ecb3ff8000000000000a5636f756e7403a56c6162656ca464656d6f";
    let stats = Stats {
        mean: 1.5,
        count: 3,
        label: "demo".to_string(),
    };
    let lit = stats.to_literal().unwrap();
    let flyte::idl::literal::Value::Scalar(scalar) = lit.value.as_ref().unwrap() else {
        panic!("expected scalar literal");
    };
    let flyte::idl::scalar::Value::Binary(binary) = scalar.value.as_ref().unwrap() else {
        panic!("expected binary literal");
    };
    assert_eq!(binary.tag, "msgpack");
    assert_eq!(hex(&binary.value), GOLDEN);

    // Decode the Python-produced bytes back into the Rust struct.
    let decoded = Stats::from_literal(&lit).unwrap();
    assert_eq!(decoded, stats);
}

#[test]
fn primitive_literal_roundtrips() {
    assert_eq!(
        i64::from_literal(&42i64.to_literal().unwrap()).unwrap(),
        42
    );
    assert_eq!(
        f64::from_literal(&1.25f64.to_literal().unwrap()).unwrap(),
        1.25
    );
    assert_eq!(
        String::from_literal(&"x".to_string().to_literal().unwrap()).unwrap(),
        "x"
    );
    assert!(bool::from_literal(&true.to_literal().unwrap()).unwrap());
    // Lenient int → float, matching Python.
    assert_eq!(f64::from_literal(&3i64.to_literal().unwrap()).unwrap(), 3.0);
    // Type mismatch errors.
    assert!(i64::from_literal(&true.to_literal().unwrap()).is_err());
}

#[test]
fn typed_interface_is_key_sorted() {
    let iface = types::build_typed_interface(
        &[
            ("zeta", <i64 as flyte::FlyteType>::literal_type()),
            ("alpha", <String as flyte::FlyteType>::literal_type()),
        ],
        &[("o0", <bool as flyte::FlyteType>::literal_type())],
    );
    let keys: Vec<_> = iface
        .inputs
        .unwrap()
        .variables
        .iter()
        .map(|v| v.key.clone())
        .collect();
    assert_eq!(keys, vec!["alpha", "zeta"]);
}

#[test]
fn envelopes_preserve_declaration_order() {
    let inputs = types::build_inputs(vec![
        ("b", 1i64.to_literal().unwrap()),
        ("a", 2i64.to_literal().unwrap()),
    ]);
    let names: Vec<_> = inputs.literals.iter().map(|n| n.name.clone()).collect();
    assert_eq!(names, vec!["b", "a"]);
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
