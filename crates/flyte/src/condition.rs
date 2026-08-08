//! Wait for something outside the task: a human approving, a system calling back.
//!
//! A condition is a child action that the backend parks in phase `PAUSED`. It
//! becomes terminal only when someone signals it — from the console, from
//! `flyte signal condition`, or from any client — and the value they sent arrives
//! inline on the action update. The task's pod stays alive for the wait.
//!
//! Creating and waiting are deliberately separate. [`ConditionBuilder::create`]
//! is what makes the condition exist and answerable, so a task can raise every
//! question it needs up front, get on with other work, and collect the answers
//! later:
//!
//! ```ignore
//! let approval = flyte::condition("approve-deploy")
//!     .prompt("Ship build 1234 to production?")
//!     .timeout(Duration::from_secs(3600))
//!     .create()
//!     .await?;
//!
//! // ... other work, or hand `approval` to whoever owns the decision ...
//!
//! let approved: bool = approval.wait().await?;
//! ```
//!
//! Only `bool`, `i64`, `i32`, `f64`, `f32` and `String` can be waited on — the
//! backend validates a signal against a simple literal type — and that is
//! enforced at compile time by [`ConditionValue`].

use std::marker::PhantomData;
use std::time::Duration as StdDuration;

use crate::context;
use crate::controller::ConditionRecord;
use crate::error::Error;
use crate::hash;
use crate::idl::{ConditionAction, ConditionPromptType, ConditionWebhook, Duration};
use crate::storage::Storage;
use crate::types::FlyteType;

mod sealed {
    pub trait Sealed {}
}

/// A value a condition can be signalled with.
///
/// Sealed on purpose: the backend accepts only simple literal types, so waiting
/// for a `#[derive(FlyteStruct)]` type is a compile error rather than a run that
/// fails when someone tries to answer it.
pub trait ConditionValue: sealed::Sealed + FlyteType {}

macro_rules! impl_condition_value {
    ($($t:ty),* $(,)?) => {
        $(
            impl sealed::Sealed for $t {}
            impl ConditionValue for $t {}
        )*
    };
}

impl_condition_value!(bool, i64, i32, f64, f32, String);

/// Start describing a condition. Nothing is registered until
/// [`ConditionBuilder::create`].
pub fn condition<T: ConditionValue>(name: impl Into<String>) -> ConditionBuilder<T> {
    ConditionBuilder {
        name: name.into(),
        prompt: "Approve?".to_string(),
        prompt_type: ConditionPromptType::Text,
        description: String::new(),
        timeout: None,
        webhook: None,
        _value: PhantomData,
    }
}

/// Builder for a condition. See [`condition`].
pub struct ConditionBuilder<T: ConditionValue> {
    name: String,
    prompt: String,
    prompt_type: ConditionPromptType,
    description: String,
    timeout: Option<StdDuration>,
    webhook: Option<String>,
    _value: PhantomData<T>,
}

impl<T: ConditionValue> ConditionBuilder<T> {
    /// The question shown to whoever answers. Defaults to `"Approve?"`.
    pub fn prompt(mut self, prompt: impl Into<String>) -> Self {
        self.prompt = prompt.into();
        self
    }

    /// Render the prompt as Markdown rather than plain text.
    pub fn markdown(mut self) -> Self {
        self.prompt_type = ConditionPromptType::Markdown;
        self
    }

    /// Longer context for whoever answers.
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = description.into();
        self
    }

    /// Give up after `timeout`, enforced by the backend.
    ///
    /// Worth setting on anything that might go unanswered: an abandoned wait
    /// leaves the condition parked, and this is what reaps it. Ignored if zero.
    pub fn timeout(mut self, timeout: StdDuration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// POST to `url` when the condition is created, so an external system can be
    /// told there is something to answer. The backend substitutes
    /// `{callback_uri}` with the URI that signals this condition.
    pub fn webhook(mut self, url: impl Into<String>) -> Self {
        self.webhook = Some(url.into());
        self
    }

    /// Register the condition. From here on it exists, is visible, and can be
    /// signalled — whether or not anything is waiting yet.
    pub async fn create(self) -> Result<Condition<T>, Error> {
        let state = context::current().ok_or_else(|| {
            Error::System(
                "conditions can only be created inside a running task; there is no \
                 backend attached here"
                    .to_string(),
            )
        })?;

        // Same derivation Python uses, which passes the condition's name as both
        // the identity and the inputs hash. Deterministic across attempts, so a
        // retry finds an already-signalled condition instead of asking again.
        let seq = state.next_seq(&self.name);
        let action_name = hash::sub_action_name(&state.action_name, &self.name, &self.name, seq);
        if seq > 1 {
            tracing::warn!(
                condition = %self.name,
                "condition name reused within this task (call {seq}); each decision \
                 should have its own name, since these are separate questions"
            );
        }

        let spec = ConditionAction {
            name: self.name.clone(),
            r#type: Some(T::literal_type()),
            prompt: self.prompt,
            description: self.description,
            prompt_type: self.prompt_type as i32,
            // Only strictly positive timeouts mean anything; the backend ignores
            // the rest.
            timeout: self.timeout.filter(|t| !t.is_zero()).map(|t| Duration {
                seconds: t.as_secs() as i64,
                nanos: t.subsec_nanos() as i32,
            }),
            webhook: self.webhook.map(|url| ConditionWebhook {
                url,
                payload: None,
            }),
        };

        // Nothing is written to this path -- conditions have no inputs -- but the
        // enqueue path rejects an empty inputs URI.
        let inputs_uri = Storage::join(
            &Storage::join(&state.run_base_dir, &action_name),
            "inputs.pb",
        );

        state
            .controller
            .start_condition(ConditionRecord {
                parent_action_name: state.action_name.clone(),
                action_name: action_name.clone(),
                spec,
                inputs_uri,
                run_output_base: state.run_base_dir.clone(),
            })
            .await?;

        tracing::info!(condition = %self.name, action = %action_name, "condition awaiting signal");
        Ok(Condition {
            name: self.name,
            action_name,
            _value: PhantomData,
        })
    }
}

/// A registered condition, waiting to be signalled.
pub struct Condition<T: ConditionValue> {
    name: String,
    action_name: String,
    _value: PhantomData<T>,
}

// Hand-written rather than derived so the handle stays printable for any value
// type, without requiring `T: Debug`.
impl<T: ConditionValue> std::fmt::Debug for Condition<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Condition")
            .field("name", &self.name)
            .field("action_name", &self.action_name)
            .finish()
    }
}

impl<T: ConditionValue> Condition<T> {
    /// The name this condition was created with.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The action name, which is what `flyte signal condition` takes — the
    /// friendly name above will not work there.
    pub fn action_name(&self) -> &str {
        &self.action_name
    }

    /// Block until the condition is signalled, and return the value.
    ///
    /// Errors with [`Error::Condition`] if it timed out, was rejected, or was
    /// aborted. Safe to call after the signal has already arrived: the answer is
    /// held for you, so this returns immediately.
    pub async fn wait(&self) -> Result<T, Error> {
        let state = context::current().ok_or_else(|| {
            Error::System("conditions can only be awaited inside a running task".to_string())
        })?;

        let value = state
            .controller
            .wait_condition(&self.name, &state.action_name, &self.action_name)
            .await?;

        let literal = value.ok_or_else(|| {
            Error::Type(format!(
                "condition {} completed without a value; expected {:?}",
                self.name,
                T::literal_type()
            ))
        })?;
        T::from_literal(&literal)
    }
}
