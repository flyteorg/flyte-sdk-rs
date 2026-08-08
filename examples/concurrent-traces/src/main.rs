//! Many traced steps at once.
//!
//! Traced fns are just futures, so ordinary combinators fan them out:
//! `try_join_all` below puts one trace action per value in flight
//! simultaneously, and each is recorded (and replayed on retry) independently.
//!
//! Why concurrency is safe here: a trace's action name derives from the parent
//! action, the step's identity, its *inputs*, and a call counter kept per
//! (identity, inputs). Calls with distinct inputs therefore get their own
//! counter, and each name is a pure function of the call — independent of which
//! future happens to finish first. Concurrent calls with *identical* inputs do
//! share a counter, so they draw their names in arrival order, but such calls are
//! byte-identical and interchangeable: it makes no difference which one replays
//! which recording.
//!
//! Both cases rest on the same contract replay assumes anyway — a traced step is
//! a function of its inputs.
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
