//! What a config chunk builds up while it runs.
//!
//! Assignments do not touch the live compositor: they land here, and only a
//! chunk that reaches its end hands the result over. A config that raises
//! half-way leaves nothing behind, which is what makes "0xin always starts"
//! structural rather than a thing to remember.

use std::collections::HashSet;

use luna::StashedFunction;

use crate::config::defaults::default_binds;
use crate::config::{Bind, Config};

/// A chord, already resolved to what the keyboard handler matches on.
pub(crate) type Chord = (u32, u32);

/// Config-in-progress, plus the Lua functions it registered.
#[derive(Default)]
pub(crate) struct Staged {
    pub(crate) config: Config,
    /// Lua functions bound to chords, indexed by the id in `Action::Lua`.
    pub(crate) functions: Vec<StashedFunction>,
    /// Chords the config explicitly set to `nil`. A default must not come back
    /// for one of these when the built-ins are filled in.
    pub(crate) unbound: HashSet<Chord>,
    /// Handlers registered per event, in registration order.
    pub(crate) handlers: [Vec<StashedFunction>; super::events::Event::ALL.len()],
    /// Whether any binding has already resolved `MOD`. Changing the modifier
    /// after that point would silently move those bindings, so it is refused.
    pub(crate) mod_resolved: bool,
}

impl Staged {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Record a Lua function and return the id an `Action::Lua` carries.
    pub(crate) fn add_function(&mut self, f: StashedFunction) -> u32 {
        self.functions.push(f);
        (self.functions.len() - 1) as u32
    }

    /// Fill in the built-in keymap and hand back the finished config.
    ///
    /// Defaults are applied *last*, and only for chords the config did not
    /// speak for. Doing it here rather than up front is what lets
    /// `oxin.modifier` be set anywhere in the file: the built-ins are built
    /// against the modifier the config actually ended up with.
    ///
    /// This is `docs/design.md`'s "user config merges, never replaces": a
    /// two-line config is a two-line diff, not a fork of the whole keymap.
    pub(crate) fn finish(mut self) -> Finished {
        let bound: HashSet<Chord> = self
            .config
            .binds
            .iter()
            .map(|b| (b.mods, b.keysym))
            .collect();

        for d in default_binds(self.config.modifier) {
            let chord = (d.mods, d.keysym);
            if bound.contains(&chord) || self.unbound.contains(&chord) {
                continue;
            }
            self.config.binds.push(d);
        }

        Finished {
            config: self.config,
            functions: self.functions,
            handlers: self.handlers,
        }
    }

    /// Replace or add a bind for one chord.
    pub(crate) fn set_bind(&mut self, bind: Bind) {
        match self
            .config
            .binds
            .iter_mut()
            .find(|b| b.mods == bind.mods && b.keysym == bind.keysym)
        {
            Some(existing) => *existing = bind,
            None => self.config.binds.push(bind),
        }
    }

    /// Remove a bind, and remember that a default must not take its place.
    pub(crate) fn clear_bind(&mut self, chord: Chord) {
        self.config.binds.retain(|b| (b.mods, b.keysym) != chord);
        self.unbound.insert(chord);
    }
}

/// What a successful config load produces.
pub(crate) struct Finished {
    pub(crate) config: Config,
    pub(crate) functions: Vec<StashedFunction>,
    pub(crate) handlers: [Vec<StashedFunction>; super::events::Event::ALL.len()],
}
