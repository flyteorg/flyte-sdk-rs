//! Proc-macros for the Flyte Rust SDK.
//!
//! All generated code references only `::flyte::` paths so the SDK's internals
//! (controller/proto backends) can be swapped without touching user code.

mod flyte_struct;
mod main_attr;
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
/// sibling `fn {name}_entry() -> flyte::TaskEntry` is generated, carrying the
/// task's name, its interface (derived from the signature), and a runner that
/// decodes `Inputs` and encodes `Outputs`.
#[proc_macro_attribute]
pub fn task(attr: TokenStream, item: TokenStream) -> TokenStream {
    task::expand(attr, item)
}

/// Make this task the binary's entrypoint: generates
/// `fn main() -> ExitCode { flyte::worker_main({name}_entry()) }`.
///
/// ```ignore
/// #[flyte::main]
/// #[flyte::task]
/// async fn my_task(x: i64) -> Result<i64, flyte::Error> { Ok(x * 2) }
/// ```
///
/// Pair it with `#[flyte::task]`, which generates the `{name}_entry` fn the
/// generated `main` calls — without it you get `cannot find function
/// {name}_entry in this scope`. Attribute order does not matter, because this
/// macro passes the fn through untouched and only adds `main` beside it.
///
/// The annotated fn must sit at the crate root of a **bin** target (a `main`
/// generated inside a module or a lib target is not the process entrypoint), and
/// only one task per binary can be the entrypoint — hence opt-in rather than
/// part of `#[flyte::task]`.
#[proc_macro_attribute]
pub fn main(attr: TokenStream, item: TokenStream) -> TokenStream {
    main_attr::expand(attr, item)
}

/// Derive `FlyteType` for a serde struct, transported as msgpack (wire
/// compatible with Python dataclasses/pydantic models).
#[proc_macro_derive(FlyteStruct)]
pub fn flyte_struct(item: TokenStream) -> TokenStream {
    flyte_struct::expand(item)
}
