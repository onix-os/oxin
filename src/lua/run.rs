//! Running a config's Lua from the compositor.
//!
//! Every entry point here has the same shape: take the VM and registry out of
//! the link (which ends the borrow of `Server`), arm a reusable executor,
//! park the server, and drive under a budget. What comes back is one of three
//! outcomes, and the interesting one is `Killed`: the handler is half-applied,
//! so the layout is re-derived and the handler is quarantined rather than left
//! to burn 50ms on every key repeat.

use luna::{StashedFunction, Value};

use crate::state::Server;

use super::drive::{drive, Outcome, FUEL_PER_HANDLER, HANDLER_DEADLINE};
use super::events::{read_rule, Event, WindowRecord, WindowRule};
use super::host::HostRef;
use super::park::park;

/// Run the Lua function bound to a chord.
///
/// # Safety
///
/// `server` must be the live compositor state, uniquely borrowed here.
pub(crate) unsafe fn run_bind(server: &mut Server, id: u32) {
    call(server, id, "keybinding");
}

unsafe fn call(server: &mut Server, id: u32, what: &str) {
    // Copy the link out first: `LuaLink` is `Copy`, so this read ends the
    // borrow of `server` immediately and the three `&mut`s below are to three
    // disjoint allocations.
    let link = server.lua;
    let server_ptr = server as *mut Server;

    let Some((vm, registry)) = link.get() else {
        return;
    };
    if registry.is_quarantined(id) {
        return;
    }
    let Some(stashed) = registry.function(id).cloned() else {
        eprintln!("0xin: lua: no function for {what} {id} — this is a 0xin bug");
        return;
    };

    let armed = vm.try_enter(|ctx| {
        let f = ctx.fetch(&stashed);
        let ex = ctx.fetch(registry.executor());
        ex.restart(ctx, f, ());
        Ok(())
    });
    if let Err(e) = armed {
        eprintln!("0xin: lua: cannot start {what}: {e}");
        return;
    }

    let executor = registry.executor().clone();
    registry.enter_drive();
    // SAFETY: `server_ptr` is the `&mut Server` we were handed, and nothing
    // between here and the end of the drive touches it through that reference
    // — the borrow ended at the `link` copy above.
    let outcome = park(&mut HostRef::Live(server_ptr) as *mut HostRef, || {
        drive(vm, &executor, FUEL_PER_HANDLER, HANDLER_DEADLINE)
    });
    registry.leave_drive();

    match outcome {
        Outcome::Finished => {
            if let Err(e) = vm.try_enter(|ctx| ctx.fetch(&executor).take_result::<()>(ctx)?) {
                eprintln!("0xin: lua: {what} failed: {e}");
            }
        }
        Outcome::Killed(why) => {
            eprintln!(
                "0xin: lua: {what} stopped ({why}); disabling it for this session so a \
                 held key does not stall every repeat"
            );
            registry.quarantine(id);
            // The handler was interrupted part-way, so the window list may no
            // longer match what is on screen. The layout is derived from it,
            // which makes a re-tile a real repair rather than cosmetic.
            crate::tiling::refresh(&mut *server_ptr);
        }
        Outcome::Broken(msg) => eprintln!("0xin: lua: {what} internal error: {msg}"),
    }
}

/// Run every handler registered for `event`, in registration order.
///
/// Each gets its own error boundary: one that raises is reported and the rest
/// still run, because a mistake in the third has nothing to do with the
/// fourth. Returns the first rule a handler asked for, if the event takes one.
///
/// # Safety
///
/// `server` must be the live compositor state, uniquely borrowed here.
unsafe fn dispatch(
    server: &mut Server,
    event: Event,
    build_arg: impl Fn(luna::Context<'_>) -> Result<Value<'_>, luna::Error<'_>> + Copy,
    want_rule: bool,
) -> Option<WindowRule> {
    let link = server.lua;
    let server_ptr = server as *mut Server;
    let (vm, registry) = link.get()?;

    // A handler firing while another is mid-run would need a second `&mut
    // Server` while the first still holds one. Nothing reaches here that way
    // today; if something ever does, say so rather than alias.
    if registry.depth() > 0 {
        eprintln!(
            "0xin: lua: on.{} fired while another handler was running; skipped",
            event.name()
        );
        return None;
    }

    let handlers: Vec<StashedFunction> = registry.handlers_for(event);
    let executor = registry.executor().clone();
    let mut rule = None;

    for (i, handler) in handlers.iter().enumerate() {
        let label = format!("on.{} handler #{}", event.name(), i + 1);

        let armed = vm.try_enter(|ctx| {
            let f = ctx.fetch(handler);
            let arg = build_arg(ctx)?;
            ctx.fetch(&executor).restart(ctx, f, arg);
            Ok(())
        });
        if let Err(e) = armed {
            eprintln!("0xin: lua: cannot start {label}: {e}");
            continue;
        }

        registry.enter_drive();
        // SAFETY: the borrow of `server` ended at the `link` copy above, so
        // this is the only live reference for the duration of the drive.
        let outcome = park(&mut HostRef::Live(server_ptr) as *mut HostRef, || {
            drive(vm, &executor, FUEL_PER_HANDLER, HANDLER_DEADLINE)
        });
        registry.leave_drive();

        match outcome {
            Outcome::Finished if want_rule => {
                let got = vm.try_enter(|ctx| {
                    // `Value<'gc>` cannot escape the arena, so convert here
                    // and let the plain Rust struct out.
                    let v = ctx.fetch(&executor).take_result::<Value>(ctx)??;
                    read_rule(ctx, v)
                        .map_err(|m| luna::Error::from_value(ctx.intern(m.as_bytes()).into()))
                });
                match got {
                    // First handler to claim the window wins; later ones still
                    // run, so they can act even when they do not decide.
                    Ok(Some(r)) if rule.is_none() => rule = Some(r),
                    Ok(_) => {}
                    Err(e) => eprintln!("0xin: lua: {label} failed: {e}"),
                }
            }
            Outcome::Finished => {
                if let Err(e) = vm.try_enter(|ctx| ctx.fetch(&executor).take_result::<()>(ctx)?) {
                    eprintln!("0xin: lua: {label} failed: {e}");
                }
            }
            Outcome::Killed(why) => {
                eprintln!("0xin: lua: {label} stopped ({why}); skipping it from here on");
                crate::tiling::refresh(&mut *server_ptr);
            }
            Outcome::Broken(msg) => eprintln!("0xin: lua: {label} internal error: {msg}"),
        }
    }

    rule
}

/// Everything the session start-up wants to run once the socket is up.
///
/// # Safety
///
/// `server` must be the live compositor state, uniquely borrowed here.
pub(crate) unsafe fn run_startup(server: &mut Server) {
    dispatch(server, Event::Startup, |_| Ok(Value::Nil), false);
}

/// Ask the config what to do with a window that just mapped.
///
/// # Safety
///
/// `server` must be the live compositor state, uniquely borrowed here.
pub(crate) unsafe fn run_window(server: &mut Server, record: &WindowRecord) -> Option<WindowRule> {
    dispatch(
        server,
        Event::Window,
        |ctx| Ok(record.to_table(ctx)?.into()),
        true,
    )
}

/// Tell the config a workspace switch happened.
///
/// # Safety
///
/// `server` must be the live compositor state, uniquely borrowed here.
pub(crate) unsafe fn run_workspace(server: &mut Server, from: usize, to: usize) {
    dispatch(
        server,
        Event::Workspace,
        |ctx| {
            let t = luna::Table::new(&ctx);
            // 1-based, the way a config counts workspaces everywhere else.
            t.set(ctx, "from", from as i64 + 1)?;
            t.set(ctx, "to", to as i64 + 1)?;
            Ok(t.into())
        },
        false,
    );
}
