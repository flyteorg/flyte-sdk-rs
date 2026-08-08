//! Local-mode behavior: with no runtime state installed, traced fns run their
//! bodies directly (no backend, no recording).

use flyte::FlyteType as _;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, flyte::FlyteStruct)]
struct Point {
    x: i64,
    y: i64,
}

#[flyte::trace]
async fn shift(p: Point, by: i64) -> Result<Point, flyte::Error> {
    Ok(Point {
        x: p.x + by,
        y: p.y + by,
    })
}

#[flyte::trace]
async fn magnitude_squared(p: Point) -> Result<i64, flyte::Error> {
    // Nested traced call: runs inline in local mode (and under IN_TRACE remotely).
    let shifted = shift(p, 0).await?;
    Ok(shifted.x * shifted.x + shifted.y * shifted.y)
}

#[flyte::trace(version = "v1")]
async fn no_output(_flag: bool) -> Result<(), flyte::Error> {
    Ok(())
}

#[flyte::task]
async fn pipeline(x: i64) -> Result<i64, flyte::Error> {
    let p = shift(Point { x, y: x }, 1).await?;
    no_output(true).await?;
    magnitude_squared(p).await
}

#[test]
fn traces_run_inline_without_backend() {
    let out = flyte::run(pipeline(2)).unwrap();
    assert_eq!(out, 18); // (2+1)^2 + (2+1)^2
}

#[test]
fn task_entry_decodes_inputs_and_encodes_outputs() {
    use flyte::idl::Message as _;
    use flyte::FlyteType as _;

    let entry = pipeline_entry();
    assert_eq!(entry.name, "pipeline");
    let inputs = flyte::types::build_inputs(vec![("x", 3i64.to_literal().unwrap())]);
    let outputs = flyte::run((entry.run)(inputs)).unwrap();
    assert_eq!(outputs.literals.len(), 1);
    assert_eq!(outputs.literals[0].name, "o0");
    let value =
        i64::from_literal(outputs.literals[0].value.as_ref().unwrap()).unwrap();
    assert_eq!(value, 32); // (3+1)^2 + (3+1)^2
    // Sanity: envelope proto-encodes.
    assert!(!outputs.encode_to_vec().is_empty());
}

/// Concurrent traces with distinct inputs must be named identically no matter
/// what order the calls arrive in — that is what lets a fan-out replay on retry.
#[test]
fn distinct_inputs_name_traces_independently_of_call_order() {
    let names = |arrival: [&str; 3]| {
        let seq = flyte::context::Sequencer::default();
        arrival.map(|inputs_hash| {
            let n = seq.next(&format!("step:{inputs_hash}"));
            (
                inputs_hash.to_string(),
                flyte::hash::sub_action_name("a0", inputs_hash, "step", n),
            )
        })
    };

    let mut forward = names(["h1", "h2", "h3"]);
    let mut reverse = names(["h3", "h2", "h1"]);
    forward.sort();
    reverse.sort();
    assert_eq!(forward, reverse);

    // And every one of them is the first call for its own inputs.
    let seq = flyte::context::Sequencer::default();
    for (inputs_hash, name) in &forward {
        assert_eq!(
            *name,
            flyte::hash::sub_action_name("a0", inputs_hash, "step", seq.next(&format!("step:{inputs_hash}"))),
        );
    }
}

/// Repeated identical calls are distinct steps, not one memoized action.
#[test]
fn identical_inputs_share_a_counter() {
    let seq = flyte::context::Sequencer::default();
    assert_eq!(seq.next("step:h1"), 1);
    assert_eq!(seq.next("step:h1"), 2);
    // Distinct inputs restart at 1, which is why order cannot matter for them.
    assert_eq!(seq.next("step:h2"), 1);
}

#[test]
fn task_entry_exposes_declared_interface() {
    let iface = (pipeline_entry().interface)();

    let inputs: Vec<_> = iface.inputs.iter().map(|v| v.name).collect();
    assert_eq!(inputs, vec!["x"]);
    assert!(iface.inputs.iter().all(|v| v.required));
    assert_eq!(iface.input("x").unwrap().literal_type, i64::literal_type());
    assert!(iface.input("nope").is_none());

    let outputs: Vec<_> = iface.outputs.iter().map(|v| v.name).collect();
    assert_eq!(outputs, vec!["o0"]);
    assert_eq!(iface.outputs[0].literal_type, i64::literal_type());
}

#[test]
fn interface_json_is_the_pinned_contract() {
    // The launcher parses this exact string to build its NativeInterface, so a
    // change here is a change to a cross-language contract.
    let entry = pipeline_entry();
    assert_eq!(
        (entry.interface)().to_json(entry.name),
        r#"{"flyte_interface_version":1,"task":"pipeline","inputs":[{"name":"x","type":"integer","required":true}],"outputs":[{"name":"o0","type":"integer"}]}"#
    );
}

#[test]
fn missing_input_is_a_clear_error() {
    let entry = pipeline_entry();
    let err = flyte::run((entry.run)(flyte::types::build_inputs(vec![]))).unwrap_err();
    assert!(err.to_string().contains("inputs missing x"), "{err}");
}
