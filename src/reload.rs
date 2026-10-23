//! Re-reading the config into a running session (Stage 31).
//!
//! The Lua side is the easy half: `lua::session::reload` builds a whole new
//! VM, registry and `Config` and touches nothing live, so swapping them in is
//! three moves. The other half is the settings that are read once, when the
//! thing they shape is created (an output's background, a workspace's split
//! axis, the gesture recognizer's mask), which a new `Config` alone does not
//! reach. `apply` re-applies exactly those.

use std::mem;

use crate::config::Config;
use crate::ffi::oxide_cursor_set_gestures;
use crate::input::handle_gesture;
use crate::keybindings::cancel_hold;
use crate::lua;
use crate::state::Server;
use crate::{output, tiling, toplevel, wallpaper};

/// Load the config again and apply it, or leave the session as it was.
///
/// A config that fails to load is reported and kept on the registry for
/// `0xinctl config-error`, and the running config stays in place: falling
/// back to defaults here would take away every binding the user had because
/// of one typo in a file they are in the middle of editing.
///
/// # Safety
///
/// `server` must be the live compositor state, uniquely borrowed here, and
/// no Lua step may be running (checked: one is refused, not raced).
pub(crate) unsafe fn reload(server: &mut Server) -> Result<(), String> {
    if server.locked {
        return Err("session is locked".into());
    }
    let link = server.lua;
    let Some((vm, registry)) = link.get() else {
        return Err("the Lua layer is not running".into());
    };
    // Replacing the VM under a running handler would free the stack it is
    // executing on. Nothing can reach here from Lua today (`oxin.action.reload`
    // is dispatched by the compositor, not called), so this is a guard, not a
    // path.
    if registry.depth() > 0 {
        return Err("cannot reload from inside a running Lua handler".into());
    }

    let session = match lua::session::reload() {
        Ok(session) => session,
        Err(message) => {
            lua::session::report(&message, "keeping the previous config");
            registry.config_error = Some(format!(
                "reload failed; still running the previous config:\n{message}"
            ));
            return Err(message);
        }
    };

    // A half-held key carries an `Action::Lua` id into the registry about to
    // be dropped, and its chord may not even be a hold bind any more.
    cancel_hold(server);

    // The old registry's stashed handles go before the VM they root into.
    drop(mem::replace(registry, session.registry));
    drop(mem::replace(vm, session.vm));
    let old = mem::replace(&mut server.config, session.config);

    apply(server, &old);
    Ok(())
}

/// Re-apply the settings a swapped `Config` does not reach on its own.
///
/// Everything consulted per event (binds, holds, gestures, the modifier,
/// float size, keyboard commands) already sees the new config; this is the
/// rest.
unsafe fn apply(server: &mut Server, old: &Config) {
    // New splits only. Reshaping a layout the user has already arranged would
    // be a surprise, not a setting.
    let vertical = server.config.first_split_vertical;
    for ws in &mut server.workspaces {
        ws.first_split_vertical = vertical;
    }

    // Applied on commit, so an idle window would keep the old look until it
    // next redraws.
    if server.config.window_opacity != old.window_opacity
        || server.config.corner_radius != old.corner_radius
    {
        let windows: Vec<_> = server
            .workspaces
            .iter()
            .flat_map(|ws| ws.windows.iter().copied())
            .collect();
        for tl in windows {
            toplevel::apply_decorations(server, tl);
        }
    }

    if server.config.wallpaper != old.wallpaper {
        let wanted = server.config.wallpaper.clone();
        if let Err(error) = wallpaper::set(server, wanted.as_deref()) {
            // The rest of the config is fine; keep the picture on screen and
            // say why the new one is not there.
            eprintln!("0xin: reload: wallpaper: {error}; keeping the previous one");
            server.config.wallpaper = old.wallpaper.clone();
        }
    }

    output::apply_config(server);

    // Only field writes in the shim. Never reached while locked (refused
    // above), so this cannot re-enable gestures under a lock screen.
    oxide_cursor_set_gestures(
        server.cursor,
        server.output_layout,
        server.config.gesture_mask(),
        server.config.virtual_keyboard_height,
        server.event_loop,
        handle_gesture,
        server as *mut Server as *mut _,
    );

    if server.config.monitors != old.monitors {
        eprintln!(
            "0xin: reload: oxin.monitors changed; it applies when a monitor is \
             next connected"
        );
    }

    // Gaps, and the keyboard handle's final position.
    tiling::refresh(server);
}
