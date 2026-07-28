//! Input device hotplug, pointer-driven focus policy, and pointer grabs.

use crate::config::MOD_MASK;
use crate::ffi::{
    oxide_handle_new_input, oxide_scene_rect_set_position, oxide_scene_tree_set_position,
    oxide_xdg_toplevel_surface,
};
use crate::keybindings::{handle_keybinding, reset_signals, switch_workspace};
use crate::state::{GrabMode, Server, Toplevel};
use crate::tiling::active_workspace;
use crate::toplevel::clamp_floating;
use crate::wlr;
use std::os::raw::c_void;
use std::ptr;
use std::process::Command;

// Linux input-event button codes (input-event-codes.h).
const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;

/// Smallest size a resize drag can shrink a floating window to.
const MIN_FLOAT_SIZE: i32 = 50;

/// Show/hide the configured keyboard. wvkbd deliberately uses SIGUSR2 for
/// show and SIGUSR1 for hide, allowing it to stay connected while invisible.
pub(crate) unsafe extern "C" fn handle_keyboard_gesture(
    userdata: *mut c_void,
    show: bool,
) {
    let server = &mut *(userdata as *mut Server);
    let Some(process) = server.config.gesture_keyboard.as_deref() else {
        return;
    };
    let signal = if show { "-USR2" } else { "-USR1" };
    for output in &server.outputs {
        if !output.gesture_handle.is_null() {
            let y = output.y + output.h
                - if show {
                    server.config.gesture_keyboard_height + 8
                } else {
                    10
                };
            oxide_scene_rect_set_position(
                output.gesture_handle,
                output.x + (output.w - 120) / 2,
                y,
            );
        }
    }
    let mut command = Command::new("pkill");
    command.args([signal, "-x", process]);
    reset_signals(&mut command);
    match command.spawn() {
        Ok(_) => eprintln!(
            "0xin: keyboard gesture — {} `{process}`",
            if show { "show" } else { "hide" }
        ),
        Err(e) => eprintln!("0xin: keyboard gesture failed: {e}"),
    }
}

/// Cycle through the workspace ring after an inward side-edge swipe.
pub(crate) unsafe extern "C" fn handle_workspace_gesture(
    userdata: *mut c_void,
    delta: i32,
) {
    let server = &mut *(userdata as *mut Server);
    if server.outputs.is_empty() {
        return;
    }
    let current = active_workspace(server);
    let count = server.workspaces.len() as i32;
    let target = (current as i32 + delta).rem_euclid(count) as usize;
    switch_workspace(server, target);
}

/// Called by the shim when an input device (keyboard, pointer, …) appears.
pub(crate) unsafe extern "C" fn handle_new_input(userdata: *mut c_void, data: *mut c_void) {
    let server = &mut *(userdata as *mut Server);
    let device = data as *mut wlr::wlr_input_device;
    oxide_handle_new_input(server.seat, server.cursor, device, handle_keybinding, userdata);
}

/// Called by the shim on every click with the clicked root wlr_surface. The
/// shim already moved seat keyboard focus; this keeps `Workspace.focused` in
/// step so close/movefocus/movewindow act on the clicked window, not the last
/// keyboard-focused one. A click on a non-toplevel surface (bar, wallpaper)
/// matches nothing and changes nothing.
pub(crate) unsafe extern "C" fn handle_click_focus(userdata: *mut c_void, data: *mut c_void) {
    let server = &mut *(userdata as *mut Server);
    for ws in server.workspaces.iter_mut() {
        let hit = ws.windows.iter().position(|&tl| oxide_xdg_toplevel_surface((*tl).xdg_toplevel) == data);
        if let Some(idx) = hit {
            ws.focused = idx;
            return;
        }
    }
}

/// The toplevel whose root surface is `surface`, if we track one.
unsafe fn toplevel_from_surface(server: &Server, surface: *mut c_void) -> Option<*mut Toplevel> {
    for ws in &server.workspaces {
        for &tl in &ws.windows {
            if oxide_xdg_toplevel_surface((*tl).xdg_toplevel) == surface {
                return Some(tl);
            }
        }
    }
    None
}

/// Called by the shim for every pointer button. Returning true consumes the
/// event. A press with the primary modifier held on a floating window starts
/// a grab (left button moves, right resizes); any release ends an active
/// grab — and is swallowed with it, since the client never saw the press.
pub(crate) unsafe extern "C" fn handle_grab_button(
    userdata: *mut c_void,
    root_surface: *mut c_void,
    button: u32,
    modifiers: u32,
    pressed: bool,
    cx: f64,
    cy: f64,
) -> bool {
    let server = &mut *(userdata as *mut Server);

    if !pressed {
        if server.grab == GrabMode::None {
            return false;
        }
        server.grab = GrabMode::None;
        server.grab_tl = ptr::null_mut();
        return true;
    }

    if modifiers & MOD_MASK != server.config.modifier || root_surface.is_null() {
        return false;
    }
    let mode = match button {
        BTN_LEFT => GrabMode::Move,
        BTN_RIGHT => GrabMode::Resize,
        _ => return false,
    };
    let Some(tl) = toplevel_from_surface(server, root_surface) else {
        return false;
    };
    if !(*tl).floating || (*tl).fullscreen {
        return false;
    }
    server.grab = mode;
    server.grab_tl = tl;
    (server.grab_cx, server.grab_cy) = (cx, cy);
    (server.grab_x, server.grab_y, server.grab_w, server.grab_h) =
        ((*tl).x, (*tl).y, (*tl).w, (*tl).h);
    true
}

/// Called by the shim for every cursor motion, before any client processing.
/// Returning true means a grab is active: the grabbed window followed the
/// cursor and no client should see enter/motion.
pub(crate) unsafe extern "C" fn handle_grab_motion(
    userdata: *mut c_void,
    cx: f64,
    cy: f64,
) -> bool {
    let server = &mut *(userdata as *mut Server);
    let tl = server.grab_tl;
    let (dx, dy) = ((cx - server.grab_cx) as i32, (cy - server.grab_cy) as i32);
    match server.grab {
        GrabMode::None => false,
        GrabMode::Move => {
            let (x, y) = clamp_floating(server, tl, server.grab_x + dx, server.grab_y + dy);
            oxide_scene_tree_set_position((*tl).scene_tree, x, y);
            ((*tl).x, (*tl).y) = (x, y);
            true
        }
        GrabMode::Resize => {
            // Bottom-right-corner semantics: position stays, size follows.
            // The size is a floating-semantics hint, but the clients that
            // matter honor it.
            let w = (server.grab_w + dx).max(MIN_FLOAT_SIZE);
            let h = (server.grab_h + dy).max(MIN_FLOAT_SIZE);
            wlr::wlr_xdg_toplevel_set_size((*tl).xdg_toplevel, w, h);
            ((*tl).w, (*tl).h) = (w, h);
            true
        }
    }
}
