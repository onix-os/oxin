//! Keybinding dispatch: VT switching, hold bindings, and the config's bind
//! table. Input plumbing lives in `input.rs`; this is what an action *does*.

use std::process::Command;
use std::sync::atomic::Ordering;
use std::time::Duration;

use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::utils::SERIAL_COUNTER;

use crate::config::{Action, Direction, MOD_ALT, MOD_CTRL, MOD_MASK};
use crate::layout::tree_resize;
use crate::state::Oxin;
use crate::tiling::{
    active_output, active_workspace, place, refresh, spatial_neighbor, tiled_position, tree_track,
    tree_untrack,
};
use crate::toplevel::{clamp_floating, set_floating, set_fullscreen, set_solo};
use crate::window;

// Function-key keysyms (contiguous): F1 = 0xffbe … F12 = 0xffc9.
const KEY_F1: u32 = 0xffbe;
const KEY_F12: u32 = 0xffc9;

/// How far one Mod+Shift+hjkl press moves a floating window, in pixels.
const NUDGE_STEP: i32 = 50;

/// How far one Mod+Ctrl+hjkl press adjusts a tiled window's split ratio.
const RESIZE_STEP: f32 = 0.05;

/// Give keyboard focus to window `idx` (wrapped) of the active output's
/// workspace.
pub fn focus_index(state: &mut Oxin, idx: usize) {
    if state.outputs.is_empty() {
        return;
    }
    let workspace = active_workspace(state);
    let len = state.workspaces[workspace].windows.len();
    if len == 0 {
        return;
    }
    let index = idx % len;
    state.workspaces[workspace].focused = index;
    let window = state.workspaces[workspace].windows[index].clone();
    focus_window(state, &window);
}

/// Give keyboard focus to one window: raise it in the space, mark it activated
/// (and every other window deactivated) and point the seat's keyboard at it.
pub fn focus_window(state: &mut Oxin, window: &smithay::desktop::Window) {
    state.space.raise_element(window, true);

    for ws in &state.workspaces {
        for other in &ws.windows {
            let activated = other == window;
            if let Some(toplevel) = other.toplevel() {
                let changed = toplevel.with_pending_state(|pending| {
                    let had = pending.states.contains(xdg_toplevel::State::Activated);
                    if activated {
                        pending.states.set(xdg_toplevel::State::Activated);
                    } else {
                        pending.states.unset(xdg_toplevel::State::Activated);
                    }
                    had != activated
                });
                if changed {
                    toplevel.send_pending_configure();
                }
            }
        }
    }

    let Some(surface) = window
        .toplevel()
        .map(|toplevel| toplevel.wl_surface().clone())
    else {
        return;
    };
    if let Some(keyboard) = state.seat.get_keyboard() {
        let serial = SERIAL_COUNTER.next_serial();
        keyboard.set_focus(state, Some(surface), serial);
    }
}

/// Ask the focused window of the active output's workspace to close.
fn close_focused(state: &Oxin) {
    if state.outputs.is_empty() {
        return;
    }
    let ws = &state.workspaces[active_workspace(state)];
    if let Some(window) = ws.windows.get(ws.focused) {
        if let Some(toplevel) = window.toplevel() {
            toplevel.send_close();
        }
    }
}

/// Display `target` on the active output. If it's already shown on another
/// output, swap the two outputs' workspaces (so no workspace is on two
/// monitors).
pub fn switch_workspace(state: &mut Oxin, target: usize) {
    if state.outputs.is_empty() || target >= state.workspaces.len() {
        return;
    }
    let output = active_output(state);
    let current = state.outputs[output].workspace;
    if target == current {
        return;
    }
    if let Some(other) = state.outputs.iter().position(|o| o.workspace == target) {
        state.outputs[other].workspace = current; // swap: that monitor takes ours
    }
    state.outputs[output].workspace = target;
    refresh(state);
    let focused = state.workspaces[target].focused;
    focus_index(state, focused);
    eprintln!("0xin: output {} -> workspace {}", output, target + 1);
}

/// Move the active output's focused window to another workspace.
fn move_to_workspace(state: &mut Oxin, target: usize) {
    if state.outputs.is_empty() || target >= state.workspaces.len() {
        return;
    }
    let current = active_workspace(state);
    if target == current || state.workspaces[current].windows.is_empty() {
        return;
    }
    let focused = state.workspaces[current].focused;
    let window = state.workspaces[current].windows[focused].clone();
    let tiled = window::is_tiled(&window);
    if tiled {
        tree_untrack(&mut state.workspaces[current], &window);
    }
    if state.workspaces[current].solo.as_ref() == Some(&window) {
        state.workspaces[current].solo = None;
    }
    state.workspaces[current].windows.remove(focused);
    let len = state.workspaces[current].windows.len();
    if state.workspaces[current].focused >= len && len > 0 {
        state.workspaces[current].focused = len - 1;
    }
    state.workspaces[target].windows.push(window.clone());
    if tiled {
        tree_track(&mut state.workspaces[target], &window);
    }
    refresh(state); // recomputes visibility (target may or may not be displayed)
    let focused = state.workspaces[current].focused;
    focus_index(state, focused);
    eprintln!("0xin: moved window to workspace {}", target + 1);
}

/// Move a floating window one step in `dir`, kept within the usable area of
/// its output (the same clamp pointer-grab moves use).
fn nudge_floating(state: &mut Oxin, window: &smithay::desktop::Window, dir: Direction) {
    let current = window::rect(window);
    let (mut x, mut y) = (current.loc.x, current.loc.y);
    match dir {
        Direction::Left => x -= NUDGE_STEP,
        Direction::Right => x += NUDGE_STEP,
        Direction::Up => y -= NUDGE_STEP,
        Direction::Down => y += NUDGE_STEP,
    }
    let (x, y) = clamp_floating(state, window, x, y);
    place(
        state,
        window,
        crate::tiling::rect(x, y, current.size.w, current.size.h),
    );
}

/// Launch a program as a client of 0xin (inherits our WAYLAND_DISPLAY). Runs
/// through a shell (like Hyprland's `exec`) so `~`, env vars, `&&`, and quoting
/// in bind commands work as expected — a plain `execvp` doesn't expand any of
/// that.
pub fn spawn(cmd: &str) {
    let mut command = Command::new("sh");
    command.arg("-c").arg(cmd);
    reset_signals(&mut command);
    if let Err(error) = command.spawn() {
        eprintln!("0xin: failed to spawn `{cmd}`: {error}");
    }
}

/// Arrange for a spawned client to start with clean process state. The
/// compositor's ignored SIGCHLD, blocked SIGINT/SIGTERM, and private
/// `LD_LIBRARY_PATH` would otherwise leak into clients. The FP5 sysroot's
/// libraries are compositor dependencies and make Firefox crash when they
/// override its system libraries. Every spawn path must go through this.
pub fn reset_signals(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    command.env_remove("LD_LIBRARY_PATH");
    unsafe {
        command.pre_exec(|| {
            // Undo the compositor's own signal setup: an empty signal mask and
            // default dispositions, so clients are not born with SIGCHLD
            // ignored or SIGINT/SIGTERM blocked.
            let mut mask: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut mask);
            libc::sigprocmask(libc::SIG_SETMASK, &mask, std::ptr::null_mut());
            libc::signal(libc::SIGCHLD, libc::SIG_DFL);
            libc::signal(libc::SIGINT, libc::SIG_DFL);
            libc::signal(libc::SIGTERM, libc::SIG_DFL);
            libc::signal(libc::SIGPIPE, libc::SIG_DFL);
            Ok(())
        });
    }
}

fn set_keyboard_visible(state: &mut Oxin, visible: bool) {
    if state.keyboard_visible == visible {
        return;
    }
    let command = if visible {
        state.config.virtual_keyboard_show.clone()
    } else {
        state.config.virtual_keyboard_hide.clone()
    };
    let Some(command) = command else {
        eprintln!(
            "0xin: no virtual_keyboard_{} command configured",
            if visible { "show" } else { "hide" }
        );
        return;
    };
    spawn(&command);
    state.keyboard_visible = visible;
    state.gestures.set_keyboard_visible(visible);
}

/// The keyboard produced a chord. Returns true to consume it (the focused
/// client never sees it); false forwards it on.
pub fn handle_keybinding(state: &mut Oxin, keysym: u32, modifiers: u32, pressed: bool) -> bool {
    let mods = modifiers & MOD_MASK;

    // The lock client owns all physical and virtual keyboard input. Returning
    // false forwards the event through the seat instead of executing policy.
    if state.locked {
        return false;
    }

    if state.held_keysym == keysym && state.held_modifiers == mods {
        if !pressed {
            let short_action = if state.held_action.is_some() {
                state
                    .config
                    .binds
                    .iter()
                    .find(|binding| binding.mods == mods && binding.keysym == keysym)
                    .map(|binding| binding.action.clone())
            } else {
                None
            };
            cancel_hold(state);
            if let Some(action) = short_action {
                dispatch_action(state, action);
            }
        }
        return true;
    }

    if let Some(binding) = state
        .config
        .hold_binds
        .iter()
        .find(|binding| binding.mods == mods && binding.keysym == keysym)
        .cloned()
    {
        if pressed {
            cancel_hold(state);
            state.held_keysym = keysym;
            state.held_modifiers = mods;
            state.held_action = Some(binding.action);
            let timer = Timer::from_duration(Duration::from_millis(binding.duration_ms as u64));
            match state.loop_handle.insert_source(timer, |_, _, state| {
                state.hold_timer = None;
                if let Some(action) = state.held_action.take() {
                    dispatch_action(state, action);
                }
                TimeoutAction::Drop
            }) {
                Ok(token) => state.hold_timer = Some(token),
                Err(error) => {
                    eprintln!("0xin: failed to arm hold binding timer: {error}");
                    cancel_hold(state);
                }
            }
        }
        return true;
    }

    // Ordinary bindings act only on press, but consume their release too so a
    // client never receives a release for a press handled by the compositor.
    let is_regular_bind = state
        .config
        .binds
        .iter()
        .any(|binding| binding.mods == mods && binding.keysym == keysym);
    if !pressed {
        return is_regular_bind;
    }

    // VT switching (Ctrl+Alt+F1..F12). Handled before config binds and always
    // consumed; a no-op when there's no session (nested).
    if mods == MOD_CTRL | MOD_ALT && (KEY_F1..=KEY_F12).contains(&keysym) {
        let vt = (keysym - KEY_F1 + 1) as i32;
        if let Some(backend) = state.backend.as_mut() {
            backend.change_vt(vt);
        }
        return true;
    }

    // Find the matching bind, then act. We clone the action first so the
    // immutable borrow of `state.config` ends before we mutate `state`.
    let action = state
        .config
        .binds
        .iter()
        .find(|binding| binding.mods == mods && binding.keysym == keysym)
        .map(|binding| binding.action.clone());
    let Some(action) = action else { return false };

    dispatch_action(state, action);
    true
}

fn cancel_hold(state: &mut Oxin) {
    if let Some(token) = state.hold_timer.take() {
        state.loop_handle.remove(token);
    }
    state.held_keysym = 0;
    state.held_modifiers = 0;
    state.held_action = None;
}

/// Execute compositor policy independently of which input produced it.
pub fn dispatch_action(state: &mut Oxin, action: Action) {
    // Window count on the active output's workspace (0 if no output yet).
    let n = if state.outputs.is_empty() {
        0
    } else {
        state.workspaces[active_workspace(state)].windows.len()
    };
    match action {
        Action::Spawn(cmd) => spawn(&cmd),
        Action::Close => close_focused(state),
        Action::Quit => state.running.store(false, Ordering::SeqCst),
        Action::FocusNext if n > 0 => {
            let focused = state.workspaces[active_workspace(state)].focused;
            focus_index(state, focused + 1);
        }
        Action::FocusPrev if n > 0 => {
            let focused = state.workspaces[active_workspace(state)].focused;
            focus_index(state, focused + n - 1);
        }
        Action::FocusNext | Action::FocusPrev => {}
        Action::MoveFocus(dir) if n > 0 => {
            let workspace = active_workspace(state);
            let focused = state.workspaces[workspace].focused;
            if let Some(index) = spatial_neighbor(state, workspace, focused, dir) {
                focus_index(state, index);
            }
        }
        Action::MoveWindow(dir) if n > 0 => {
            let workspace = active_workspace(state);
            let focused = state.workspaces[workspace].focused;
            let window = state.workspaces[workspace].windows[focused].clone();
            if window::is_floating(&window) && !window::is_fullscreen(&window) {
                // A floating window has no tiling position to swap; nudge it
                // instead.
                nudge_floating(state, &window, dir);
            } else if let Some(index) = spatial_neighbor(state, workspace, focused, dir) {
                state.workspaces[workspace].windows.swap(focused, index);
                state.workspaces[workspace].focused = index;
                refresh(state);
            }
        }
        Action::ResizeWindow(dir) if n > 0 => {
            let workspace = active_workspace(state);
            let focused = state.workspaces[workspace].focused;
            if let Some(window) = state.workspaces[workspace].windows.get(focused).cloned() {
                if let Some(index) = tiled_position(&state.workspaces[workspace], &window) {
                    if let Some(tree) = &mut state.workspaces[workspace].tree {
                        tree_resize(tree, index, dir, RESIZE_STEP);
                    }
                    refresh(state);
                }
            }
        }
        Action::MoveFocus(_) | Action::MoveWindow(_) | Action::ResizeWindow(_) => {}
        Action::Fullscreen if n > 0 => {
            let workspace = active_workspace(state);
            let focused = state.workspaces[workspace].focused;
            if let Some(window) = state.workspaces[workspace].windows.get(focused).cloned() {
                let on = !window::is_fullscreen(&window);
                set_fullscreen(state, &window, on);
            }
        }
        Action::Fullscreen => {}
        Action::ToggleSolo if n > 0 => {
            let workspace = active_workspace(state);
            let focused = state.workspaces[workspace].focused;
            if let Some(window) = state.workspaces[workspace].windows.get(focused).cloned() {
                let on = state.workspaces[workspace].solo.as_ref() != Some(&window);
                set_solo(state, &window, on);
            }
        }
        Action::ToggleSolo => {}
        Action::ToggleFloating if n > 0 => {
            let workspace = active_workspace(state);
            let focused = state.workspaces[workspace].focused;
            if let Some(window) = state.workspaces[workspace].windows.get(focused).cloned() {
                let on = !window::is_floating(&window);
                set_floating(state, &window, on);
            }
        }
        Action::ToggleFloating => {}
        Action::Workspace(workspace) => switch_workspace(state, workspace),
        Action::MoveToWorkspace(workspace) => move_to_workspace(state, workspace),
        Action::MoveToWorkspaceNext if n > 0 => {
            let current = active_workspace(state);
            move_to_workspace(state, (current + 1) % state.workspaces.len());
        }
        Action::MoveToWorkspacePrevious if n > 0 => {
            let current = active_workspace(state);
            move_to_workspace(
                state,
                (current + state.workspaces.len() - 1) % state.workspaces.len(),
            );
        }
        Action::MoveToWorkspaceNext | Action::MoveToWorkspacePrevious => {}
        Action::WorkspaceNext if !state.outputs.is_empty() => {
            let current = active_workspace(state);
            let count = state.workspaces.len() as i32;
            switch_workspace(state, (current as i32 + 1).rem_euclid(count) as usize);
        }
        Action::WorkspacePrevious if !state.outputs.is_empty() => {
            let current = active_workspace(state);
            let count = state.workspaces.len() as i32;
            switch_workspace(state, (current as i32 - 1).rem_euclid(count) as usize);
        }
        Action::WorkspaceNext | Action::WorkspacePrevious => {}
        Action::KeyboardShow => set_keyboard_visible(state, true),
        Action::KeyboardHide => set_keyboard_visible(state, false),
        Action::KeyboardToggle => {
            let visible = state.keyboard_visible;
            set_keyboard_visible(state, !visible)
        }
    }
}
