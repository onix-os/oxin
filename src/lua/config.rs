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

use std::path::{Path, PathBuf};

use luna::{Lua, Table};

use super::drive::{drive, Outcome, CONFIG_DEADLINE, FUEL_FOR_CONFIG};
use super::host::HostRef;
use super::load::start_chunk;
use super::module;
use super::park::park;
use super::staging::{Finished, Staged};

/// Run the config and then the plugins, all against one staged config.
///
/// The two have deliberately different failure policies. `init.lua` failing is
/// fatal to the load — carrying on would silently apply settings the user did
/// not ask for. A *plugin* failing is one of several, and taking the rest down
/// with it is worse than doing without it, so it is reported and the others
/// still load.
pub(crate) fn load_all(
    vm: &mut Lua,
    config: Option<(&Path, Vec<u8>)>,
    plugin_files: &[PathBuf],
) -> Result<Finished, String> {
    let mut staged = Staged::new();

    // The module has to exist before the chunk runs: it is what the config
    // assigns onto, and `package.loaded` is what makes `require("oxin")` find
    // it without going near the filesystem.
    vm.try_enter(|ctx| {
        let oxin = module::build(ctx, &staged.config)?;
        let package: Table = ctx.get_global("package")?;
        let loaded: Table = package.get(ctx, "loaded")?;
        loaded.set(ctx, "oxin", oxin)?;
        // Also a global, so a one-line config need not `require` first.
        ctx.set_global("oxin", oxin);
        Ok(())
    })
    .map_err(|e| format!("preparing the oxin module: {e}"))?;

    if let Some((path, source)) = &config {
        run_chunk(vm, &mut staged, path, source)?;
    }

    // Plugins run after init.lua and before the `after/` roots, the same order
    // neovim uses — which is why `after/plugin/` is where you override what a
    // plugin did.
    for path in plugin_files {
        let source = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(e) => {
                eprintln!("0xin: plugin {}: cannot read: {e}", path.display());
                continue;
            }
        };
        if let Err(message) = run_chunk(vm, &mut staged, path, &source) {
            eprintln!("0xin: plugin failed, skipping it: {message}");
        }
    }

    Ok(staged.finish())
}

/// Compile and run one chunk against `staged`.
fn run_chunk(vm: &mut Lua, staged: &mut Staged, path: &Path, source: &[u8]) -> Result<(), String> {
    let ex = start_chunk(vm, path, source)?;

    // SAFETY: `staged` is a live local, borrowed uniquely for the drive — the
    // only thing that touches it inside is the module's `__newindex`, through
    // the parked pointer — and it is not moved or dropped until after.
    let mut host = HostRef::Load(staged as *mut Staged);
    let outcome = unsafe {
        park(&mut host as *mut HostRef, || {
            drive(vm, &ex, FUEL_FOR_CONFIG, CONFIG_DEADLINE)
        })
    };

    match outcome {
        Outcome::Finished => {
            // The chunk returns nothing, so this is only ever here to surface
            // an error it raised on the way out.
            vm.try_enter(|ctx| ctx.fetch(&ex).take_result::<()>(ctx)?)
                .map_err(|e| {
                    let text = e.to_string();
                    text.strip_prefix("lua error: ").unwrap_or(&text).to_owned()
                })?;
            Ok(())
        }
        Outcome::Killed(why) => Err(format!(
            "{}: did not finish ({why}). A loop with no exit, or work far \
             beyond what a config should do at startup.",
            path.display()
        )),
        Outcome::Broken(msg) => Err(format!("{}: internal error: {msg}", path.display())),
    }
}
