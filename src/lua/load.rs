//! Compiling and starting a chunk, with errors that name the file.
//!
//! luna attaches the chunk name to *runtime* errors ("init.lua:3: boom") but
//! not to compile errors: a syntax error stringifies as "parse error at line
//! 1: ..." with no indication of which file. That is tolerable for one config
//! and useless once plugins are on the runtimepath, so we prepend the path
//! here and every caller gets a consistent `<path>: <what went wrong>`.

use std::path::Path;

use luna::{Closure, Executor, Lua, StashedExecutor};

/// Compile `source` as `path` and start an executor for it.
///
/// The returned error is already formatted for the user: it names the file,
/// and for a runtime failure luna's own message carries the line.
pub(crate) fn start_chunk(
    vm: &mut Lua,
    path: &Path,
    source: &[u8],
) -> Result<StashedExecutor, String> {
    let name = path.display().to_string();
    vm.try_enter(|ctx| {
        // `Some(name)` is what makes runtime errors carry `file:line`; with
        // `None` luna substitutes "<anonymous>".
        let closure = Closure::load(ctx, Some(name.as_str()), source)?;
        Ok(ctx.stash(Executor::start(ctx, closure.into(), ())))
    })
    .map_err(|e| format!("{name}: {e}"))
}
