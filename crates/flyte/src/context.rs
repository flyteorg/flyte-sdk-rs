//! Runtime state for the running task action, plus the in-trace flag used to
//! make nested traced fns run inline.
//!
//! There are two ways state gets here, because there are two entrypoints:
//!
//! - [`install`] sets it process-wide, for the one-shot container that runs
//!   exactly one action and exits (`worker_main`).
//! - [`scope`] sets it per tokio task, for a reusable container running several
//!   actions at once (`union-reuse`). Concurrent actions must not see each
//!   other's `action_name`/`run_base_dir`, so a process-global cannot serve them.
//!
//! [`current`] prefers the task-local and falls back to the global, so nothing
//! about the one-shot path changes.

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex, OnceLock};

use crate::controller::Controller;
use crate::storage::Storage;

pub struct RuntimeState {
    pub controller: Controller,
    pub storage: Storage,
    /// The real running task action (parent of all trace actions), e.g. "a0".
    pub action_name: String,
    pub run_base_dir: String,
    pub output_path: String,
    /// True when FLYTE_ATTEMPT_NUMBER > 0 (0-based): a previous attempt may
    /// have recorded traces, so replay lookups are worth waiting for.
    pub is_retry: bool,
    /// Per-call sequence numbers keyed by "{identity}:{inputs_hash}" so repeated
    /// identical calls get distinct deterministic names (first call = 1).
    sequencer: Sequencer,
}

/// Deterministic per-key call counter (Python's TaskCallSequencer).
///
/// The key combines a step's identity with its inputs hash, which is what makes
/// trace names independent of scheduling order:
///
/// - **Calls with distinct inputs never share a counter.** Each gets its own
///   sequence starting at 1, so its action name is a pure function of (parent,
///   identity, inputs) — concurrent calls are named the same however they
///   interleave, and replay on a later attempt finds them.
/// - **Calls that do share a counter are byte-identical**, so they draw their
///   sequence numbers in arrival order but are interchangeable: which recording
///   each one replays is immaterial.
///
/// The one consequence worth knowing: if the *number* of identical calls differs
/// between attempts, the surplus calls find no recording and simply re-run. That
/// is a missed replay, never a wrong result.
#[derive(Default)]
pub struct Sequencer(Mutex<HashMap<String, u32>>);

impl Sequencer {
    pub fn next(&self, key: &str) -> u32 {
        let mut map = self.0.lock().expect("sequencer lock poisoned");
        let entry = map.entry(key.to_string()).or_insert(0);
        *entry += 1;
        *entry
    }
}

static STATE: OnceLock<Arc<RuntimeState>> = OnceLock::new();

impl RuntimeState {
    pub fn new(
        controller: Controller,
        storage: Storage,
        action_name: String,
        run_base_dir: String,
        output_path: String,
        is_retry: bool,
    ) -> Self {
        RuntimeState {
            controller,
            storage,
            action_name,
            run_base_dir,
            output_path,
            is_retry,
            sequencer: Sequencer::default(),
        }
    }

    pub fn next_seq(&self, key: &str) -> u32 {
        self.sequencer.next(key)
    }
}

/// Install the process-wide state (once, at worker startup). Not set in local mode.
pub fn install(state: RuntimeState) -> Arc<RuntimeState> {
    let arc = Arc::new(state);
    STATE
        .set(arc.clone())
        .unwrap_or_else(|_| panic!("flyte runtime state installed twice"));
    arc
}

/// The current task context, if running as a remote worker.
///
/// Task-local first: in a reusable container the global is never set, and even
/// if it were, the task-local is the one that names *this* action.
pub fn current() -> Option<Arc<RuntimeState>> {
    CURRENT
        .try_with(Arc::clone)
        .ok()
        .or_else(|| STATE.get().cloned())
}

/// Run `fut` with `state` as the current context. The reusable-container
/// entrypoint wraps each assigned action in this.
pub async fn scope<F: Future>(state: Arc<RuntimeState>, fut: F) -> F::Output {
    CURRENT.scope(state, fut).await
}

tokio::task_local! {
    /// The action this tokio task belongs to. Set by [`scope`]; absent in the
    /// one-shot worker, which uses the process-global instead.
    pub static CURRENT: Arc<RuntimeState>;

    /// The group set by [`group`], if any. Folded into the names of the actions
    /// recorded under it and sent along with them, so the UI can fold them
    /// together.
    pub static GROUP: Option<Arc<str>>;

    /// True while a traced fn body is executing; nested traced fns then run
    /// inline instead of recording their own actions. Note: task-local, so the
    /// flag does not cross `tokio::spawn` boundaries.
    pub static IN_TRACE: bool;
}

/// Run `fut` with every trace and condition it starts placed in the group
/// `name` -- Python's `with flyte.group(name):`.
///
/// ```ignore
/// flyte::group("preprocess", async {
///     clean(raw).await?;
///     validate(raw).await
/// })
/// .await?;
/// ```
///
/// The group is part of each action's deterministic name, so the same call in
/// two different groups is two actions, not one replayed twice. Groups do not
/// nest: an inner `group` replaces the outer one for its duration, exactly as
/// in Python. Outside a running task it only scopes the name, which nothing
/// reads, so the body runs the same either way.
///
/// Like the rest of the context it is task-local: use [`spawn`], not
/// `tokio::spawn`, to keep it across a spawned task.
pub async fn group<F: Future>(name: impl Into<String>, fut: F) -> F::Output {
    let name: String = name.into();
    GROUP.scope(Some(Arc::from(name)), fut).await
}

/// The group the calling code is in, if any. Empty names count as none, as
/// they do in Python.
pub fn current_group() -> Option<String> {
    GROUP
        .try_with(|g| g.as_deref().map(str::to_string))
        .ok()
        .flatten()
        .filter(|g| !g.is_empty())
}

/// Fold the group into a call-sequence key, as Python's
/// `generate_task_call_sequence` does. The group is part of the action name, so
/// identical calls in different groups must not share a counter: if they did,
/// which group drew which sequence number would depend on scheduling order and
/// the names would change from one attempt to the next.
pub fn sequence_key(key: &str, group: Option<&str>) -> String {
    match group {
        Some(g) => format!("{key}:{g}"),
        None => key.to_string(),
    }
}

pub fn in_trace() -> bool {
    IN_TRACE.try_with(|v| *v).unwrap_or(false)
}

/// `tokio::spawn` that carries the flyte context across the task boundary.
///
/// [`CURRENT`], [`GROUP`] and [`IN_TRACE`] are all task-locals, so a bare `tokio::spawn`
/// inside a task body loses them: traced fns in the spawned future would see no
/// runtime state and silently run un-recorded, and a step spawned from inside a
/// traced body would start recording actions of its own instead of running
/// inline. Use this instead. Same signature as `tokio::spawn`.
///
/// Same-task combinators (`join!`, `try_join_all`, `select!`) never had this
/// problem and need nothing special.
pub fn spawn<F>(fut: F) -> tokio::task::JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    let state = CURRENT.try_with(Arc::clone).ok();
    let tracing = in_trace();
    let group = GROUP.try_with(Clone::clone).ok().flatten();
    tokio::spawn(async move {
        let fut = GROUP.scope(group, IN_TRACE.scope(tracing, fut));
        match state {
            Some(state) => CURRENT.scope(state, fut).await,
            None => fut.await,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::{Sequencer, current_group, group, sequence_key, spawn};

    #[test]
    fn sequencer_counts_per_key_from_one() {
        let seq = Sequencer::default();
        assert_eq!(seq.next("f:h1"), 1);
        assert_eq!(seq.next("f:h1"), 2);
        assert_eq!(seq.next("f:h2"), 1);
        assert_eq!(seq.next("g:h1"), 1);
        assert_eq!(seq.next("f:h1"), 3);
    }

    #[test]
    fn sequence_key_folds_the_group_like_python() {
        assert_eq!(sequence_key("f:h1", None), "f:h1");
        assert_eq!(sequence_key("f:h1", Some("g")), "f:h1:g");
    }

    #[tokio::test]
    async fn group_scopes_and_replaces_rather_than_nests() {
        assert_eq!(current_group(), None);
        group("outer", async {
            assert_eq!(current_group().as_deref(), Some("outer"));
            group("inner", async {
                assert_eq!(current_group().as_deref(), Some("inner"));
            })
            .await;
            assert_eq!(current_group().as_deref(), Some("outer"));
        })
        .await;
        assert_eq!(current_group(), None);
        group("", async { assert_eq!(current_group(), None) }).await;
    }

    #[tokio::test]
    async fn spawn_carries_the_group() {
        let seen = group("g", async {
            spawn(async { current_group() }).await.unwrap()
        })
        .await;
        assert_eq!(seen.as_deref(), Some("g"));
    }
}
