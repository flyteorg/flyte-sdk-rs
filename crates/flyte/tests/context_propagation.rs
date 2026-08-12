//! Why `flyte::spawn` exists.
//!
//! Task context lives in tokio task-locals, which do not cross `tokio::spawn`.
//! That was harmless while a container ran one action and kept its state in a
//! process-global, but a reusable container scopes state per action — so a bare
//! spawn inside a task body would silently drop it, and every traced step in
//! that future would run un-recorded.

#[test]
fn spawn_carries_the_trace_flag_where_tokio_spawn_drops_it() {
    flyte::run(async {
        flyte::context::IN_TRACE
            .scope(true, async {
                assert!(flyte::context::in_trace(), "scope sets the flag");

                let bare = tokio::spawn(async { flyte::context::in_trace() });
                assert!(
                    !bare.await.unwrap(),
                    "tokio::spawn leaves the scope behind — this is the trap"
                );

                let carried = flyte::spawn(async { flyte::context::in_trace() });
                assert!(
                    carried.await.unwrap(),
                    "flyte::spawn carries it across the boundary"
                );
            })
            .await;
    });
}

#[test]
fn spawn_outside_a_task_context_is_just_a_spawn() {
    // Local mode (`flyte::run` in a test, `--local`) installs no context at all.
    // Spawning there must work rather than panic on a missing task-local.
    let answer = flyte::run(async { flyte::spawn(async { 21 * 2 }).await.unwrap() });
    assert_eq!(answer, 42);
}

#[test]
fn concurrent_scopes_do_not_leak_into_each_other() {
    flyte::run(async {
        let inside = flyte::context::IN_TRACE.scope(true, async {
            // A sibling scope with the opposite value, running concurrently.
            let outside = flyte::context::IN_TRACE
                .scope(false, async { flyte::context::in_trace() });
            let (mine, theirs) =
                futures::join!(async { flyte::context::in_trace() }, outside);
            assert!(!theirs, "the sibling scope keeps its own value");
            mine
        });
        assert!(inside.await);
    });
}
