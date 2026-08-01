//! xdg-shell application windows: the state transitions that make up 0xin's
//! window policy — fullscreen, solo, floating, and the map/unmap bookkeeping
//! that keeps each workspace's split tree in sync.

use smithay::desktop::Window;
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Size};
use smithay::wayland::compositor::with_states;
use smithay::wayland::shell::xdg::{SurfaceCachedState, XdgToplevelSurfaceData};

use crate::keybindings::focus_index;
use crate::state::{GrabMode, Oxin};
use crate::tiling::{
    active_output, active_workspace, place, refresh, rect, tree_track, tree_untrack,
};
use crate::window;

/// Put a window into or out of fullscreen: full output box, painted above
/// layer-shell bars. Also answers the client — the protocol requires every
/// state request to get a configure.
pub fn set_fullscreen(state: &mut Oxin, win: &Window, on: bool) {
    let already = window::is_fullscreen(win);
    if already == on {
        // Still answer the request (a configure is mandatory either way).
        send_fullscreen_state(win, on);
        return;
    }
    // Untrack/track around the flag flip: tiled_position (which both read)
    // needs to see the state from the side it's being called on.
    let ws_idx = state.workspace_of(win);
    if on && !window::is_floating(win) {
        if let Some(index) = ws_idx {
            tree_untrack(&mut state.workspaces[index], win);
        }
    }
    window::data_mut(win).fullscreen = on;
    send_fullscreen_state(win, on);
    if !on && !window::is_floating(win) {
        if let Some(index) = ws_idx {
            tree_track(&mut state.workspaces[index], win);
        }
    }
    refresh(state);
    // A floating window keeps no protocol-side size when fullscreen ends
    // (refresh only restores its remembered rect), so re-centre it.
    if !on && window::is_floating(win) {
        let size = window::rect(win).size;
        place_floating(state, win, size.w, size.h);
    }
    println!("0xin: fullscreen {}", if on { "on" } else { "off" });
}

fn send_fullscreen_state(win: &Window, on: bool) {
    let Some(toplevel) = win.toplevel() else {
        return;
    };
    toplevel.with_pending_state(|pending| {
        if on {
            pending.states.set(xdg_toplevel::State::Fullscreen);
        } else {
            pending.states.unset(xdg_toplevel::State::Fullscreen);
        }
    });
    toplevel.send_pending_configure();
}

/// Toggle `win` as the sole visible window on its workspace. Unlike
/// `set_fullscreen`, this never touches the split tree and never tells the
/// client it is protocol-fullscreen (an ordinary resize configure only) — it
/// just hides every sibling and sizes `win` to the usable area (respecting
/// layer-shell bars). A no-op for floating/fullscreen windows (solo only
/// applies to tiled ones) and for a redundant on/off flip.
pub fn set_solo(state: &mut Oxin, win: &Window, on: bool) {
    if window::is_fullscreen(win) || window::is_floating(win) {
        return;
    }
    let Some(index) = state.workspace_of(win) else {
        return;
    };
    let current = state.workspaces[index].solo.clone();
    if on == (current.as_ref() == Some(win)) {
        return;
    }
    state.workspaces[index].solo = if on { Some(win.clone()) } else { None };
    refresh(state);
    println!("0xin: solo {}", if on { "on" } else { "off" });
}

/// Float or re-tile a window. Floating windows keep their own size (no tiled
/// states, configures are hints), paint above tiled ones, and hold no leaf in
/// the split tree; re-tiling restores the tiled states so `refresh()`'s sizes
/// bind again.
pub fn set_floating(state: &mut Oxin, win: &Window, on: bool) {
    if window::is_floating(win) == on {
        return;
    }
    // Same untrack-before/track-after split as set_fullscreen, and for the
    // same reason: tiled_position needs to see the pre-flip state to find the
    // leaf, and the post-flip state to know where it belongs again.
    let ws_idx = state.workspace_of(win);
    if on && !window::is_fullscreen(win) {
        if let Some(index) = ws_idx {
            tree_untrack(&mut state.workspaces[index], win);
            // Floating a solo'd window would otherwise leave solo's forced
            // full-usable-rect placement fighting place_floating's centred
            // sizing on every refresh — end solo instead.
            if state.workspaces[index].solo.as_ref() == Some(win) {
                state.workspaces[index].solo = None;
            }
        }
    }
    window::data_mut(win).floating = on;
    if on {
        set_tiled_states(win, false);
        // The configured default floating size, centred — not the size it
        // happened to have as a tile.
        let (w, h) = float_default_size(state);
        place_floating(state, win, w, h);
    } else {
        set_tiled_states(win, true);
        if !window::is_fullscreen(win) {
            if let Some(index) = ws_idx {
                tree_track(&mut state.workspaces[index], win);
            }
        }
    }
    refresh(state);
    println!("0xin: floating {}", if on { "on" } else { "off" });
}

/// Tiled state makes a configure's size binding: without it the configure has
/// floating semantics, and clients with a remembered size (Firefox) may use
/// that instead of what we sent.
pub fn set_tiled_states(win: &Window, tiled: bool) {
    let Some(toplevel) = win.toplevel() else {
        return;
    };
    toplevel.with_pending_state(|pending| {
        for edge in [
            xdg_toplevel::State::TiledLeft,
            xdg_toplevel::State::TiledRight,
            xdg_toplevel::State::TiledTop,
            xdg_toplevel::State::TiledBottom,
        ] {
            if tiled {
                pending.states.set(edge);
            } else {
                pending.states.unset(edge);
            }
        }
    });
}

/// Centre a floating window (at `w`×`h`) in the active output's usable area
/// and record the rect. Floating sizes are the client's own, but a window with
/// a remembered size larger than the output (file pickers, browsers) would
/// centre with its header pushed off-screen — so cap the size hint to the
/// usable area and clamp the position into it. The hint is non-binding (no
/// tiled states); the position clamp is what guarantees the top-left corner
/// stays reachable either way.
pub fn place_floating(state: &mut Oxin, win: &Window, w: i32, h: i32) {
    if state.outputs.is_empty() {
        return;
    }
    let usable = state.outputs[active_output(state)].usable;
    let (w, h) = (w.min(usable.size.w), h.min(usable.size.h));
    let x = (usable.loc.x + (usable.size.w - w) / 2).max(usable.loc.x);
    let y = (usable.loc.y + (usable.size.h - h) / 2).max(usable.loc.y);
    place(state, win, rect(x, y, w, h));
}

/// Clamp a floating window's position into the usable area of the output
/// currently showing its workspace (so it can't be pushed under a bar or off
/// the screen). Position passes through unchanged when the workspace isn't on
/// any output. Shared by keyboard nudges and pointer-grab moves.
pub fn clamp_floating(state: &Oxin, win: &Window, x: i32, y: i32) -> (i32, i32) {
    let Some(ws_idx) = state.workspace_of(win) else {
        return (x, y);
    };
    let Some(entry) = state.outputs.iter().find(|o| o.workspace == ws_idx) else {
        return (x, y);
    };
    let size = window::rect(win).size;
    let usable = entry.usable;
    (
        x.clamp(usable.loc.x, (usable.loc.x + usable.size.w - size.w).max(usable.loc.x)),
        y.clamp(usable.loc.y, (usable.loc.y + usable.size.h - size.h).max(usable.loc.y)),
    )
}

/// Does this window float *at its own natural size*? True for dialogs (a
/// parent toplevel is set — file pickers, "Save as…") and windows declaring a
/// fixed size: their size is exactly what floating exists to preserve.
pub fn floats_naturally(win: &Window) -> bool {
    let Some(toplevel) = win.toplevel() else {
        return false;
    };
    if toplevel.parent().is_some() {
        return true;
    }
    with_states(toplevel.wl_surface(), |states| {
        let mut cached = states.cached_state.get::<SurfaceCachedState>();
        let current = cached.current();
        let (min, max) = (current.min_size, current.max_size);
        min.w > 0 && min.h > 0 && min == max
    })
}

/// Does a `float = <app_id>` config rule float this window? Rule windows are
/// ordinary apps told to float, so they get the configured default size
/// (`float_size`) rather than their own.
pub fn floats_by_rule(state: &Oxin, win: &Window) -> bool {
    let Some(app_id) = app_id(win) else {
        return false;
    };
    state.config.float_rules.contains(&app_id.to_ascii_lowercase())
}

pub fn app_id(win: &Window) -> Option<String> {
    let toplevel = win.toplevel()?;
    with_states(toplevel.wl_surface(), |states| {
        states
            .data_map
            .get::<XdgToplevelSurfaceData>()
            .and_then(|data| data.lock().ok().and_then(|data| data.app_id.clone()))
    })
}

/// The configured default floating size (`float_size`, percentages) applied to
/// the active output's usable area. (0, 0) — "client decides" — when no output
/// exists yet.
pub fn float_default_size(state: &Oxin) -> (i32, i32) {
    if state.outputs.is_empty() {
        return (0, 0);
    }
    let usable = state.outputs[active_output(state)].usable;
    let (pw, ph) = state.config.float_size;
    (usable.size.w * pw / 100, usable.size.h * ph / 100)
}

/// A window's surface became mapped: add it to the active output's workspace,
/// re-tile and focus it.
pub fn map_window(state: &mut Oxin, win: &Window) {
    if state.outputs.is_empty() {
        return; // no monitor to place it on yet
    }
    // Backstop for clients that set their dialog parent (or committed their
    // fixed size) after the initial commit — complete by map time.
    if !window::is_floating(win) && floats_naturally(win) {
        window::data_mut(win).floating = true;
    }
    let workspace = active_workspace(state);
    state.workspaces[workspace].windows.push(win.clone());
    if window::is_floating(win) {
        // Centre it at the natural size the client just committed.
        let geometry = win.geometry().size;
        place_floating(state, win, geometry.w, geometry.h);
    } else {
        tree_track(&mut state.workspaces[workspace], win);
    }
    refresh(state);
    let last = state.workspaces[workspace].windows.len() - 1;
    focus_index(state, last);
    println!(
        "0xin: window mapped — ws {} now {} ({})",
        workspace + 1,
        state.workspaces[workspace].windows.len(),
        if window::is_floating(win) {
            "floating"
        } else {
            "tiled"
        }
    );
    // A client may request fullscreen before it maps (e.g. launched with
    // --fullscreen); its pending state carries that through to here.
    let wants_fullscreen = win
        .toplevel()
        .map(|toplevel| {
            toplevel.with_pending_state(|pending| {
                pending.states.contains(xdg_toplevel::State::Fullscreen)
            })
        })
        .unwrap_or(false);
    if wants_fullscreen {
        set_fullscreen(state, win, true);
    }
}

/// Remove a window from whichever workspace holds it, then re-tile and focus.
pub fn unmap_window(state: &mut Oxin, win: &Window) {
    // If it's the window being dragged, the grab dies with it.
    if state.grab_window.as_ref() == Some(win) {
        state.grab = GrabMode::None;
        state.grab_window = None;
    }
    state.space.unmap_elem(win);
    for ws in state.workspaces.iter_mut() {
        if let Some(position) = ws.windows.iter().position(|w| w == win) {
            if window::is_tiled(win) {
                tree_untrack(ws, win);
            }
            if ws.solo.as_ref() == Some(win) {
                ws.solo = None;
            }
            ws.windows.remove(position);
            if ws.focused >= ws.windows.len() && !ws.windows.is_empty() {
                ws.focused = ws.windows.len() - 1;
            }
            break;
        }
    }
    refresh(state);
    if !state.outputs.is_empty() {
        let workspace = active_workspace(state);
        if !state.workspaces[workspace].windows.is_empty() {
            let focused = state.workspaces[workspace].focused;
            focus_index(state, focused);
        }
    }
}

/// The size to put in a brand-new window's very first configure.
///
/// That initial commit must be answered with a configure (or the client never
/// maps) — and the size we put in it is the client's first real size hint.
/// Answering `0,0` ("pick your own size") lets clients map at their
/// remembered/preferred size — often larger than their tile, spilling across
/// outputs, and some (e.g. browsers) then mishandle the immediate resize that
/// follows on map. Instead, predict the tile this window will get — it joins
/// the end of the active output's workspace — and send that, so the first
/// frame the client ever draws already fits.
pub fn initial_configure_size(state: &mut Oxin, win: &Window) -> Size<i32, Logical> {
    // Floating windows get the opposite treatment: no tiled states, and either
    // a 0,0 configure ("pick your own size" — dialogs and fixed-size windows,
    // whose natural size is the point) or the configured default floating size
    // (`float =` rule windows: ordinary apps told to float).
    if floats_naturally(win) {
        window::data_mut(win).floating = true;
        println!("0xin: new window — floating, initial configure 0x0");
        return Size::from((0, 0));
    }
    if floats_by_rule(state, win) {
        window::data_mut(win).floating = true;
        let (w, h) = float_default_size(state);
        println!("0xin: new window — floating (rule), initial configure {w}x{h}");
        return Size::from((w, h));
    }

    let mut size = Size::from((0, 0)); // 0,0 = client decides (no output yet)
    if !state.outputs.is_empty() {
        let entry = &state.outputs[active_output(state)];
        let usable = entry.usable;
        let ws = &state.workspaces[entry.workspace];
        let (_, _, w, h) = crate::tiling::predict_tile_rect(
            ws,
            usable.loc.x,
            usable.loc.y,
            usable.size.w,
            usable.size.h,
            state.config.gap,
        );
        size = Size::from((w, h));
    }
    set_tiled_states(win, true);
    println!("0xin: new window — initial configure {}x{}", size.w, size.h);
    size
}

/// Is `surface` the root (toplevel) surface of `win`?
pub fn is_toplevel_surface(win: &Window, surface: &WlSurface) -> bool {
    win.toplevel()
        .map(|toplevel| toplevel.wl_surface() == surface)
        .unwrap_or(false)
}
