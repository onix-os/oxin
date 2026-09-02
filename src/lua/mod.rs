//! The embedded Lua layer.
//!
//! Everything luna-shaped lives under this directory. The rest of 0xin sees
//! only plain Rust types from here — no `Context`, no `Value`, no stashed
//! handles in `Server` or in `config::types`. luna is pre-1.0 and says so, and
//! this boundary is what keeps a breaking bump a one-directory port.

// The layer is built out in phases (see docs); the driver and parking helpers
// land before the call sites that use them, so the pieces are proven in
// isolation first. Drop this once Stage 2 wires the runtime layer in.
#![allow(dead_code)]

pub(crate) mod config;
pub(crate) mod drive;
pub(crate) mod link;
pub(crate) mod load;
pub(crate) mod module;
pub(crate) mod park;
pub(crate) mod registry;
pub(crate) mod session;
pub(crate) mod vm;

#[cfg(test)]
mod tests;
