//! Local-mode behavior: with no runtime state installed, traced fns run their
//! bodies directly (no backend, no recording).

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
    let out = flyte::run_local(pipeline(2)).unwrap();
    assert_eq!(out, 18); // (2+1)^2 + (2+1)^2
}

#[test]
fn task_entry_decodes_inputs_and_encodes_outputs() {
    use flyte::idl::Message as _;
    use flyte::FlyteType as _;

    let entry = pipeline_entry();
    assert_eq!(entry.name, "pipeline");
    let inputs = flyte::types::build_inputs(vec![("x", 3i64.to_literal().unwrap())]);
    let outputs = flyte::run_local((entry.run)(inputs)).unwrap();
    assert_eq!(outputs.literals.len(), 1);
    assert_eq!(outputs.literals[0].name, "o0");
    let value =
        i64::from_literal(outputs.literals[0].value.as_ref().unwrap()).unwrap();
    assert_eq!(value, 32); // (3+1)^2 + (3+1)^2
    // Sanity: envelope proto-encodes.
    assert!(!outputs.encode_to_vec().is_empty());
}

#[test]
fn missing_input_is_a_clear_error() {
    let entry = pipeline_entry();
    let err = flyte::run_local((entry.run)(flyte::types::build_inputs(vec![]))).unwrap_err();
    assert!(err.to_string().contains("inputs missing x"), "{err}");
}
