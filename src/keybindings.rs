//! Keybinding dispatch: VT switching, and the config's bind table.

use crate::config::{Action, Direction, MOD_ALT, MOD_CTRL, MOD_MASK};
use crate::ffi::*;
use crate::layout::tree_resize;
use crate::state::*;
use crate::tiling::{
    active_output, active_workspace, refresh, spatial_neighbor, tiled_position, tree_track,
    tree_untrack,
};
use crate::toplevel::{clamp_floating, set_floating, set_fullscreen};
use crate::wlr;
use std::os::raw::c_void;
use std::process::Command;

// Function-key keysyms (contiguous): F1 = 0xffbe … F12 = 0xffc9.
const KEY_F1: u32 = 0xffbe;
const KEY_F12: u32 = 0xffc9;

/// Give keyboard focus to window `idx` (wrapped) of the focused output's
/// workspace.
pub(crate) unsafe fn focus_index(server: &mut Server, idx: usize) {
    if server.outputs.is_empty() {
        return;
    }
    let a = active_workspace(server);
    let len = server.workspaces[a].windows.len();
    if len == 0 {
        return;
    }
    let i = idx % len;
    server.workspaces[a].focused = i;
    oxide_focus_toplevel(server.seat, (*server.workspaces[a].windows[i]).xdg_toplevel);
}

/// Ask the focused window of the focused output's workspace to close.
unsafe fn close_focused(server: &Server) {
    if server.outputs.is_empty() {
        return;
    }
    let ws = &server.workspaces[active_workspace(server)];
    if let Some(&tl) = ws.windows.get(ws.focused) {
        wlr::wlr_xdg_toplevel_send_close((*tl).xdg_toplevel);
    }
}

/// Display `target` on the focused output. If it's already shown on another
/// output, swap the two outputs' workspaces (so no workspace is on two monitors).
pub(crate) unsafe fn switch_workspace(server: &mut Server, target: usize) {
    if server.outputs.is_empty() || target >= server.workspaces.len() {
        return;
    }
    let fo = active_output(server);
    let current = server.outputs[fo].workspace;
    if target == current {
        return;
    }
    if let Some(other) = server.outputs.iter().position(|o| o.workspace == target) {
        server.outputs[other].workspace = current; // swap: that monitor takes ours
    }
    server.outputs[fo].workspace = target;
    refresh(server);
    let f = server.workspaces[target].focused;
    focus_index(server, f);
    eprintln!("0xin: output {} -> workspace {}", fo, target + 1);
}

/// Move the focused output's focused window to another workspace.
unsafe fn move_to_workspace(server: &mut Server, target: usize) {
    if server.outputs.is_empty() || target >= server.workspaces.len() {
        return;
    }
    let a = active_workspace(server);
    if target == a || server.workspaces[a].windows.is_empty() {
        return;
    }
    let focused = server.workspaces[a].focused;
    let tl = server.workspaces[a].windows[focused];
    let tiled = !(*tl).floating && !(*tl).fullscreen;
    if tiled {
        tree_untrack(&mut server.workspaces[a], tl);
    }
    server.workspaces[a].windows.remove(focused);
    let len = server.workspaces[a].windows.len();
    if server.workspaces[a].focused >= len && len > 0 {
        server.workspaces[a].focused = len - 1;
    }
    server.workspaces[target].windows.push(tl);
    if tiled {
        tree_track(&mut server.workspaces[target], tl);
    }
    refresh(server); // recomputes visibility (target may or may not be displayed)
    let f = server.workspaces[a].focused;
    focus_index(server, f);
    eprintln!("0xin: moved window to workspace {}", target + 1);
}

/// How far one Mod+Shift+hjkl press moves a floating window, in pixels.
const NUDGE_STEP: i32 = 50;

/// How far one Mod+Ctrl+hjkl press adjusts a tiled window's split ratio.
const RESIZE_STEP: f32 = 0.05;

/// Move a floating window one step in `dir`, kept within the usable area of
/// its output (the same clamp pointer-grab moves use).
unsafe fn nudge_floating(server: &mut Server, tl: *mut Toplevel, dir: Direction) {
    let (mut x, mut y) = ((*tl).x, (*tl).y);
    match dir {
        Direction::Left => x -= NUDGE_STEP,
        Direction::Right => x += NUDGE_STEP,
        Direction::Up => y -= NUDGE_STEP,
        Direction::Down => y += NUDGE_STEP,
    }
    let (x, y) = clamp_floating(server, tl, x, y);
    oxide_scene_tree_set_position((*tl).scene_tree, x, y);
    ((*tl).x, (*tl).y) = (x, y);
}

/// Launch a program as a client of 0xin (inherits our WAYLAND_DISPLAY). Runs
/// through a shell (like Hyprland's `exec`) so `~`, env vars, `&&`, and quoting
/// in bind commands work as expected — a plain `execvp` doesn't expand any of
/// that.
pub(crate) fn spawn(cmd: &str) {
    let mut command = Command::new("sh");
    command.arg("-c").arg(cmd);
    reset_signals(&mut command);
    if let Err(e) = command.spawn() {
        eprintln!("0xin: failed to spawn `{cmd}`: {e}");
    }
}

unsafe fn set_keyboard_visible(server: &mut Server, visible: bool) {
    if server.keyboard_visible == visible {
        return;
    }
    let command = if visible {
        server.config.virtual_keyboard_show.clone()
    } else {
        server.config.virtual_keyboard_hide.clone()
    };
    let Some(command) = command else {
        eprintln!(
            "0xin: no virtual_keyboard_{} command configured",
            if visible { "show" } else { "hide" }
        );
        return;
    };
    spawn(&command);
    server.keyboard_visible = visible;
    oxide_cursor_set_keyboard_visible(server.cursor, visible);
    for output in &server.outputs {
        if !output.gesture_handle.is_null() {
            let y = output.y + output.h
                - if visible {
                    server.config.virtual_keyboard_height + 8
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
}

/// Arrange for a spawned client to start with clean process state. The
/// compositor's ignored SIGCHLD, blocked SIGINT/SIGTERM, and private
/// `LD_LIBRARY_PATH` would otherwise leak into clients. The FP5 sysroot's
/// libraries are compositor dependencies and make Firefox crash when they
/// override its system libraries. Every spawn path must go through this.
pub(crate) fn reset_signals(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    command.env_remove("LD_LIBRARY_PATH");
    unsafe {
        command.pre_exec(|| {
            oxide_reset_child_signals();
            Ok(())
        });
    }
}

/// Called by the shim for each key press; returns true to consume the key.
/// We look the (modifiers, keysym) up in the config's bind table; an unmatched
/// chord falls through to the focused app.
pub(crate) unsafe extern "C" fn handle_keybinding(
    userdata: *mut c_void,
    keysym: u32,
    modifiers: u32,
    pressed: bool,
) -> bool {
    let server = &mut *(userdata as *mut Server);
    let mods = modifiers & MOD_MASK;

    if server.held_keysym == keysym && server.held_modifiers == mods {
        if !pressed {
            cancel_hold(server);
        }
        return true;
    }

    if let Some(binding) = server
        .config
        .hold_binds
        .iter()
        .find(|binding| binding.mods == mods && binding.keysym == keysym)
        .cloned()
    {
        if pressed {
            cancel_hold(server);
            server.held_keysym = keysym;
            server.held_modifiers = mods;
            server.held_action = Some(binding.action);
            server.hold_source = oxide_event_loop_add_timer(
                server.event_loop,
                binding.duration_ms,
                handle_hold_timer,
                userdata,
            );
            if server.hold_source.is_null() {
                eprintln!("0xin: failed to arm hold binding timer");
                cancel_hold(server);
            }
        }
        return true;
    }

    // Ordinary bindings act only on press, but consume their release too so a
    // client never receives a release for a press handled by the compositor.
    let is_regular_bind = server
        .config
        .binds
        .iter()
        .any(|binding| binding.mods == mods && binding.keysym == keysym);
    if !pressed {
        return is_regular_bind;
    }

    // VT switching (Ctrl+Alt+F1..F12). Handled before config binds and always
    // consumed; the shim no-ops it when there's no session (nested).
    if mods == MOD_CTRL | MOD_ALT && (KEY_F1..=KEY_F12).contains(&keysym) {
        oxide_session_change_vt(server.session, keysym - KEY_F1 + 1);
        return true;
    }

    // Find the matching bind, then act. We clone the action first so the
    // immutable borrow of `server.config` ends before we mutate `server`.
    let action = server
        .config
        .binds
        .iter()
        .find(|b| b.mods == mods && b.keysym == keysym)
        .map(|b| b.action.clone());
    let Some(action) = action else { return false };

    dispatch_action(server, action);
    true
}

unsafe extern "C" fn handle_hold_timer(userdata: *mut c_void, source: *mut c_void) {
    let server = &mut *(userdata as *mut Server);
    if server.hold_source != source {
        return;
    }
    server.hold_source = std::ptr::null_mut();
    oxide_event_source_remove(source);
    if let Some(action) = server.held_action.take() {
        dispatch_action(server, action);
    }
}

unsafe fn cancel_hold(server: &mut Server) {
    if !server.hold_source.is_null() {
        oxide_event_source_remove(server.hold_source);
        server.hold_source = std::ptr::null_mut();
    }
    server.held_keysym = 0;
    server.held_modifiers = 0;
    server.held_action = None;
}

/// Execute compositor policy independently of which input produced it.
pub(crate) unsafe fn dispatch_action(server: &mut Server, action: Action) {
    // Window count on the focused output's workspace (0 if no output yet).
    let n = if server.outputs.is_empty() {
        0
    } else {
        server.workspaces[active_workspace(server)].windows.len()
    };
    match action {
        Action::Spawn(cmd) => spawn(&cmd),
        Action::Close => close_focused(server),
        Action::Quit => wlr::wl_display_terminate(server.display),
        Action::FocusNext if n > 0 => {
            let f = server.workspaces[active_workspace(server)].focused;
            focus_index(server, f + 1);
        }
        Action::FocusPrev if n > 0 => {
            let f = server.workspaces[active_workspace(server)].focused;
            focus_index(server, f + n - 1);
        }
        Action::FocusNext | Action::FocusPrev => {}
        Action::MoveFocus(dir) if n > 0 => {
            let a = active_workspace(server);
            let f = server.workspaces[a].focused;
            if let Some(i) = spatial_neighbor(server, a, f, dir) {
                focus_index(server, i);
            }
        }
        Action::MoveWindow(dir) if n > 0 => {
            let a = active_workspace(server);
            let f = server.workspaces[a].focused;
            let tl = server.workspaces[a].windows[f];
            if (*tl).floating && !(*tl).fullscreen {
                // A floating window has no tiling position to swap; nudge it
                // instead (keyboard-only move until interactive drag lands).
                nudge_floating(server, tl, dir);
            } else if let Some(i) = spatial_neighbor(server, a, f, dir) {
                server.workspaces[a].windows.swap(f, i);
                server.workspaces[a].focused = i;
                refresh(server);
            }
        }
        Action::ResizeWindow(dir) if n > 0 => {
            let a = active_workspace(server);
            let f = server.workspaces[a].focused;
            if let Some(&tl) = server.workspaces[a].windows.get(f) {
                if let Some(i) = tiled_position(&server.workspaces[a], tl) {
                    if let Some(tree) = &mut server.workspaces[a].tree {
                        tree_resize(tree, i, dir, RESIZE_STEP);
                    }
                    refresh(server);
                }
            }
        }
        Action::MoveFocus(_) | Action::MoveWindow(_) | Action::ResizeWindow(_) => {}
        Action::Fullscreen if n > 0 => {
            let a = active_workspace(server);
            let ws = &server.workspaces[a];
            if let Some(&tl) = ws.windows.get(ws.focused) {
                set_fullscreen(server, tl, !(*tl).fullscreen);
            }
        }
        Action::Fullscreen => {}
        Action::ToggleFloating if n > 0 => {
            let a = active_workspace(server);
            let ws = &server.workspaces[a];
            if let Some(&tl) = ws.windows.get(ws.focused) {
                set_floating(server, tl, !(*tl).floating);
            }
        }
        Action::ToggleFloating => {}
        Action::Workspace(ws) => switch_workspace(server, ws),
        Action::MoveToWorkspace(ws) => move_to_workspace(server, ws),
        Action::MoveToWorkspaceNext if n > 0 => {
            let current = active_workspace(server);
            move_to_workspace(server, (current + 1) % server.workspaces.len());
        }
        Action::MoveToWorkspacePrevious if n > 0 => {
            let current = active_workspace(server);
            move_to_workspace(
                server,
                (current + server.workspaces.len() - 1) % server.workspaces.len(),
            );
        }
        Action::MoveToWorkspaceNext | Action::MoveToWorkspacePrevious => {}
        Action::WorkspaceNext if !server.outputs.is_empty() => {
            let current = active_workspace(server);
            let count = server.workspaces.len() as i32;
            switch_workspace(server, (current as i32 + 1).rem_euclid(count) as usize);
        }
        Action::WorkspacePrevious if !server.outputs.is_empty() => {
            let current = active_workspace(server);
            let count = server.workspaces.len() as i32;
            switch_workspace(server, (current as i32 - 1).rem_euclid(count) as usize);
        }
        Action::WorkspaceNext | Action::WorkspacePrevious => {}
        Action::KeyboardShow => set_keyboard_visible(server, true),
        Action::KeyboardHide => set_keyboard_visible(server, false),
        Action::KeyboardToggle => set_keyboard_visible(server, !server.keyboard_visible),
    }
}
