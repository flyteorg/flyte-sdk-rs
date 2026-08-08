//! Wait for a human before doing something irreversible.
//!
//! `flyte::condition(..).create()` registers a question and returns immediately,
//! so this task raises both reviews up front — reviewers can answer in parallel —
//! and only then blocks collecting the answers. The task's pod stays alive while
//! it waits; the conditions show up in the console as PAUSED child actions.
//!
//! Answer them with (note it takes the *action* name, which the logs print):
//!
//!     flyte get condition <run-name>
//!     flyte signal condition <run-name> <action-name> true
//!
//! - dev loop:      `cargo test -p human-approval`
//! - its interface: `cargo run -p human-approval -- describe-interface`

use std::time::Duration;

#[flyte::trace]
async fn build_artifact(version: i64) -> Result<String, flyte::Error> {
    Ok(format!("build-{version}"))
}

#[flyte::trace]
async fn deploy(artifact: String) -> Result<String, flyte::Error> {
    Ok(format!("deployed {artifact}"))
}

#[flyte::main]
#[flyte::task]
async fn gated_deploy(version: i64) -> Result<String, flyte::Error> {
    let artifact = build_artifact(version).await?;

    // Both questions exist from here on, so the two reviewers are unblocked at
    // the same time rather than one after the other.
    let security = flyte::condition::<bool>("security-review")
        .prompt(format!("Approve **{artifact}** on security grounds?"))
        .markdown()
        .description("Reviewed by the on-call security engineer")
        // Always bound a wait that might go unanswered: abandoning it does not
        // reap the condition, only the server-side timeout does.
        .timeout(Duration::from_secs(24 * 60 * 60))
        .create()
        .await?;

    let release = flyte::condition::<String>("release-ticket")
        .prompt(format!("Release ticket for {artifact}?"))
        .timeout(Duration::from_secs(24 * 60 * 60))
        .create()
        .await?;

    tracing::info!(
        security = security.action_name(),
        release = release.action_name(),
        "waiting for approvals; signal these action names"
    );

    // Now collect. Concurrently, since they are independent.
    let (approved, ticket) = futures::try_join!(security.wait(), release.wait())?;

    if !approved {
        // A rejection is a normal outcome, not a system fault.
        return Err(flyte::Error::user(
            "SecurityRejected",
            format!("{artifact} was rejected by security review"),
        ));
    }

    let result = deploy(artifact).await?;
    Ok(format!("{result} (ticket {ticket})"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn traced_steps_run_without_a_backend() {
        // Conditions need a backend, so the dev loop covers the traced steps.
        let artifact = flyte::run(build_artifact(7)).unwrap();
        assert_eq!(artifact, "build-7");
        assert_eq!(flyte::run(deploy(artifact)).unwrap(), "deployed build-7");
    }

    #[test]
    fn creating_a_condition_without_a_backend_fails_clearly() {
        // Reported at create(), where the problem is -- not later at wait().
        let err = flyte::run(async { flyte::condition::<bool>("x").create().await }).unwrap_err();
        assert!(
            err.to_string().contains("inside a running task"),
            "unexpected error: {err}"
        );
    }
}
