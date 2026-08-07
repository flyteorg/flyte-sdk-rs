//! The trace protocol: replay-or-run-then-record. Called from `#[flyte::trace]`
//! macro expansions; mirrors the Python `RemoteController.get_action_outputs` /
//! `record_trace` sequence.

use prost::Message as _;

use crate::context::RuntimeState;
use crate::controller::TraceRecord;
use crate::error::Error;
use crate::hash;
use crate::idl::{Inputs, Outputs, TypedInterface};
use crate::storage::Storage;

#[doc(hidden)]
pub enum TracePrep {
    /// A prior attempt recorded this exact call — outputs are ready to decode.
    Replay(Outputs),
    /// Run the body, then call `TraceHandle::record` on success.
    Run(TraceHandle),
}

#[doc(hidden)]
pub struct TraceHandle {
    pub action_name: String,
    pub friendly_name: String,
    pub inputs_uri: String,
    pub iface_bytes: Vec<u8>,
    pub start: f64,
}

#[doc(hidden)]
pub fn now_f64() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock before unix epoch")
        .as_secs_f64()
}

/// Compute the deterministic sub-action name, upload `inputs.pb`, and check for
/// a previously recorded result.
#[doc(hidden)]
pub async fn prepare_trace(
    state: &RuntimeState,
    identity: &str,
    friendly_name: &str,
    inputs: Inputs,
    iface: TypedInterface,
    has_outputs: bool,
) -> Result<TracePrep, Error> {
    let serialized = inputs.encode_to_vec();
    let input_hash = hash::inputs_hash(&inputs);
    let seq = state.next_seq(&format!("{identity}:{input_hash}"));
    let action_name = hash::sub_action_name(&state.action_name, &input_hash, identity, seq);

    // Upload inputs before the replay lookup (matches Python ordering).
    let sub_path = Storage::join(&state.run_base_dir, &action_name);
    let inputs_uri = Storage::join(&sub_path, "inputs.pb");
    state
        .storage
        .put(&inputs_uri, bytes::Bytes::from(serialized))
        .await?;

    let mut found = state
        .controller
        .lookup_action(&action_name, &state.action_name)
        .await?;
    // Cold-cache mitigation, retry attempts only: the informer's watch stream
    // may still be syncing actions recorded by a previous attempt. On first
    // attempts a miss is the expected case — don't tax it with sleeps.
    if found.is_none() && state.is_retry {
        for _ in 0..3 {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            found = state
                .controller
                .lookup_action(&action_name, &state.action_name)
                .await?;
            if found.is_some() {
                break;
            }
        }
    }

    if let Some(recorded) = found {
        if recorded.failed {
            tracing::info!(action = %action_name, "trace previously failed; re-running");
        } else if let Some(outputs_uri) = recorded.outputs_uri {
            if has_outputs {
                let data = state.storage.get(&outputs_uri).await?;
                let outputs = Outputs::decode(data.as_ref())?;
                tracing::info!(action = %action_name, "replaying recorded trace");
                return Ok(TracePrep::Replay(outputs));
            }
        }
    }

    Ok(TracePrep::Run(TraceHandle {
        action_name,
        friendly_name: friendly_name.to_string(),
        inputs_uri,
        iface_bytes: iface.encode_to_vec(),
        start: now_f64(),
    }))
}

impl TraceHandle {
    /// Upload outputs (if any) and record the finished trace as a child action.
    /// Record failures are logged, not surfaced — the user's step already
    /// succeeded; the cost of a lost record is one re-run on retry.
    #[doc(hidden)]
    pub async fn record(self, state: &RuntimeState, outputs: Option<Outputs>, end: f64) {
        let outputs_uri = match &outputs {
            Some(outs) => {
                let uri = Storage::join(
                    &Storage::join(&state.run_base_dir, &self.action_name),
                    "outputs.pb",
                );
                let data = bytes::Bytes::from(outs.encode_to_vec());
                if let Err(e) = state.storage.put(&uri, data).await {
                    tracing::error!(action = %self.action_name, "trace outputs upload failed: {e}");
                    return;
                }
                uri
            }
            None => String::new(),
        };
        let record = TraceRecord {
            parent_action_name: state.action_name.clone(),
            action_name: self.action_name.clone(),
            friendly_name: self.friendly_name,
            inputs_uri: self.inputs_uri,
            outputs_uri,
            start: self.start,
            end,
            run_output_base: state.run_base_dir.clone(),
            typed_interface_bytes: self.iface_bytes,
        };
        if let Err(e) = state.controller.record_trace(record).await {
            tracing::error!(action = %self.action_name, "trace record failed: {e}");
        }
    }
}
