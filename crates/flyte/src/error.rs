/// Errors surfaced by the Flyte SDK.
///
/// This is the only error type exposed to user code; backend-specific errors
/// (controller, storage, proto decode) are wrapped so the internals can be
/// swapped without touching user-facing signatures.
#[derive(thiserror::Error, Debug)]
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
