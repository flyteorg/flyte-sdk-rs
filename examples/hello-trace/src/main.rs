//! A Flyte task written in Rust.
//!
//! `#[flyte::trace]` steps run in-process, are recorded as child trace actions,
//! and are replayed instead of re-run when the task is retried.
//! `#[flyte::main]` makes this crate the task container's entrypoint.
//!
//! - dev loop:      `cargo test -p hello-trace`  (runs the task in-process)
//! - its interface: `cargo run -p hello-trace -- describe-interface`
//! - launch it:     see `task.py` / `workflow.py` next door
//!
//! In a task container the backend supplies `a0 --inputs <uri> --outputs-path
//! <uri>` plus the run env; nothing here has to know about that.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, flyte::FlyteStruct)]
struct Stats {
    mean: f64,
    count: i64,
    label: String,
}

#[flyte::trace]
async fn double(x: i64) -> Result<i64, flyte::Error> {
    Ok(x * 2)
}

#[flyte::trace]
async fn compute_stats(total: i64, label: String) -> Result<Stats, flyte::Error> {
    Ok(Stats {
        mean: total as f64 / 2.0,
        count: 2,
        label,
    })
}

#[flyte::trace]
async fn describe(stats: Stats) -> Result<String, flyte::Error> {
    Ok(format!(
        "{}: mean={} over {} values",
        stats.label, stats.mean, stats.count
    ))
}

#[flyte::main]
#[flyte::task]
async fn my_task(x: i64, label: String) -> Result<String, flyte::Error> {
    let doubled = double(x).await?;
    let stats = compute_stats(doubled, label).await?;
    describe(stats).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_in_process_without_a_backend() {
        let out = flyte::run(my_task(21, "demo".to_string())).unwrap();
        assert_eq!(out, "demo: mean=21 over 2 values");
    }

    #[test]
    fn interface_is_derived_from_the_signature() {
        let entry = my_task_entry();
        assert_eq!(
            (entry.interface)().to_json(entry.name),
            r#"{"flyte_interface_version":1,"task":"my_task","inputs":[{"name":"x","type":"integer","required":true},{"name":"label","type":"string","required":true}],"outputs":[{"name":"o0","type":"string"}]}"#
        );
    }
}
