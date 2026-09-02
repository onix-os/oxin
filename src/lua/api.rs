//! `oxin.*` functions a running config can call.
//!
//! Named plural-for-all and singular-for-one — `windows()` / `window()` —
//! because this is the vocabulary a sibling tool will eventually reach over
//! the control socket, and the shape has to be right before anything copies
//! it.
//!
//! Everything here needs a live compositor. Calling one while `init.lua` is
//! still loading says so rather than doing nothing: at that point there is no
//! `WAYLAND_DISPLAY` yet, so a spawn would attach to the *host* session
//! instead of ours — a confusing failure worth naming.

use luna::{Callback, CallbackReturn, Context, Error, IntoValue, Table, Value};

use super::host::with_server;

pub(crate) fn install<'gc>(ctx: Context<'gc>, oxin: Table<'gc>) -> Result<(), Error<'gc>> {
    oxin.set_raw(
        &ctx,
        "spawn".into_value(ctx),
        Callback::from_fn(&ctx, |ctx, exec, mut stack| {
            let at = super::module::position(&exec);
            let cmd: luna::String = stack.consume(ctx)?;
            let cmd = String::from_utf8_lossy(cmd.as_bytes()).into_owned();
            // Through the compositor's own spawn path, never `Command::spawn`
            // directly: it resets the child's signal mask and drops our
            // private LD_LIBRARY_PATH. Clients break in subtle ways otherwise
            // (see notes/signals-and-spawning.md).
            finish(
                ctx,
                &at,
                with_server("oxin.spawn", |_| crate::keybindings::spawn(&cmd)),
            )?;
            Ok(CallbackReturn::Return)
        })
        .into_value(ctx),
    )?;

    oxin.set_raw(
        &ctx,
        "workspace".into_value(ctx),
        Callback::from_fn(&ctx, |ctx, exec, mut stack| {
            let at = super::module::position(&exec);
            let n = finish(
                ctx,
                &at,
                with_server("oxin.workspace", |server| {
                    if server.outputs.is_empty() {
                        return 0;
                    }
                    // SAFETY: `active_output` only reads `server`, which we
                    // hold uniquely here.
                    let out = unsafe { crate::tiling::active_output(server) };
                    // 1-based, as everywhere else a config counts them.
                    server.outputs[out].workspace as i64 + 1
                }),
            )?;
            stack.replace(ctx, n);
            Ok(CallbackReturn::Return)
        })
        .into_value(ctx),
    )?;

    oxin.set_raw(
        &ctx,
        "workspaces".into_value(ctx),
        Callback::from_fn(&ctx, |ctx, exec, mut stack| {
            let at = super::module::position(&exec);
            let rows = finish(
                ctx,
                &at,
                with_server("oxin.workspaces", |server| {
                    server
                        .workspaces
                        .iter()
                        .enumerate()
                        .map(|(i, ws)| (i as i64 + 1, ws.windows.len() as i64))
                        .collect::<Vec<_>>()
                }),
            )?;
            let list = Table::new(&ctx);
            for (i, (number, windows)) in rows.into_iter().enumerate() {
                let row = Table::new(&ctx);
                row.set(ctx, "number", number)?;
                row.set(ctx, "windows", windows)?;
                list.set(ctx, i as i64 + 1, row)?;
            }
            stack.replace(ctx, list);
            Ok(CallbackReturn::Return)
        })
        .into_value(ctx),
    )?;

    Ok(())
}

/// Turn a `with_server` result into a Lua error, or the value it produced.
fn finish<'gc, T>(
    ctx: Context<'gc>,
    at: &Option<String>,
    outcome: Result<Result<T, String>, super::park::ParkError>,
) -> Result<T, Error<'gc>> {
    match outcome {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(message)) => Err(super::module::err(ctx, at, message)),
        Err(park) => Err(super::module::err(ctx, at, park.message().to_owned())),
    }
}

/// Unused for now; kept so the value conversion has one home when the query
/// surface grows window records.
pub(crate) fn _unused(_: Value<'_>) {}
