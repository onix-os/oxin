//! xdg-shell application windows: creation, map/unmap/destroy, removal.

use crate::config::ScratchpadPosition;
use crate::ffi::*;
use crate::keybindings::focus_index;
use crate::state::*;
use crate::tiling::{
    active_output, active_workspace, inset_for_border, predict_tile_rect, refresh, tree_track,
    tree_untrack, workspace_of,
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
        scratchpad: None,
        commit_listener: ptr::null_mut(),
        map_listener: ptr::null_mut(),
        unmap_listener: ptr::null_mut(),
        destroy_listener: ptr::null_mut(),
        fullscreen_listener: ptr::null_mut(),
        corner_masks: ptr::null_mut(),
        subsurface_listener: ptr::null_mut(),
        subsurface_watches: Vec::new(),
        border: ptr::null_mut(),
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
    // Corner-radius masking needs to re-mask on a subsurface's own commit,
    // not just the root's (see shim/gles2_corner.c's module comment). Like
    // the commit listener above, this goes after the scene tree's own
    // listeners on the same signal — see reorder_listeners_after_scene_rebuild.
    // A client may also have added subsurfaces before giving the surface
    // this role; those are picked up by walking what's already there.
    let root = oxide_xdg_toplevel_surface(toplevel);
    (*tl).subsurface_listener = oxide_surface_add_new_subsurface(root, handle_new_subsurface, ud);
    oxide_surface_for_each_subsurface(root, handle_new_subsurface, ud);
}

/// A client added a wl_subsurface to a surface this toplevel already watches
/// (the root the first time, or recursively a subsurface of a subsurface).
/// Tracks it for corner-radius masking: its own commit re-runs the masking
/// sweep, and its own destroy frees its mask swapchain immediately.
unsafe extern "C" fn handle_new_subsurface(userdata: *mut c_void, data: *mut c_void) {
    let tl = userdata as *mut Toplevel;
    let surface = oxide_subsurface_surface(data);
    // Reachable twice — through new_subsurface and through the walk of
    // already-present subsurfaces — so make the second arrival a no-op.
    if (*tl)
        .subsurface_watches
        .iter()
        .any(|&w| (*w).surface == surface)
    {
        return;
    }
    let watch = Box::into_raw(Box::new(SubsurfaceWatch {
        tl,
        surface,
        commit_listener: ptr::null_mut(),
        destroy_listener: ptr::null_mut(),
        new_subsurface_listener: ptr::null_mut(),
    }));
    let wd = watch as *mut c_void;
    (*watch).commit_listener = oxide_surface_add_commit(surface, handle_subsurface_commit, wd);
    (*watch).destroy_listener = oxide_subsurface_add_destroy(data, handle_subsurface_destroy, wd);
    (*watch).new_subsurface_listener =
        oxide_surface_add_new_subsurface(surface, handle_new_subsurface, userdata);
    (*tl).subsurface_watches.push(watch);
    // Its own children that were added before the listener above existed
    // (wlroots emits new_subsurface once per subsurface, on its parent's
    // first commit after creation — a whole hierarchy built before one root
    // commit announces the deeper levels before this one).
    oxide_surface_for_each_subsurface(surface, handle_new_subsurface, userdata);
}

/// A tracked subsurface committed — on its own, if the client desynchronized
/// it, so the root's commit can't be relied on to cover it. On every commit
/// wlroots also resets that surface's scene buffer: back to the client's own
/// unmasked buffer, at full opacity. Re-apply both; the masking sweep only
/// redoes the buffers that actually need it.
unsafe extern "C" fn handle_subsurface_commit(userdata: *mut c_void, _data: *mut c_void) {
    let watch = userdata as *mut SubsurfaceWatch;
    let tl = (*watch).tl;
    apply_decorations(&*(*tl).server, tl);
}

/// Our commit and new_subsurface listeners must run *after* wlroots' own scene
/// listeners on the same signals: wlroots' commit handler is what puts a
/// surface's fresh client buffer into the scene, and masking has to replace
/// it afterwards — run first, it would just be overwritten (a wl_signal runs
/// its listeners in the order they were added). A new window gets that order
/// for free, since handle_new_toplevel creates the scene tree before adding
/// any listener. Rebuilding the scene tree (`handle_session_active`) adds
/// wlroots' listeners again, behind ours — so call this right after, to move
/// ours back to the end.
pub(crate) unsafe fn reorder_listeners_after_scene_rebuild(tl: *mut Toplevel) {
    let ud = tl as *mut c_void;
    oxide_listener_remove((*tl).commit_listener);
    (*tl).commit_listener = oxide_xdg_add_commit((*tl).xdg_toplevel, handle_commit, ud);
    let root = oxide_xdg_toplevel_surface((*tl).xdg_toplevel);
    oxide_listener_remove((*tl).subsurface_listener);
    (*tl).subsurface_listener = oxide_surface_add_new_subsurface(root, handle_new_subsurface, ud);
    for &watch in &(*tl).subsurface_watches {
        let wd = watch as *mut c_void;
        oxide_listener_remove((*watch).commit_listener);
        (*watch).commit_listener =
            oxide_surface_add_commit((*watch).surface, handle_subsurface_commit, wd);
        oxide_listener_remove((*watch).new_subsurface_listener);
        (*watch).new_subsurface_listener =
            oxide_surface_add_new_subsurface((*watch).surface, handle_new_subsurface, ud);
    }
}

/// A tracked subsurface was destroyed: drop its listeners and free its mask
/// swapchain immediately, rather than leave it for the next full sweep.
unsafe extern "C" fn handle_subsurface_destroy(userdata: *mut c_void, _data: *mut c_void) {
    let watch = userdata as *mut SubsurfaceWatch;
    let tl = (*watch).tl;
    oxide_listener_remove((*watch).commit_listener);
    oxide_listener_remove((*watch).destroy_listener);
    oxide_listener_remove((*watch).new_subsurface_listener);
    if !(*tl).corner_masks.is_null() {
        oxide_corner_mask_pool_forget((*tl).corner_masks, (*watch).surface);
    }
    (*tl).subsurface_watches.retain(|&w| w != watch);
    drop(Box::from_raw(watch));
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
    place_floating_at(server, tl, w, h, ScratchpadPosition::Center);
}

/// `place_floating`, or with `ScratchpadPosition::Top` against the top edge
/// of the usable area instead of its middle (a drop-down).
pub(crate) unsafe fn place_floating_at(
    server: &Server,
    tl: *mut Toplevel,
    w: i32,
    h: i32,
    at: ScratchpadPosition,
) {
    if server.outputs.is_empty() {
        return;
    }
    let o = &server.outputs[active_output(server)];
    // The border goes outside the window, and it has to fit as well.
    let bw = server.config.border_width;
    let (w, h) = (w.min(o.uw - 2 * bw).max(1), h.min(o.uh - 2 * bw).max(1));
    let x = (o.ux + (o.uw - w) / 2).max(o.ux + bw);
    let y = match at {
        ScratchpadPosition::Center => (o.uy + (o.uh - h) / 2).max(o.uy + bw),
        ScratchpadPosition::Top => o.uy + bw,
    };
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
    // Border included: it sits outside the window's own rect.
    let bw = server.config.border_width;
    let (left, top) = (o.ux + bw, o.uy + bw);
    (
        x.clamp(left, (o.ux + o.uw - bw - (*tl).w).max(left)),
        y.clamp(top, (o.uy + o.uh - bw - (*tl).h).max(top)),
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

/// Apply the configured opacity, corner radius and border to `tl`.
///
/// Run on every commit of the window's root surface or any of its
/// subsurfaces, because a new buffer or subsurface has neither opacity nor
/// mask, and a new size moves the border; for every window when a config
/// reload changes any of them — an idle window would otherwise keep the old
/// look until it next redraws; and as a window is shown (`refresh`), since
/// commits made while it was hidden weren't masked. Cheap when nothing
/// changed: masking skips buffers that are already right, and the border
/// only redraws its corners when their size or color changes.
pub(crate) unsafe fn apply_decorations(server: &Server, tl: *mut Toplevel) {
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
            (*tl).xdg_toplevel,
            server.config.corner_radius,
            &mut (*tl).corner_masks,
        );
    }
    apply_border(server, tl);
}

/// Show `tl`'s border at its current size, in the focused or unfocused
/// color — or hide it: no border configured, fullscreen, or unmapped.
pub(crate) unsafe fn apply_border(server: &Server, tl: *mut Toplevel) {
    let cfg = &server.config;
    let width = if (*tl).fullscreen {
        0
    } else {
        cfg.border_width
    };
    if width == 0 && (*tl).border.is_null() {
        return; // never had one, so nothing to hide either
    }
    // Rounded only when the window's own corners are: masking needs the
    // GLES2 program, and a round border around square corners leaves gaps.
    let radius = if server.corner_program.is_null() {
        0
    } else {
        cfg.corner_radius
    };
    let (r, g, b) = if server.focused_tl == tl {
        cfg.border_color_focused
    } else {
        cfg.border_color_unfocused
    };
    oxide_toplevel_update_border(
        (*tl).scene_tree,
        (*tl).xdg_toplevel,
        width,
        radius,
        r,
        g,
        b,
        &mut (*tl).border,
    );
}

/// The seat's keyboard focus moved — by a keybinding, a click, a tap, a
/// workspace switch, a lock screen, a layer surface taking the keyboard;
/// this signal sees all of them. Repaint the borders of the window that lost
/// focus and the one that gained it.
pub(crate) unsafe extern "C" fn handle_keyboard_focus_change(
    userdata: *mut c_void,
    data: *mut c_void,
) {
    let server = &mut *(userdata as *mut Server);
    let surface = oxide_keyboard_focus_change_new_surface(data);
    let now = if surface.is_null() {
        ptr::null_mut()
    } else {
        toplevel_from_surface(server, surface).unwrap_or(ptr::null_mut())
    };
    let before = std::mem::replace(&mut server.focused_tl, now);
    if before == now {
        return;
    }
    for tl in [before, now] {
        if !tl.is_null() {
            apply_border(server, tl);
        }
    }
}

/// The toplevel whose root surface is `surface`, if we track one.
pub(crate) unsafe fn toplevel_from_surface(
    server: &Server,
    surface: *mut c_void,
) -> Option<*mut Toplevel> {
    server
        .workspaces
        .values()
        .flat_map(|ws| ws.windows.iter().copied())
        .find(|&tl| oxide_xdg_toplevel_surface((*tl).xdg_toplevel) == surface)
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
    apply_decorations(server, tl);
    if !oxide_xdg_initial_commit((*tl).xdg_toplevel) {
        return;
    }

    // Floating windows get the opposite treatment: no tiled states, and
    // either a 0,0 configure ("pick your own size" — dialogs and fixed-size
    // windows, whose natural size is the point) or the configured default
    // floating size (`float =` rule windows: ordinary apps told to float).
    // This is why detection happens here and not on map: the very first
    // configure already differs. A scratchpad floats at its own configured
    // size.
    if let Some(spec) = crate::scratchpad::claim(server, tl) {
        (*tl).floating = true;
        let (w, h) = crate::scratchpad::size(server, &spec);
        wlr::wlr_xdg_toplevel_set_size((*tl).xdg_toplevel, w, h);
        println!(
            "0xin: new window — scratchpad {}, initial configure {w}x{h}",
            spec.name
        );
        return;
    }
    if floats_naturally(tl) {
        (*tl).floating = true;
        wlr::wlr_xdg_toplevel_set_size((*tl).xdg_toplevel, 0, 0);
        println!("0xin: new window — floating, initial configure 0x0");
        return;
    }

    let (mut w, mut h) = (0, 0); // 0,0 = client decides (no output to predict from)
    if !server.outputs.is_empty() {
        let o = &server.outputs[active_output(server)];
        let ws = &server.workspaces[o.workspace];
        let tile = predict_tile_rect(ws, o.ux, o.uy, o.uw, o.uh, server.config.gap);
        (_, _, w, h) = inset_for_border(tile, server.config.border_width);
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
    // Claimed again for clients that set their app_id after the initial
    // commit. A scratchpad opens per its spec, and window rules don't apply
    // to it: it is already configured.
    let scratchpad = crate::scratchpad::claim(server, tl);
    if let Some(spec) = &scratchpad {
        server
            .scratchpad_spawning
            .retain(|(name, _)| *name != spec.name);
        (*tl).floating = true;
        oxide_scene_tree_reparent((*tl).scene_tree, server.tree_floating);
        crate::scratchpad::place(server, tl, spec);
    }
    // Backstop for clients that set their dialog parent (or committed their
    // fixed size) after the initial commit — complete by map time.
    if !(*tl).floating && floats_naturally(tl) {
        (*tl).floating = true;
    }
    if (*tl).floating && scratchpad.is_none() {
        // Center it at the natural size the client just committed, above
        // the tiled windows.
        oxide_scene_tree_reparent((*tl).scene_tree, server.tree_floating);
        let (mut w, mut h) = (0, 0);
        oxide_xdg_toplevel_geometry((*tl).xdg_toplevel, &mut w, &mut h);
        place_floating(server, tl, w, h);
    }
    let mut a = active_workspace(server);

    // Ask the config what to do with this window, before it is placed. Ahead
    // of `refresh` on purpose: a rule that sends it to another workspace would
    // otherwise show up as a visible tile-then-move.
    let rule = match scratchpad {
        Some(_) => None,
        None => crate::lua::run_window(server, &window_record(tl, a)),
    };
    if let Some(rule) = rule {
        if let Some(float) = rule.float {
            (*tl).floating = float;
            if float {
                oxide_scene_tree_reparent((*tl).scene_tree, server.tree_floating);
                let (mut w, mut h) = (0, 0);
                oxide_xdg_toplevel_geometry((*tl).xdg_toplevel, &mut w, &mut h);
                place_floating(server, tl, w, h);
            }
        }
        if let Some(ws) = rule.workspace {
            // 1-based in a config, 0-based here. Any number will do: one
            // that does not exist yet is made on the focused output.
            a = (ws - 1).clamp(0, MAX_WORKSPACE as i64 - 1) as usize;
        }
    }

    let here = active_output(server);
    crate::workspaces::ensure(server, a, here);
    if (*tl).floating {
        // Centred on the focused output above; a rule may have sent it to a
        // workspace that belongs to another one.
        crate::workspaces::carry_floating_to(server, tl, here, a);
    }
    server.workspaces[a].windows.push(tl);
    if !(*tl).floating {
        tree_track(&mut server.workspaces[a], tl);
    }
    refresh(server);
    // Only focus it if it actually landed on the workspace we are looking at;
    // a rule may have sent it elsewhere.
    if a == active_workspace(server) {
        focus_index(server, server.workspaces[a].windows.len() - 1);
    }
    println!(
        "0xin: window mapped — ws {} now {} ({})",
        a + 1,
        server.workspaces[a].windows.len(),
        if (*tl).floating { "floating" } else { "tiled" }
    );
    let record = window_record(tl, a);
    crate::events::emit(
        server,
        "openwindow",
        &format!(
            "{} {} {} {}",
            record.workspace,
            if record.floating { "floating" } else { "tiled" },
            record.app_id.as_deref().unwrap_or(""),
            record.title.as_deref().unwrap_or(""),
        ),
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
    // Captured before `remove_window` frees the tracking that identifies it.
    let owner = server
        .workspaces
        .iter()
        .find(|(_, ws)| ws.windows.contains(&tl))
        .map(|(id, _)| id);
    if let Some(id) = owner {
        let record = window_record(tl, id);
        crate::events::emit(
            server,
            "closewindow",
            &format!(
                "{} {}",
                record.app_id.as_deref().unwrap_or(""),
                record.title.as_deref().unwrap_or(""),
            ),
        );
    }
    remove_window(server, tl);
    // Unmapped now, so this hides it: the window's own surfaces hide
    // themselves, but nothing hides the border's tree for us.
    apply_border(server, tl);
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
    oxide_listener_remove((*tl).subsurface_listener);
    // Each tracked subsurface's own 3 listeners, freeing its box — done here
    // rather than relying on each subsurface's own destroy to fire first
    // (wlroots tears down a surface's subsurfaces along with it, in an order
    // this code should not depend on).
    for watch in (*tl).subsurface_watches.drain(..) {
        oxide_listener_remove((*watch).commit_listener);
        oxide_listener_remove((*watch).destroy_listener);
        oxide_listener_remove((*watch).new_subsurface_listener);
        drop(Box::from_raw(watch));
    }
    oxide_corner_mask_pool_destroy((*tl).corner_masks);
    oxide_border_destroy((*tl).border);
    (*tl).border = ptr::null_mut();
    // Before remove_window: the focus it hands on to the next window would
    // otherwise repaint this one's border as it goes. Not done at unmap — a
    // window that unmaps and maps again can keep keyboard focus throughout,
    // and wlroots reports no change for that.
    if server.focused_tl == tl {
        server.focused_tl = ptr::null_mut();
    }
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
    crate::workspaces::detach(server, tl);
    // A hidden scratchpad is on no workspace; it can still go away.
    server.scratchpad_hidden.retain(|&w| w != tl);
    refresh(server);
    if !server.outputs.is_empty() {
        let a = active_workspace(server);
        if !server.workspaces[a].windows.is_empty() {
            let f = server.workspaces[a].focused;
            focus_index(server, f);
        }
    }
}

/// Describe a freshly-mapped window for `oxin.on.window`.
unsafe fn window_record(tl: *mut Toplevel, workspace: usize) -> crate::lua::events::WindowRecord {
    let text = |p: *const std::os::raw::c_char| {
        (!p.is_null()).then(|| CStr::from_ptr(p).to_string_lossy().into_owned())
    };
    crate::lua::events::WindowRecord {
        app_id: text(oxide_xdg_toplevel_app_id((*tl).xdg_toplevel)),
        title: text(oxide_xdg_toplevel_title((*tl).xdg_toplevel)),
        // 1-based, matching how a config numbers workspaces everywhere else.
        workspace: workspace as i64 + 1,
        floating: (*tl).floating,
    }
}
