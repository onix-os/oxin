//! State the Lua layer keeps across event-loop iterations.
//!
//! Handlers registered from a config are Lua functions that must survive long
//! after the chunk that registered them returned, so they are stashed here
//! rather than held as `'gc` values.

use std::collections::HashSet;

use luna::{StashedExecutor, StashedFunction};

/// Long-lived Lua state, owned by a `main` local and reached through
/// [`super::link::LuaLink`].
#[derive(Default)]
pub(crate) struct LuaRegistry {
    /// Functions the config bound to chords, indexed by the id an
    /// `Action::Lua` carries.
    functions: Vec<StashedFunction>,
    /// Reused across drives: `ctx.stash` allocates a root, and we do not want
    /// one per keypress.
    executor: Option<StashedExecutor>,
    /// Handlers registered per event, in registration order.
    handlers: [Vec<StashedFunction>; crate::lua::events::Event::ALL.len()],
    /// Handlers that were stopped mid-run. Re-running one costs the deadline
    /// every time, so a held-down key would stall on every repeat.
    quarantined: HashSet<u32>,
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

    /// Adopt the functions a config registered.
    pub(crate) fn set_functions(&mut self, functions: Vec<StashedFunction>) {
        self.functions = functions;
    }

    pub(crate) fn set_handlers(
        &mut self,
        handlers: [Vec<StashedFunction>; crate::lua::events::Event::ALL.len()],
    ) {
        self.handlers = handlers;
    }

    /// The handlers for one event, as a snapshot.
    ///
    /// Copied rather than borrowed because a handler may register another one
    /// while we are part-way through dispatching.
    pub(crate) fn handlers_for(&self, event: crate::lua::events::Event) -> Vec<StashedFunction> {
        self.handlers[event.index()].clone()
    }

    pub(crate) fn function(&self, id: u32) -> Option<&StashedFunction> {
        self.functions.get(id as usize)
    }

    /// The shared executor, created on first use.
    pub(crate) fn executor(&self) -> &StashedExecutor {
        self.executor
            .as_ref()
            .expect("executor is installed when the Lua layer starts")
    }

    pub(crate) fn set_executor(&mut self, ex: StashedExecutor) {
        self.executor = Some(ex);
    }

    pub(crate) fn is_quarantined(&self, id: u32) -> bool {
        self.quarantined.contains(&id)
    }

    pub(crate) fn quarantine(&mut self, id: u32) {
        self.quarantined.insert(id);
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
