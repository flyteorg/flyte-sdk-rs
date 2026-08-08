//! Many traced steps at once.
//!
//! Traced fns are just futures, so ordinary combinators fan them out:
//! `try_join_all` below puts one trace action per value in flight
//! simultaneously, and each is recorded (and replayed on retry) independently.
//!
//! Determinism note: a trace's action name is derived from its identity plus its
//! *inputs*, so concurrent calls with distinct inputs get stable names no matter
//! what order they finish in — that is what makes replay work here. Firing many
//! concurrent calls with *identical* inputs is the one case where names depend on
//! completion order; give each call distinct inputs (as `1..=n` does).
//!
//! - dev loop:      `cargo test -p concurrent-traces`
//! - its interface: `cargo run -p concurrent-traces -- describe-interface`

use serde::{Deserialize, Serialize};

/// Collections travel inside a struct (msgpack), which is how a step can take or
/// return more than one value.
#[derive(Debug, Clone, Serialize, Deserialize, flyte::FlyteStruct)]
struct Batch {
    values: Vec<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, flyte::FlyteStruct)]
struct Summary {
    count: i64,
    total: i64,
    max: i64,
}

#[flyte::trace]
async fn square(x: i64) -> Result<i64, flyte::Error> {
    Ok(x * x)
}

#[flyte::trace]
async fn summarize(batch: Batch) -> Result<Summary, flyte::Error> {
    Ok(Summary {
        count: batch.values.len() as i64,
        total: batch.values.iter().sum(),
        max: batch.values.iter().copied().max().unwrap_or_default(),
    })
}

#[flyte::main]
#[flyte::task]
async fn fanout(n: i64) -> Result<String, flyte::Error> {
    // n traced actions, all in flight at once.
    let squares = futures::future::try_join_all((1..=n).map(square)).await?;
    let summary = summarize(Batch { values: squares }).await?;
    Ok(format!(
        "squared {} values: total={} max={}",
        summary.count, summary.total, summary.max
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fans_out_in_process() {
        let out = flyte::run(fanout(4)).unwrap();
        // 1 + 4 + 9 + 16
        assert_eq!(out, "squared 4 values: total=30 max=16");
    }
}
