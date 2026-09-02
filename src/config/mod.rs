//! 0xin config: the vocabulary, the built-in keymap, and the name resolution
//! shared with the Lua layer.
//!
//! The config itself is `~/.config/0xin/init.lua`, read by `crate::lua`. What
//! lives here is everything that is *not* interpreter-shaped:
//!
//! - `types` — the structs a config resolves to (`Config`, `Bind`, `Action`, …)
//! - `defaults` — the built-in keymap, which a config overrides chord by chord
//!   rather than replacing wholesale
//! - `names` — modifier and xkb key names, so `"MOD+SHIFT+q"` means what it
//!   always did
//!
//! Keeping this side free of luna types is deliberate: the interpreter is
//! pre-1.0, and the boundary is what makes a breaking version bump a change to
//! `src/lua/` alone.

pub(crate) mod defaults;
pub(crate) mod names;
mod types;

pub use types::*;
