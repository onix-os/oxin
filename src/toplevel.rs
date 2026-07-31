//! xdg-shell application windows: creation, map/unmap/destroy, removal.

use crate::ffi::*;
use crate::keybindings::focus_index;
use crate::state::*;
use crate::tiling::{
    active_output, active_workspace, predict_tile_rect, refresh, tree_track, tree_untrack,
    workspace_of,
};
use crate::wlr;
use std::ffi::CStr;
use std::os::raw::c_void;
use std::ptr;

/// Called by the shim when a client creates an application window (toplevel).
pub(crate) unsafe extern "C" fn handle_new_toplevel(userdata: *mut c_void, data: *mut c_void) {
    let server = userdata as *mut Server;
    let toplevel = data as *mut wlr::wlr_xdg_toplevel;

    // Give it a scene node, then track it in Rust. We don't add it to the
    // layout yet — that happens on map, when it actually has content.
    let scene_tree = oxide_scene_add_xdg_toplevel((*server).tree_normal, toplevel);
    let tl = Box::into_raw(Box::new(Toplevel {
        server,
        xdg_toplevel: toplevel,
        scene_tree,
        x: 0,
        y: 0,
        w: 0,
        h: 0,
        fullscreen: false,
        floating: false,
        commit_listener: ptr::null_mut(),
        map_listener: ptr::null_mut(),
        unmap_listener: ptr::null_mut(),
        destroy_listener: ptr::null_mut(),
        fullscreen_listener: ptr::null_mut(),
        corner_swapchain: ptr::null_mut(),
        corner_swapchain_w: 0,
        corner_swapchain_h: 0,
    }));

    // Listen for its lifecycle so Rust can keep the window list current. We keep
    // the listener handles to unregister them on destroy.
    let ud = tl as *mut c_void;
    (*tl).commit_listener = oxide_xdg_add_commit(toplevel, handle_commit, ud);
    (*tl).map_listener = oxide_xdg_add_map(toplevel, handle_map, ud);
    (*tl).unmap_listener = oxide_xdg_add_unmap(toplevel, handle_unmap, ud);
    (*tl).destroy_listener = oxide_xdg_add_destroy(toplevel, handle_destroy, ud);
    (*tl).fullscreen_listener =
        oxide_xdg_add_request_fullscreen(toplevel, handle_request_fullscreen, ud);
}

/// Put a window into or out of fullscreen: full output box, painted above
/// layer-shell bars (the `tree_fullscreen` scene layer). Also answers the
/// client — the protocol requires every state request to get a configure,
/// which `wlr_xdg_toplevel_set_fullscreen` schedules.
pub(crate) unsafe fn set_fullscreen(server: &mut Server, tl: *mut Toplevel, on: bool) {
    if (*tl).fullscreen == on {
        // Still answer the request (a configure is mandatory either way).
        wlr::wlr_xdg_toplevel_set_fullscreen((*tl).xdg_toplevel, on);
        return;
    }
    // Untrack/track around the flag flip: tiled_position (which both read)
    // needs to see the state from the side it's being called on.
    let ws_idx = workspace_of(server, tl);
    if on && !(*tl).floating {
        if let Some(a) = ws_idx {
            tree_untrack(&mut server.workspaces[a], tl);
        }
    }
    (*tl).fullscreen = on;
    wlr::wlr_xdg_toplevel_set_fullscreen((*tl).xdg_toplevel, on);
    let tree = match (on, (*tl).floating) {
        (true, _) => server.tree_fullscreen,
        (false, true) => server.tree_floating,
        (false, false) => server.tree_normal,
    };
    oxide_scene_tree_reparent((*tl).scene_tree, tree);
    if !on && !(*tl).floating {
        if let Some(a) = ws_idx {
            tree_track(&mut server.workspaces[a], tl);
        }
    }
    refresh(server);
    // A floating window keeps no protocol-side size when fullscreen ends
    // (refresh skips it), so restore its remembered floating rect itself.
    if !on && (*tl).floating {
        place_floating(server, tl, (*tl).w, (*tl).h);
    }
    println!("0xin: fullscreen {}", if on { "on" } else { "off" });
}

/// Toggle `tl` as the sole visible window on its workspace. Unlike
/// `set_fullscreen`, this never touches the split tree, never reparents the
/// scene node, and never tells the client it's protocol-fullscreen (an
/// ordinary resize configure only) — it just hides every sibling's scene
/// node and sizes `tl` to the usable area (respecting layer-shell bars). A
/// no-op for floating/fullscreen windows (solo only applies to tiled ones)
/// and for a redundant on/off flip.
pub(crate) unsafe fn set_solo(server: &mut Server, tl: *mut Toplevel, on: bool) {
    if (*tl).fullscreen || (*tl).floating {
        return;
    }
    let Some(a) = workspace_of(server, tl) else {
        return;
    };
    let current = server.workspaces[a].solo;
    if on == (current == Some(tl)) {
        return;
    }
    server.workspaces[a].solo = if on { Some(tl) } else { None };
    refresh(server);
    println!("0xin: solo {}", if on { "on" } else { "off" });
}

/// Float or re-tile a window. Floating windows keep their own size (no tiled
/// states, configures are hints), paint above tiled ones (the `tree_floating`
/// scene layer), and hold no leaf in the split tree; re-tiling restores the
/// tiled states so `refresh()`'s sizes bind again.
pub(crate) unsafe fn set_floating(server: &mut Server, tl: *mut Toplevel, on: bool) {
    if (*tl).floating == on {
        return;
    }
    // Same untrack-before/track-after split as set_fullscreen, and for the
    // same reason: tiled_position needs to see the pre-flip state to find
    // the leaf, and the post-flip state to know where it belongs again.
    let ws_idx = workspace_of(server, tl);
    if on && !(*tl).fullscreen {
        if let Some(a) = ws_idx {
            tree_untrack(&mut server.workspaces[a], tl);
            // Floating a solo'd window would otherwise leave solo's forced
            // full-usable-rect placement fighting place_floating's centered
            // sizing on every refresh — end solo instead.
            if server.workspaces[a].solo == Some(tl) {
                server.workspaces[a].solo = None;
            }
        }
    }
    (*tl).floating = on;
    if !(*tl).fullscreen {
        let tree = if on {
            server.tree_floating
        } else {
            server.tree_normal
        };
        oxide_scene_tree_reparent((*tl).scene_tree, tree);
    }
    if on {
        // The configured default floating size, centered — not the size it
        // happened to have as a tile.
        oxide_xdg_toplevel_set_tiled_none((*tl).xdg_toplevel);
        let (w, h) = float_default_size(server);
        place_floating(server, tl, w, h);
    } else {
        oxide_xdg_toplevel_set_tiled_all((*tl).xdg_toplevel);
        if !(*tl).fullscreen {
            if let Some(a) = ws_idx {
                tree_track(&mut server.workspaces[a], tl);
            }
        }
    }
    refresh(server);
    println!("0xin: floating {}", if on { "on" } else { "off" });
}

/// Center a floating window (at `w`×`h`) in the active output's usable area
/// and record the rect. Floating sizes are the client's own, but a window
/// with a remembered size larger than the output (file pickers, browsers)
/// would center with its header pushed off-screen — so cap the size hint to
/// the usable area and clamp the position into it. The hint is non-binding
/// (no tiled states); the position clamp is what guarantees the top-left
/// corner stays reachable either way.
unsafe fn place_floating(server: &Server, tl: *mut Toplevel, w: i32, h: i32) {
    if server.outputs.is_empty() {
        return;
    }
    let o = &server.outputs[active_output(server)];
    let (w, h) = (w.min(o.uw), h.min(o.uh));
    let x = (o.ux + (o.uw - w) / 2).max(o.ux);
    let y = (o.uy + (o.uh - h) / 2).max(o.uy);
    oxide_scene_tree_set_position((*tl).scene_tree, x, y);
    wlr::wlr_xdg_toplevel_set_size((*tl).xdg_toplevel, w, h);
    oxide_scene_tree_set_clip((*tl).scene_tree, 0, 0);
    ((*tl).x, (*tl).y, (*tl).w, (*tl).h) = (x, y, w, h);
}

/// Clamp a floating window's position into the usable area of the output
/// currently showing its workspace (so it can't be pushed under a bar or off
/// the screen). Position passes through unchanged when the workspace isn't
/// on any output. Shared by keyboard nudges and pointer-grab moves.
pub(crate) unsafe fn clamp_floating(
    server: &Server,
    tl: *mut Toplevel,
    x: i32,
    y: i32,
) -> (i32, i32) {
    let Some(ws_idx) = workspace_of(server, tl) else {
        return (x, y);
    };
    let Some(o) = server.outputs.iter().find(|o| o.workspace == ws_idx) else {
        return (x, y);
    };
    (
        x.clamp(o.ux, (o.ux + o.uw - (*tl).w).max(o.ux)),
        y.clamp(o.uy, (o.uy + o.uh - (*tl).h).max(o.uy)),
    )
}

/// Does this window float *at its own natural size*? True for dialogs (a
/// parent toplevel is set — file pickers, "Save as…") and windows declaring
/// a fixed size: their size is exactly what floating exists to preserve.
/// Checked on the initial commit (the first configure already differs) and
/// re-checked on map for a late-set parent.
unsafe fn floats_naturally(tl: *mut Toplevel) -> bool {
    !oxide_xdg_toplevel_parent((*tl).xdg_toplevel).is_null()
        || oxide_xdg_toplevel_fixed_size((*tl).xdg_toplevel)
}

/// Does a `float = <app_id>` config rule float this window? Rule windows are
/// ordinary apps told to float, so they get the configured default size
/// (`float_size`) rather than their own.
unsafe fn floats_by_rule(server: &Server, tl: *mut Toplevel) -> bool {
    let app_id = oxide_xdg_toplevel_app_id((*tl).xdg_toplevel);
    if app_id.is_null() {
        return false;
    }
    let app_id = CStr::from_ptr(app_id)
        .to_string_lossy()
        .to_ascii_lowercase();
    server.config.float_rules.contains(&app_id)
}

/// The configured default floating size (`float_size`, percentages) applied
/// to the active output's usable area. (0, 0) — "client decides" — when no
/// output exists yet.
unsafe fn float_default_size(server: &Server) -> (i32, i32) {
    if server.outputs.is_empty() {
        return (0, 0);
    }
    let o = &server.outputs[active_output(server)];
    let (pw, ph) = server.config.float_size;
    (o.uw * pw / 100, o.uh * ph / 100)
}

/// The client asked to enter or leave fullscreen (e.g. F11). Apply whatever
/// it requested; the answer-configure happens inside set_fullscreen.
unsafe extern "C" fn handle_request_fullscreen(userdata: *mut c_void, _data: *mut c_void) {
    let tl = userdata as *mut Toplevel;
    let server = &mut *(*tl).server;
    let want = oxide_xdg_toplevel_requested_fullscreen((*tl).xdg_toplevel);
    set_fullscreen(server, tl, want);
}

/// Every surface commit; only the client's very first one matters here. That
/// initial commit must be answered with a configure (or the client never
/// maps) — and the size we put in it is the client's first real size hint.
/// Answering `0,0` ("pick your own size") lets clients map at their
/// remembered/preferred size — often larger than their tile, spilling across
/// outputs, and some (e.g. browsers) then mishandle the immediate resize that
/// follows on map. Instead, predict the tile this window will get — it joins
/// the end of the active output's workspace, so `predict_tile_rect` simulates
/// appending it to that workspace's split tree — and send that, so the first
/// frame the client ever draws already fits.
unsafe extern "C" fn handle_commit(userdata: *mut c_void, _data: *mut c_void) {
    let tl = userdata as *mut Toplevel;
    let server = &*(*tl).server;
    // Surface buffers do not necessarily exist when the scene tree is first
    // created. Reapply on every commit so new buffers and subsurfaces inherit
    // the configured application opacity.
    oxide_scene_tree_set_opacity((*tl).scene_tree, server.config.window_opacity);
    // Fullscreen windows are excluded: rounding a window's edges against the
    // bare screen looks wrong, and skipping the extra GPU pass on exactly the
    // "video playing, committing every frame" case is a real perf win too.
    if server.config.corner_radius > 0 && !(*tl).fullscreen {
        oxide_toplevel_apply_corner_radius(
            server.renderer,
            server.allocator,
            server.corner_program,
            (*tl).scene_tree,
            oxide_xdg_toplevel_surface((*tl).xdg_toplevel),
            server.config.corner_radius,
            &mut (*tl).corner_swapchain,
            &mut (*tl).corner_swapchain_w,
            &mut (*tl).corner_swapchain_h,
        );
    }
    if !oxide_xdg_initial_commit((*tl).xdg_toplevel) {
        return;
    }

    // Floating windows get the opposite treatment: no tiled states, and
    // either a 0,0 configure ("pick your own size" — dialogs and fixed-size
    // windows, whose natural size is the point) or the configured default
    // floating size (`float =` rule windows: ordinary apps told to float).
    // This is why detection happens here and not on map: the very first
    // configure already differs.
    if floats_naturally(tl) {
        (*tl).floating = true;
        wlr::wlr_xdg_toplevel_set_size((*tl).xdg_toplevel, 0, 0);
        println!("0xin: new window — floating, initial configure 0x0");
        return;
    }
    if floats_by_rule(server, tl) {
        (*tl).floating = true;
        let (w, h) = float_default_size(server);
        wlr::wlr_xdg_toplevel_set_size((*tl).xdg_toplevel, w, h);
        println!("0xin: new window — floating (rule), initial configure {w}x{h}");
        return;
    }

    let (mut w, mut h) = (0, 0); // 0,0 = client decides (no output to predict from)
    if !server.outputs.is_empty() {
        let o = &server.outputs[active_output(server)];
        let ws = &server.workspaces[o.workspace];
        (_, _, w, h) = predict_tile_rect(ws, o.ux, o.uy, o.uw, o.uh, server.config.gap);
    }
    // Tiled state makes the size binding: without it this configure has
    // floating semantics, and clients with a remembered size (Firefox) may
    // use that instead of what we send.
    oxide_xdg_toplevel_set_tiled_all((*tl).xdg_toplevel);
    wlr::wlr_xdg_toplevel_set_size((*tl).xdg_toplevel, w, h);
    println!("0xin: new window — initial configure {w}x{h}");
}

/// A window's surface became mapped: add it to the focused output's workspace,
/// re-tile and focus it.
unsafe extern "C" fn handle_map(userdata: *mut c_void, _data: *mut c_void) {
    let tl = userdata as *mut Toplevel;
    let server = &mut *(*tl).server;
    // Mapping is a final backstop after the client has attached its initial
    // buffer. Commits keep this updated for later buffer/subsurface changes.
    oxide_scene_tree_set_opacity((*tl).scene_tree, server.config.window_opacity);
    if server.outputs.is_empty() {
        return; // no monitor to place it on yet
    }
    // Backstop for clients that set their dialog parent (or committed their
    // fixed size) after the initial commit — complete by map time.
    if !(*tl).floating && floats_naturally(tl) {
        (*tl).floating = true;
    }
    if (*tl).floating {
        // Center it at the natural size the client just committed, above
        // the tiled windows.
        oxide_scene_tree_reparent((*tl).scene_tree, server.tree_floating);
        let (mut w, mut h) = (0, 0);
        oxide_xdg_toplevel_geometry((*tl).xdg_toplevel, &mut w, &mut h);
        place_floating(server, tl, w, h);
    }
    let a = active_workspace(server);
    server.workspaces[a].windows.push(tl);
    if !(*tl).floating {
        tree_track(&mut server.workspaces[a], tl);
    }
    refresh(server);
    focus_index(server, server.workspaces[a].windows.len() - 1);
    println!(
        "0xin: window mapped — ws {} now {} ({})",
        a + 1,
        server.workspaces[a].windows.len(),
        if (*tl).floating { "floating" } else { "tiled" }
    );
    // A client may request fullscreen before it maps (e.g. launched with
    // --fullscreen); the request struct is meant to be checked on map.
    if oxide_xdg_toplevel_requested_fullscreen((*tl).xdg_toplevel) {
        set_fullscreen(server, tl, true);
    }
}

/// A window's surface was unmapped (hidden): drop it from the layout.
unsafe extern "C" fn handle_unmap(userdata: *mut c_void, _data: *mut c_void) {
    let tl = userdata as *mut Toplevel;
    let server = &mut *(*tl).server;
    remove_window(server, tl);
}

/// A window was destroyed: unregister its listeners, drop it from the layout,
/// and free our tracking.
unsafe extern "C" fn handle_destroy(userdata: *mut c_void, _data: *mut c_void) {
    let tl = userdata as *mut Toplevel;
    let server = &mut *(*tl).server;
    // Remove every listener we put on this window before wlroots frees it.
    oxide_listener_remove((*tl).commit_listener);
    oxide_listener_remove((*tl).map_listener);
    oxide_listener_remove((*tl).unmap_listener);
    oxide_listener_remove((*tl).destroy_listener);
    oxide_listener_remove((*tl).fullscreen_listener);
    oxide_swapchain_destroy((*tl).corner_swapchain);
    remove_window(server, tl);
    drop(Box::from_raw(tl));
}

/// Remove a window from whichever workspace holds it, then re-tile and focus.
unsafe fn remove_window(server: &mut Server, tl: *mut Toplevel) {
    // If it's the window being dragged, the grab dies with it — otherwise
    // the next motion event dereferences a freed Toplevel.
    if server.grab_tl == tl {
        server.grab = GrabMode::None;
        server.grab_tl = ptr::null_mut();
    }
    for ws in server.workspaces.iter_mut() {
        if let Some(pos) = ws.windows.iter().position(|&w| w == tl) {
            if !(*tl).floating && !(*tl).fullscreen {
                tree_untrack(ws, tl);
            }
            if ws.solo == Some(tl) {
                ws.solo = None;
            }
            ws.windows.remove(pos);
            if ws.focused >= ws.windows.len() && !ws.windows.is_empty() {
                ws.focused = ws.windows.len() - 1;
            }
            break;
        }
    }
    refresh(server);
    if !server.outputs.is_empty() {
        let a = active_workspace(server);
        if !server.workspaces[a].windows.is_empty() {
            let f = server.workspaces[a].focused;
            focus_index(server, f);
        }
    }
}
