//! Proc-macros for the Flyte Rust SDK.
//!
//! All generated code references only `::flyte::` paths so the SDK's internals
//! (controller/proto backends) can be swapped without touching user code.

mod flyte_struct;
mod task;
mod trace;

use proc_macro::TokenStream;

/// Mark an async fn as a traced step: inside a running task it is recorded as a
/// child trace action and replayed on retry; outside a task context (or nested
/// in another trace) it just runs its body.
///
/// Requirements: `async fn`, by-value params implementing `FlyteType`
/// (primitives or `#[derive(FlyteStruct)]` types), return type
/// `Result<T, flyte::Error>` with `T: FlyteType`.
///
/// `#[flyte::trace(version = "v2")]` overrides the auto identity (a hash of the
/// fn body) — bump it to force re-execution instead of replay.
#[proc_macro_attribute]
pub fn trace(attr: TokenStream, item: TokenStream) -> TokenStream {
    trace::expand(attr, item)
}

/// Mark an async fn as the task entrypoint. The fn itself is unchanged; a
/// sibling `fn {name}_entry() -> flyte::TaskEntry` is generated for use with
/// `flyte::worker_main`.
#[proc_macro_attribute]
pub fn task(attr: TokenStream, item: TokenStream) -> TokenStream {
    task::expand(attr, item)
}

/// Derive `FlyteType` for a serde struct, transported as msgpack (wire
/// compatible with Python dataclasses/pydantic models).
#[proc_macro_derive(FlyteStruct)]
pub fn flyte_struct(item: TokenStream) -> TokenStream {
    flyte_struct::expand(item)
}
