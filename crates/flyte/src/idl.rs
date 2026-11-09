//! Facade over the generated Flyte protos.
//!
//! Every proto type the SDK touches is re-exported here, and all other modules
//! import protos only via `crate::idl`. When the pyo3-linked `flyteidl2` crate is
//! replaced by pure tonic-generated protos, only this file (and `controller.rs`)
//! changes.

pub use flyteidl2::flyteidl::common::{ActionIdentifier, ActionPhase, RunIdentifier};
pub use flyteidl2::flyteidl::core::{
    Binary, ContainerError, ErrorDocument, ExecutionError, KeyValuePair, Literal, LiteralType,
    Primitive, RetryStrategy, Scalar, SimpleType, TaskTemplate, TypedInterface, Variable,
    VariableEntry, VariableMap, container_error, execution_error, literal, literal_type, parameter,
    primitive, scalar,
};
pub use flyteidl2::flyteidl::task::{Inputs, NamedLiteral, Outputs, TaskSpec};
pub use flyteidl2::flyteidl::workflow::{ConditionAction, ConditionPromptType, ConditionWebhook};
pub use flyteidl2::google::protobuf::Duration;
pub use prost::Message;
