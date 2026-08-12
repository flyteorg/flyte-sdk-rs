//! Adapter over the rs_controller crate (`flyte_controller_base`).
//!
//! This is one half of the swap boundary (the other is `idl.rs`): every
//! rs_controller/pyo3 touchpoint lives here, and the API speaks only SDK-owned
//! types. Replacing the pyo3-linked controller with a pure-Rust one later
//! changes this file only.

use std::sync::Arc;

use flyte_controller_base::action::{Action, ActionType};
use flyte_controller_base::core::CoreBaseController;

use crate::error::{ConditionOutcome, Error};
use crate::idl::{ActionIdentifier, ActionPhase, ConditionAction, Literal, Message as _, RunIdentifier};

/// What the SDK needs to know about a previously recorded action.
pub struct RecordedAction {
    pub failed: bool,
    /// Full URI of the recorded `outputs.pb`, when outputs were recorded.
    pub outputs_uri: Option<String>,
}

/// Everything needed to record a finished trace as a child action.
pub struct TraceRecord {
    pub parent_action_name: String,
    pub action_name: String,
    pub friendly_name: String,
    pub inputs_uri: String,
    /// Empty string when the trace has no outputs.
    pub outputs_uri: String,
    pub start: f64,
    pub end: f64,
    pub run_output_base: String,
    pub typed_interface_bytes: Vec<u8>,
}

/// Everything needed to register a condition as a child action.
pub struct ConditionRecord {
    pub parent_action_name: String,
    pub action_name: String,
    pub spec: ConditionAction,
    /// Placeholder path: conditions have no inputs and nothing is written here,
    /// but the enqueue path requires a non-empty value.
    pub inputs_uri: String,
    pub run_output_base: String,
}

/// Cloning shares the connection, worker pool and informer cache; only the run
/// binding is per-clone. See [`Controller::for_run`].
#[derive(Clone)]
pub struct Controller {
    inner: Arc<CoreBaseController>,
    run_id: RunIdentifier,
}

impl Controller {
    /// Auth from `_UNION_EAGER_API_KEY`. Must be called from a non-async thread:
    /// the underlying constructor blocks on the shared runtime.
    pub fn with_auth(run_id: RunIdentifier, workers: usize) -> Result<Self, Error> {
        let inner = CoreBaseController::new_with_auth(workers)
            .map_err(|e| Error::Controller(format!("controller init (auth) failed: {e:?}")))?;
        Ok(Controller { inner, run_id })
    }

    /// Unauthenticated endpoint (devbox/local). Same non-async-thread constraint.
    pub fn without_auth(
        run_id: RunIdentifier,
        endpoint: String,
        workers: usize,
    ) -> Result<Self, Error> {
        let inner = CoreBaseController::new_without_auth(endpoint, workers)
            .map_err(|e| Error::Controller(format!("controller init failed: {e:?}")))?;
        Ok(Controller { inner, run_id })
    }

    pub fn run_id(&self) -> &RunIdentifier {
        &self.run_id
    }

    /// A view of this controller bound to a different run, sharing the same
    /// connection, worker pool and informer cache.
    ///
    /// `run_id` is only ever used to build `ActionIdentifier`s and to key the
    /// informer cache -- which `flyte_core` names `"{run_name}.{parent_action}"`
    /// -- so one `CoreBaseController` serves many runs. That is what lets a
    /// reusable container pay for the connection once and then handle whatever
    /// runs the backend assigns it. (Org/project/domain are absent from that key
    /// but fixed for the lifetime of a reusable environment: they are part of
    /// the fasttask queue id, so replicas only ever see one of them.)
    ///
    /// Note that [`Self::watch_for_errors`] belongs to the underlying
    /// controller, not to the view -- call it once, on the controller the pool
    /// built, not per run.
    pub fn for_run(&self, run_id: RunIdentifier) -> Self {
        Controller {
            inner: self.inner.clone(),
            run_id,
        }
    }

    fn action_id(&self, action_name: &str) -> ActionIdentifier {
        ActionIdentifier {
            run: Some(self.run_id.clone()),
            name: action_name.to_string(),
        }
    }

    /// Informer-cache lookup of a previously recorded child action. Creates the
    /// informer (and its watch stream) on first use for `parent_action_name`.
    pub async fn lookup_action(
        &self,
        action_name: &str,
        parent_action_name: &str,
    ) -> Result<Option<RecordedAction>, Error> {
        let found = self
            .inner
            .get_action(self.action_id(action_name), parent_action_name)
            .await
            .map_err(|e| Error::Controller(format!("get_action failed: {e:?}")))?;
        Ok(found.map(|a| RecordedAction {
            failed: a.phase == Some(ActionPhase::Failed) || a.err.is_some(),
            outputs_uri: a.realized_outputs_uri.filter(|u| !u.is_empty()),
        }))
    }

    /// Record a finished trace as a child action (enqueue + bounded wait for the
    /// server echo; `AlreadyExists` counts as success, so re-recording on retry
    /// is safe).
    pub async fn record_trace(&self, rec: TraceRecord) -> Result<(), Error> {
        let action_id_bytes = self.action_id(&rec.action_name).encode_to_vec();
        let action = Action::from_trace(
            rec.parent_action_name,
            &action_id_bytes,
            rec.friendly_name,
            None, // group: not supported in v0
            rec.inputs_uri,
            rec.outputs_uri,
            rec.start,
            rec.end,
            rec.run_output_base,
            None, // report_uri
            Some(&rec.typed_interface_bytes),
        )
        .map_err(|e| Error::Controller(format!("building trace action failed: {e}")))?;
        self.inner
            .submit_action(action)
            .await
            .map_err(|e| Error::Controller(format!("trace submit failed: {e:?}")))?;
        Ok(())
    }

    /// Register a condition as a child action. Returns as soon as it is
    /// enqueued: the condition exists and is answerable from that point, and
    /// [`Self::wait_condition`] collects the answer later.
    pub async fn start_condition(&self, rec: ConditionRecord) -> Result<(), Error> {
        let action_id_bytes = self.action_id(&rec.action_name).encode_to_vec();
        let action = Action::from_condition(
            rec.parent_action_name,
            &action_id_bytes,
            &rec.spec.encode_to_vec(),
            rec.inputs_uri,
            rec.run_output_base,
            None, // group: not supported in v0
        )
        .map_err(|e| Error::Controller(format!("building condition action failed: {e}")))?;
        self.inner
            .start_action(action)
            .await
            .map_err(|e| Error::Controller(format!("condition submit failed: {e:?}")))
    }

    /// Wait for a condition registered by [`Self::start_condition`] to be
    /// signalled, and return the value it carried.
    ///
    /// `Ok(None)` means it succeeded without a value, which the backend permits.
    /// A terminal-but-unsuccessful phase becomes [`Error::Condition`] rather than
    /// a controller error: it is a real answer, not a transport failure.
    pub async fn wait_condition(
        &self,
        condition_name: &str,
        parent_action_name: &str,
        action_name: &str,
    ) -> Result<Option<Literal>, Error> {
        let action = self
            .inner
            .wait_for_action(
                &self.run_id,
                parent_action_name,
                action_name,
                ActionType::Condition,
            )
            .await
            .map_err(|e| Error::Controller(format!("condition wait failed: {e:?}")))?;

        // `Recovered` counts as success here: the condition was adopted from a
        // prior run and its value is valid.
        if action.is_action_successful() {
            return Ok(action.condition_output);
        }

        let outcome = match action.phase {
            Some(ActionPhase::TimedOut) => ConditionOutcome::TimedOut,
            Some(ActionPhase::Aborted) => ConditionOutcome::Aborted,
            Some(ActionPhase::Failed) => ConditionOutcome::Failed,
            _ => ConditionOutcome::Unknown,
        };
        let message = action
            .err
            .as_ref()
            .map(|e| e.message.clone())
            .filter(|m| !m.is_empty())
            .unwrap_or_else(|| format!("phase {:?}", action.phase));
        Err(Error::Condition {
            name: condition_name.to_string(),
            outcome,
            message,
        })
    }

    pub async fn finalize(&self, parent_action_name: &str) {
        self.inner
            .finalize_parent_action(&self.run_id, parent_action_name)
            .await;
    }

    /// Surfaces informer/worker-pool failures. May be called at most once.
    pub async fn watch_for_errors(&self) -> Result<(), Error> {
        self.inner
            .watch_for_errors()
            .await
            .map_err(|e| Error::Controller(format!("controller background failure: {e:?}")))
    }
}

/// The shared tokio runtime the controller's background workers run on. All SDK
/// async work must run on this runtime (worker_main / run block on it).
pub fn runtime() -> &'static tokio::runtime::Runtime {
    pyo3_async_runtimes::tokio::get_runtime()
}
