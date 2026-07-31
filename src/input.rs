//! Input device hotplug, pointer-driven focus policy, and pointer grabs.

use crate::config::{Action, GestureTrigger, MOD_MASK};
use crate::ffi::{
    oxide_focus_toplevel, oxide_handle_new_input, oxide_scene_tree_set_position,
    oxide_xdg_toplevel_surface,
};
use crate::keybindings::{dispatch_action, handle_keybinding};
use crate::state::{GrabMode, Server, Toplevel};
use crate::toplevel::{clamp_floating, set_solo};
use crate::wlr;
use std::os::raw::c_void;
use std::ptr;

// Linux input-event button codes (input-event-codes.h).
const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;

/// Smallest size a resize drag can shrink a floating window to.
const MIN_FLOAT_SIZE: i32 = 50;

/// Turn the recognizer's device-level trigger into the same configured Action
/// keyboard chords use. `GestureTrigger::DoubleTap` is deliberately absent
/// from the table below — this callback's `(userdata, trigger: u32)` shape
/// has no room for the tapped surface, so double-tap fires through the
/// separate `handle_double_tap` callback instead.
pub(crate) unsafe extern "C" fn handle_gesture(userdata: *mut c_void, raw_trigger: u32) {
    let server = &mut *(userdata as *mut Server);
    if server.locked {
        return;
    }
    let trigger = match raw_trigger {
        0 => GestureTrigger::BottomUp,
        1 => GestureTrigger::BottomDown,
        2 => GestureTrigger::EdgeLeftIn,
        3 => GestureTrigger::EdgeRightIn,
        4 => GestureTrigger::TopRight,
        5 => GestureTrigger::TopLeft,
        6 => GestureTrigger::TopDown,
        7 => GestureTrigger::ToTop,
        8 => GestureTrigger::TwoUp,
        9 => GestureTrigger::TwoDown,
        10 => GestureTrigger::TwoLeft,
        11 => GestureTrigger::TwoRight,
        12 => GestureTrigger::ThreeUp,
        13 => GestureTrigger::ThreeDown,
        14 => GestureTrigger::ThreeLeft,
        15 => GestureTrigger::ThreeRight,
        17 => GestureTrigger::EdgeLeftUp,
        18 => GestureTrigger::EdgeLeftDown,
        _ => return,
    };
    let action = server
        .config
        .gestures
        .iter()
        .find(|binding| binding.trigger == trigger)
        .map(|binding| binding.action.clone());
    let Some(action) = action else {
        return;
    };
    dispatch_action(server, action);
}

/// Called by the shim when an input device (keyboard, pointer, …) appears.
pub(crate) unsafe extern "C" fn handle_new_input(userdata: *mut c_void, data: *mut c_void) {
    let server = &mut *(userdata as *mut Server);
    let device = data as *mut wlr::wlr_input_device;
    oxide_handle_new_input(
        server.seat,
        server.cursor,
        device,
        handle_keybinding,
        userdata,
    );
}

/// Called by the shim on every click with the clicked root wlr_surface. The
/// shim already moved seat keyboard focus; this keeps `Workspace.focused` in
/// step so close/movefocus/movewindow act on the clicked window, not the last
/// keyboard-focused one. A click on a non-toplevel surface (bar, wallpaper)
/// matches nothing and changes nothing.
pub(crate) unsafe extern "C" fn handle_click_focus(userdata: *mut c_void, data: *mut c_void) {
    let server = &mut *(userdata as *mut Server);
    if server.locked {
        return;
    }
    for ws in server.workspaces.iter_mut() {
        let hit = ws
            .windows
            .iter()
            .position(|&tl| oxide_xdg_toplevel_surface((*tl).xdg_toplevel) == data);
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

/// Called by the shim when a completed touch double-tap is recognized on a
/// window's root surface. Focuses the tapped window, then applies whatever
/// action is configured for `double-tap`. `Action::ToggleSolo` is resolved
/// directly against the tapped window rather than through `dispatch_action`'s
/// usual active-workspace lookup: that lookup depends on the pointer
/// cursor's last position, which touch never updates, so on a multi-monitor
/// setup it could target the wrong output's focused window — we already
/// have the exact tapped window in hand from the hit test above.
pub(crate) unsafe extern "C" fn handle_double_tap(userdata: *mut c_void, data: *mut c_void) {
    let server = &mut *(userdata as *mut Server);
    if server.locked {
        return;
    }
    let Some(tl) = toplevel_from_surface(server, data) else {
        return;
    };
    let Some(wi) = server.workspaces.iter().position(|ws| ws.windows.contains(&tl)) else {
        return;
    };
    let idx = server.workspaces[wi]
        .windows
        .iter()
        .position(|&w| w == tl)
        .unwrap();
    server.workspaces[wi].focused = idx;
    oxide_focus_toplevel(server.seat, (*tl).xdg_toplevel);

    let action = server
        .config
        .gestures
        .iter()
        .find(|binding| binding.trigger == GestureTrigger::DoubleTap)
        .map(|binding| binding.action.clone());
    let Some(action) = action else {
        return;
    };

    match action {
        Action::ToggleSolo => {
            let want_on = server.workspaces[wi].solo != Some(tl);
            set_solo(server, tl, want_on);
        }
        other => dispatch_action(server, other),
    }
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
    if server.locked {
        return false;
    }

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
    if server.locked {
        return false;
    }
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
