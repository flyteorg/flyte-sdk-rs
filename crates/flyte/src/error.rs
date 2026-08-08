/// Errors surfaced by the Flyte SDK.
///
/// This is the only error type exposed to user code; backend-specific errors
/// (controller, storage, proto decode) are wrapped so the internals can be
/// swapped without touching user-facing signatures.
#[derive(thiserror::Error, Debug)]
#[non_exhaustive]
pub enum Error {
    #[error("type error: {0}")]
    Type(String),
    #[error("storage error: {0}")]
    Storage(String),
    #[error("controller error: {0}")]
    Controller(String),
    #[error("{code}: {message}")]
    User { code: String, message: String },
    #[error("system error: {0}")]
    System(String),
    /// A condition finished without producing a value.
    #[error("condition {name} {outcome}: {message}")]
    Condition {
        name: String,
        outcome: ConditionOutcome,
        message: String,
    },
}

/// How a condition ended when it did not yield a value.
///
/// Separated from [`Error`]'s other variants so user code can match on the
/// interesting axis — "did my approval time out or was it rejected?" — without
/// string matching.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConditionOutcome {
    /// Nobody signalled it within its timeout.
    TimedOut,
    /// The backend reported it as failed.
    Failed,
    /// The run, or the condition, was aborted.
    Aborted,
    /// Terminal in a way this SDK does not recognise (a newer backend phase).
    Unknown,
}

impl ConditionOutcome {
    /// The error code reported in `error.pb`, matching the Python SDK's codes so
    /// a failure reads the same whichever SDK produced it.
    pub fn code(&self) -> &'static str {
        match self {
            ConditionOutcome::TimedOut => "ConditionTimedout",
            ConditionOutcome::Failed => "ConditionFailed",
            ConditionOutcome::Aborted => "ActionAborted",
            ConditionOutcome::Unknown => "ConditionFailed",
        }
    }
}

impl std::fmt::Display for ConditionOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            ConditionOutcome::TimedOut => "timed out",
            ConditionOutcome::Failed => "failed",
            ConditionOutcome::Aborted => "was aborted",
            ConditionOutcome::Unknown => "ended in an unrecognized phase",
        };
        f.write_str(s)
    }
}

impl Error {
    pub fn user(code: impl Into<String>, message: impl Into<String>) -> Self {
        Error::User {
            code: code.into(),
            message: message.into(),
        }
    }
}

impl From<prost::DecodeError> for Error {
    fn from(e: prost::DecodeError) -> Self {
        Error::Type(format!("proto decode failed: {e}"))
    }
}

impl From<object_store::Error> for Error {
    fn from(e: object_store::Error) -> Self {
        Error::Storage(e.to_string())
    }
}
