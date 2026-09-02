//! State the Lua layer keeps across event-loop iterations.
//!
//! Handlers registered from a config are Lua functions that must survive long
//! after the chunk that registered them returned, so they are stashed here
//! rather than held as `'gc` values.

use luna::StashedExecutor;

/// Long-lived Lua state, owned by a `main` local and reached through
/// [`super::link::LuaLink`].
#[derive(Default)]
pub(crate) struct LuaRegistry {
    /// Executors reused across drives, one per nesting depth. `ctx.stash`
    /// allocates, so we do not want a fresh one per keypress.
    executors: Vec<StashedExecutor>,
    /// How many drives are on the Rust stack right now. A deferred event
    /// raised while this is non-zero waits for the unwind.
    depth: u32,
    /// Set when a config failed to load, so `0xinctl` can report why rather
    /// than leaving the user with a silently-default session.
    pub(crate) config_error: Option<String>,
}

impl LuaRegistry {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn depth(&self) -> u32 {
        self.depth
    }

    pub(crate) fn enter_drive(&mut self) {
        self.depth += 1;
    }

    pub(crate) fn leave_drive(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }
}
