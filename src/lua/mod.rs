//! The embedded Lua layer.
//!
//! Everything luna-shaped lives under this directory. The rest of 0xin sees
//! only plain Rust types from here — no `Context`, no `Value`, no stashed
//! handles in `Server` or in `config::types`. luna is pre-1.0 and says so, and
//! this boundary is what keeps a breaking bump a one-directory port.

pub(crate) mod action;
pub(crate) mod api;
pub(crate) mod config;
pub(crate) mod drive;
pub(crate) mod events;
pub(crate) mod host;
pub(crate) mod link;
pub(crate) mod load;
pub(crate) mod module;
pub(crate) mod park;
pub(crate) mod plugins;
pub(crate) mod registrars;
pub(crate) mod registry;
pub(crate) mod run;
pub(crate) mod session;
pub(crate) mod staging;
pub(crate) mod vm;

#[cfg(test)]
mod tests;

pub(crate) use run::{run_bind, run_startup, run_window, run_workspace};
