//! Flyte SDK for Rust — v0: single-node traces.
//!
//! Write a task as an async fn, mark steps with `#[flyte::trace]`, and add
//! `#[flyte::main]` to turn the crate into a Flyte task container. Traced steps
//! execute in-process, are recorded as child actions on the backend, and are
//! replayed on retry. Without a backend traced fns just run their bodies, so
//! [`run`] executes the whole task in-process for tests and dev loops.

pub mod context;
#[doc(hidden)]
pub mod controller;
mod error;
#[doc(hidden)]
pub mod hash;
#[doc(hidden)]
pub mod idl;
mod interface;
#[doc(hidden)]
pub mod storage;
#[doc(hidden)]
pub mod trace;
#[doc(hidden)]
pub mod types;
mod worker;

pub use error::Error;
pub use flyte_macros::{main, task, trace, FlyteStruct};
pub use interface::{TaskInterface, TaskVariable};
pub use types::FlyteType;
pub use worker::{run, worker_main, TaskEntry};
