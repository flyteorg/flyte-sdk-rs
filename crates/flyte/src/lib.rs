//! Flyte SDK for Rust — v0: single-node traces.
//!
//! Write a task as an async fn, mark steps with `#[flyte::trace]`, and run the
//! binary as a Flyte task container via [`worker_main`]. Traced steps execute
//! in-process, are recorded as child actions on the backend, and are replayed
//! on retry. Without a backend (local mode / plain invocation) traced fns just
//! run their bodies.

pub mod context;
#[doc(hidden)]
pub mod controller;
mod error;
#[doc(hidden)]
pub mod hash;
#[doc(hidden)]
pub mod idl;
#[doc(hidden)]
pub mod storage;
#[doc(hidden)]
pub mod trace;
#[doc(hidden)]
pub mod types;
mod worker;

pub use error::Error;
pub use flyte_macros::{task, trace, FlyteStruct};
pub use types::FlyteType;
pub use worker::{run_local, worker_main, TaskEntry};
