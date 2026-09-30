//! Reusable execution-only Bash interpreter and embedded local command suite.
#[cfg(target_arch = "wasm32")]
pub use brush_core::execution::ExecutionServices;
pub mod commands;
mod error;
mod helpshim;
mod manifest;
mod registry;
pub mod session;
mod tools;
pub use error::ShellError;
