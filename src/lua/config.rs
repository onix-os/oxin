//! Loading `init.lua` into a `Config`.
//!
//! The config is applied all-or-nothing. A chunk that sets ten settings and
//! then raises has already made ten assignments, and half a config is a state
//! nobody wrote — so the assignments land on a *staged* `Config` that is only
//! handed back once the chunk has run to the end. On any failure it is
//! dropped and the caller keeps built-in defaults.
//!
//! That is what makes "0xin always starts" a structural property rather than a
//! discipline: there is no path on which a partly-applied config reaches the
//! compositor.

use std::path::Path;

use luna::{Lua, Table};

use crate::config::Config;

use super::drive::{drive, Outcome, CONFIG_DEADLINE, FUEL_FOR_CONFIG};
use super::load::start_chunk;
use super::module;
use super::park::park;

/// Run `source` as the config, returning the settings it asked for.
///
/// `path` is used for the chunk name, so errors carry `file:line`.
pub(crate) fn load(vm: &mut Lua, path: &Path, source: &[u8]) -> Result<Config, String> {
    let mut staged = Config::default();

    // The module has to exist before the chunk runs: it is what the config
    // assigns onto, and `package.loaded` is what makes `require("oxin")` find
    // it without going near the filesystem.
    vm.try_enter(|ctx| {
        let oxin = module::build(ctx, &staged)?;
        let package: Table = ctx.get_global("package")?;
        let loaded: Table = package.get(ctx, "loaded")?;
        loaded.set(ctx, "oxin", oxin)?;
        // Also a global, so a one-line config need not `require` first.
        ctx.set_global("oxin", oxin);
        Ok(())
    })
    .map_err(|e| format!("{}: preparing the oxin module: {e}", path.display()))?;

    let ex = start_chunk(vm, path, source)?;

    // SAFETY: `staged` is a live local, borrowed uniquely for the drive — the
    // only thing that touches it inside is the module's `__newindex`, through
    // the parked pointer — and it is not moved or dropped until after.
    let outcome = unsafe {
        park(&mut staged as *mut Config, || {
            drive(vm, &ex, FUEL_FOR_CONFIG, CONFIG_DEADLINE)
        })
    };

    match outcome {
        Outcome::Finished => {
            // The chunk returns nothing, so this is only ever here to surface
            // an error it raised on the way out.
            vm.try_enter(|ctx| ctx.fetch(&ex).take_result::<()>(ctx)?)
                .map_err(|e| format!("{e}"))?;
            Ok(staged)
        }
        Outcome::Killed(why) => Err(format!(
            "{}: config did not finish ({why}). A loop with no exit, or work \
             far beyond what a config should do at startup.",
            path.display()
        )),
        Outcome::Broken(msg) => Err(format!("{}: internal error: {msg}", path.display())),
    }
}
