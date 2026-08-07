//! Adapter over the rs_controller crate (`flyte_controller_base`).
//!
//! This is one half of the swap boundary (the other is `idl.rs`): every
//! rs_controller/pyo3 touchpoint lives here, and the API speaks only SDK-owned
//! types. Replacing the pyo3-linked controller with a pure-Rust one later
//! changes this file only.

use std::sync::Arc;

use flyte_controller_base::action::Action;
use flyte_controller_base::core::CoreBaseController;

use crate::error::Error;
use crate::idl::{ActionIdentifier, ActionPhase, Message as _, RunIdentifier};

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
/// async work must run on this runtime (worker_main / run_local block on it).
pub fn runtime() -> &'static tokio::runtime::Runtime {
    pyo3_async_runtimes::tokio::get_runtime()
}
