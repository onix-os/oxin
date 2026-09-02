//! The handle the compositor holds on the Lua layer.
//!
//! `Lua` deliberately does *not* live inside `Server`. `Server` is a `main`
//! local whose address is handed to the C shim, and every shim callback
//! reconstructs `&mut Server` from that pointer. If the VM were a field, then
//! `lua.enter(...)` would hold a `&mut server.lua` — a reborrow covering the
//! whole `Server` — and those callbacks would be reading through a pointer
//! whose provenance had been invalidated.
//!
//! So the VM and the registry are separate `main` locals, and `Server` carries
//! only this `Copy` pair of raw pointers to them. Copying the field out ends
//! the borrow of `Server` immediately, which is what lets a drain hold
//! `&mut Server`, `&mut Lua` and `&mut LuaRegistry` at once: three `&mut`s to
//! three disjoint allocations.

use std::ptr;

use luna::Lua;

use super::registry::LuaRegistry;

/// Pointers to the Lua layer, or nulls when Lua is not in use.
#[derive(Clone, Copy)]
pub(crate) struct LuaLink {
    vm: *mut Lua,
    reg: *mut LuaRegistry,
}

impl LuaLink {
    /// The "no Lua" state. `Server` starts here and stays here unless a config
    /// was actually loaded.
    pub(crate) const fn disabled() -> Self {
        LuaLink {
            vm: ptr::null_mut(),
            reg: ptr::null_mut(),
        }
    }

    /// # Safety
    ///
    /// Both pointers must outlive every use of this link — in practice both
    /// point at `main` locals held across `wl_display_run`.
    pub(crate) unsafe fn new(vm: *mut Lua, reg: *mut LuaRegistry) -> Self {
        LuaLink { vm, reg }
    }

    pub(crate) fn is_enabled(self) -> bool {
        !self.vm.is_null() && !self.reg.is_null()
    }

    /// Borrow the VM and the registry together.
    ///
    /// Returns `None` when Lua is disabled, so callers stay a single `if let`.
    ///
    /// # Safety
    ///
    /// The caller must not already hold a borrow of either, and must not be
    /// inside a Lua step (the VM is not re-entrant from here — see
    /// `registry::pump`).
    pub(crate) unsafe fn get(self) -> Option<(&'static mut Lua, &'static mut LuaRegistry)> {
        if !self.is_enabled() {
            return None;
        }
        // SAFETY: both point at live `main` locals, and the caller has
        // promised not to be holding another borrow of either.
        Some(unsafe { (&mut *self.vm, &mut *self.reg) })
    }
}

impl Default for LuaLink {
    fn default() -> Self {
        Self::disabled()
    }
}
