//! Bringing your own Dockerfile.
//!
//! Nothing here is about the Rust API — the point is the image. This task calls
//! `git`, which is not in the base image, so it only runs if the image was built
//! to include it. That is the situation a custom Dockerfile exists for: the
//! declarative layer DSL cannot install what you need, or you want a multi-stage
//! build that keeps the Rust toolchain out of the shipped image.
//!
//! The Dockerfile beside this file is wired up in `task.py` via `dockerfile=`.
//! Note it must be built with the LOCAL docker builder:
//!
//!     flyte run --image-builder local task.py probe_image --label demo
//!
//! - dev loop:      `cargo test -p custom-image`
//! - its interface: `cargo run -p custom-image -- describe-interface`

use std::process::Command;

/// Shells out to a tool the base image does not ship. Traced, so a retry replays
/// the recorded answer instead of shelling out again.
#[flyte::trace]
async fn git_version() -> Result<String, flyte::Error> {
    let output = Command::new("git").arg("--version").output().map_err(|e| {
        flyte::Error::user(
            "GitMissing",
            format!(
                "could not run `git`: {e}. Was the image built from this example's Dockerfile?"
            ),
        )
    })?;

    if !output.status.success() {
        return Err(flyte::Error::user(
            "GitFailed",
            format!("`git --version` exited with {}", output.status),
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[flyte::main]
#[flyte::task]
async fn probe_image(label: String) -> Result<String, flyte::Error> {
    let git = git_version().await?;
    Ok(format!("{label}: image provides {git}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Runs against whatever git is on the dev machine; in the container it is
    // the one the Dockerfile installed.
    #[test]
    fn reports_the_git_the_image_provides() {
        let out = flyte::run(probe_image("demo".to_string())).unwrap();
        assert!(out.starts_with("demo: image provides git version"), "{out}");
    }
}
