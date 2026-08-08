//! Process-wide runtime state for the single running task action, plus the
//! in-trace flag used to make nested traced fns run inline.

use std::collections::HashMap;
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
pub fn current() -> Option<Arc<RuntimeState>> {
    STATE.get().cloned()
}

tokio::task_local! {
    /// True while a traced fn body is executing; nested traced fns then run
    /// inline instead of recording their own actions. Note: task-local, so the
    /// flag does not cross `tokio::spawn` boundaries.
    pub static IN_TRACE: bool;
}

pub fn in_trace() -> bool {
    IN_TRACE.try_with(|v| *v).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::Sequencer;

    #[test]
    fn sequencer_counts_per_key_from_one() {
        let seq = Sequencer::default();
        assert_eq!(seq.next("f:h1"), 1);
        assert_eq!(seq.next("f:h1"), 2);
        assert_eq!(seq.next("f:h2"), 1);
        assert_eq!(seq.next("g:h1"), 1);
        assert_eq!(seq.next("f:h1"), 3);
    }
}
