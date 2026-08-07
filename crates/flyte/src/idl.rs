//! Facade over the generated Flyte protos.
//!
//! Every proto type the SDK touches is re-exported here, and all other modules
//! import protos only via `crate::idl`. When the pyo3-linked `flyteidl2` crate is
//! replaced by pure tonic-generated protos, only this file (and `controller.rs`)
//! changes.

pub use flyteidl2::flyteidl::common::{ActionIdentifier, ActionPhase, RunIdentifier};
pub use flyteidl2::flyteidl::core::{
    container_error, execution_error, literal, literal_type, primitive, scalar, Binary,
    ContainerError, ErrorDocument, ExecutionError, KeyValuePair, Literal, LiteralType, Primitive,
    Scalar, SimpleType, TypedInterface, Variable, VariableEntry, VariableMap,
};
pub use flyteidl2::flyteidl::task::{Inputs, NamedLiteral, Outputs};
pub use prost::Message;
