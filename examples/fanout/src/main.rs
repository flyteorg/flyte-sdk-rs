//! A Rust task that calls another task, many times at once.
//!
//! `flyte::call_task` launches a task as a child action of this one and waits
//! for its result. Calls are ordinary futures, so `try_join_all` puts them all
//! in flight together. Each call is named deterministically from its inputs,
//! exactly as the Python SDK names them, so a retried parent picks up the
//! calls its earlier attempt made instead of launching them again, and a
//! cached callee completes without running.
//!
//! - dev loop:      `cargo test -p fanout`  (callees are `flyte::stub_task`s)
//! - its interface: `cargo run -p fanout -- describe-interface`
//! - launch it:     see `task.py` next door
//!
//! The callee is named, never defined, here. Its spec comes from the launcher:
//! `rust_task(calls=[square])` in `task.py` serializes it at deploy time and
//! hands it to this binary.

/// `<environment>.<task>`, as the launcher names the callee. A bare task name
/// also works when only one callee has it.
static SQUARE: flyte::TaskRef = flyte::TaskRef::new("fanout_py.square");

#[flyte::main]
#[flyte::task]
async fn sum_of_squares(n: i64) -> Result<i64, flyte::Error> {
    let calls = (1..=n).map(|x| flyte::call_task::<i64>(&SQUARE, flyte::inputs! { x: x }));
    let squares = futures::future::try_join_all(calls).await?;
    Ok(squares.iter().sum())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_in_process_against_a_stub() {
        // With no backend, a call runs the stub registered under its name.
        flyte::stub_task("fanout_py.square", |inputs| async move {
            let x: i64 = inputs.get("x")?;
            Ok(x * x)
        });
        assert_eq!(flyte::run(sum_of_squares(4)).unwrap(), 1 + 4 + 9 + 16);
    }
}
