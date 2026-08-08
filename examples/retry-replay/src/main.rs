//! Replay on retry — the reason traces exist.
//!
//! `slow_step` is recorded as a child action the first time it runs. This task
//! then fails on purpose. When Flyte retries it, `slow_step` is **not re-run**:
//! its recorded outputs are replayed (look for "replaying recorded trace" in the
//! logs, and note the attempt finishes without the delay), and execution
//! continues from there.
//!
//! Retries come from the task declaration, not from here — see `retries=` in
//! `task.py`.
//!
//! - dev loop:      `cargo test -p retry-replay`
//! - its interface: `cargo run -p retry-replay -- describe-interface`

use std::time::Duration;

/// Stands in for real work — a long download, a big computation. Replayed rather
/// than repeated on the next attempt.
#[flyte::trace]
async fn slow_step(seed: i64) -> Result<i64, flyte::Error> {
    tokio::time::sleep(Duration::from_secs(2)).await;
    Ok(seed * 7)
}

#[flyte::trace]
async fn finish(value: i64) -> Result<String, flyte::Error> {
    Ok(format!("finished with {value}"))
}

fn attempt_number() -> u32 {
    // The backend injects this; 0-based.
    std::env::var("FLYTE_ATTEMPT_NUMBER")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

#[flyte::main]
#[flyte::task]
async fn flaky(seed: i64) -> Result<String, flyte::Error> {
    let value = slow_step(seed).await?;

    // Fail the first attempt, *after* the expensive step has been recorded.
    // A user error is recoverable, so Flyte retries per the task's policy.
    if attempt_number() == 0 {
        return Err(flyte::Error::user(
            "DeliberateFailure",
            "failing attempt 0 on purpose; the retry replays slow_step",
        ));
    }

    finish(value).await
}

#[cfg(test)]
mod tests {
    use super::*;

    // One test, both attempts: FLYTE_ATTEMPT_NUMBER is process-wide state and
    // cargo runs tests in parallel threads, so separate tests would race.
    #[test]
    fn fails_first_attempt_then_succeeds() {
        // No backend attached: traced fns just run their bodies, nothing recorded.
        std::env::remove_var("FLYTE_ATTEMPT_NUMBER");
        let err = flyte::run(flaky(6)).unwrap_err();
        assert!(err.to_string().contains("DeliberateFailure"), "{err}");

        std::env::set_var("FLYTE_ATTEMPT_NUMBER", "1");
        let out = flyte::run(flaky(6)).unwrap();
        assert_eq!(out, "finished with 42");
    }
}
