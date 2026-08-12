//! A task that runs in a **warm container**.
//!
//! Normally every action gets its own pod, and every pod pays for scheduling,
//! image pull and process start before the task does anything. With a
//! `ReusePolicy`, the backend keeps a pool of replicas alive and streams actions
//! to them instead — so a short task stops being mostly overhead, and anything
//! the process built on the way (clients, caches, a loaded model) is still there
//! for the next action.
//!
//! The Rust side of that is two lines: depend on `union-reuse`, and write
//! `#[union_reuse::main]` instead of `#[flyte::main]`. The task itself is
//! untouched, and the binary still runs perfectly well as a one-shot container —
//! which is why `task.py` can turn reuse on and off without a rebuild.
//!
//! What the attribute buys is the ability to answer a second kind of launch: the
//! backend starts a replica with `--queue-id`/`--worker-id` instead of an action
//! to run, and the binary then holds a lease and serves actions as they arrive.
//! See `crates/union-reuse` for the machinery.
//!
//! - dev loop:      `cargo test -p reusable`  (runs the task in-process)
//! - its interface: `cargo run -p reusable -- describe-interface`
//! - launch it:     see `task.py` / `workflow.py` next door

use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

/// Actions handled by *this process* since it started.
///
/// A one-shot container would report 1 every time. A warm one counts up, which
/// is the whole difference made visible — and a reminder that process state is
/// now shared across actions, and across users' runs. Keep anything you cache
/// here immutable or idempotent.
static SERVED: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Serialize, Deserialize, flyte::FlyteStruct)]
struct Served {
    /// Where this action landed in its replica's lifetime: 1 on a cold replica,
    /// higher on a warm one.
    nth_on_this_replica: i64,
    result: i64,
}

#[flyte::trace]
async fn square(x: i64) -> Result<i64, flyte::Error> {
    Ok(x * x)
}

#[flyte::trace]
async fn note(result: i64) -> Result<Served, flyte::Error> {
    Ok(Served {
        nth_on_this_replica: SERVED.fetch_add(1, Ordering::Relaxed) as i64 + 1,
        result,
    })
}

#[union_reuse::main]
#[flyte::task]
async fn warm(x: i64) -> Result<String, flyte::Error> {
    let squared = square(x).await?;
    let served = note(squared).await?;
    Ok(format!(
        "{x}^2 = {} (action #{} on this replica)",
        served.result, served.nth_on_this_replica
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    // One test, not two: `SERVED` is process state, so anything asserting on it
    // has to own the whole process. That is the same property the task has in
    // production, which is why it is worth seeing here.
    #[test]
    fn successive_actions_share_the_process() {
        let first = flyte::run(warm(7)).unwrap();
        assert_eq!(first, "7^2 = 49 (action #1 on this replica)");

        // A fresh pod per action would say #1 again; a warm replica counts up.
        let second = flyte::run(warm(3)).unwrap();
        assert_eq!(second, "3^2 = 9 (action #2 on this replica)");
    }
}
