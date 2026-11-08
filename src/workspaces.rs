//! Workspaces the Hyprland way (Stage 32).
//!
//! - **Made on demand, dropped when unused.** A workspace exists while it has
//!   windows or is on screen. Asking for one that does not exist makes it, on
//!   the focused output; one that is empty and off screen is dropped
//!   (`prune`).
//! - **Each belongs to one output.** Switching to a workspace that belongs to
//!   another output shows it *there* and moves focus — and the cursor, which
//!   is what decides the focused output — to that output. Nothing is pulled
//!   across or swapped.
//! - **Outputs come and go without losing anything.** A new output gets the
//!   lowest free number. An unplugged one hands its workspaces, hidden, to the
//!   first remaining output and gets them back, showing the same one, when
//!   the same connector is plugged in again.
//!
//! The decisions follow Hyprland's own (`LifecyclePolicy.cpp`,
//! `PlacementController.cpp` and `ConfigActions.cpp` in its tree), so a
//! habit formed in one carries over to the other.
//!
//! Workspace numbers are 0-based in here and 1-based wherever a user sees one.

use crate::ffi::{oxide_cursor_warp, oxide_focus_toplevel, oxide_seat_clear_keyboard_focus};
use crate::state::{Server, Toplevel, Workspace, MAX_WORKSPACE};
use crate::tiling::{active_output, refresh, tree_track, tree_untrack};

/// The output workspace `id` belongs to, if it exists and that output is
/// connected.
pub(crate) fn owner(server: &Server, id: usize) -> Option<usize> {
    let ws = server.workspaces.get(id)?;
    server.outputs.iter().position(|o| o.name == ws.monitor)
}

/// Make workspace `id`, belonging to output `out`, unless it exists already.
pub(crate) fn ensure(server: &mut Server, id: usize, out: usize) {
    if !server.workspaces.contains(id) {
        let ws = Workspace::new(
            &server.outputs[out].name,
            server.config.first_split_vertical,
        );
        server.workspaces.insert(id, ws);
    }
}

/// The lowest number no workspace has.
fn lowest_free(server: &Server) -> usize {
    (0..MAX_WORKSPACE)
        .find(|&id| !server.workspaces.contains(id))
        .unwrap_or(0)
}

/// Drop every workspace that is empty and on no output — the moment it is
/// both, a Hyprland workspace stops existing. Any output remembering it is
/// told to forget, so a later workspace reusing the number is not mistaken
/// for it.
pub(crate) fn prune(server: &mut Server) {
    let shown: Vec<usize> = server.outputs.iter().map(|o| o.workspace).collect();
    let mut dropped = Vec::new();
    server.workspaces.retain(|id, ws| {
        let keep = !ws.windows.is_empty() || shown.contains(&id);
        if !keep {
            dropped.push(id);
        }
        keep
    });
    if !dropped.is_empty() {
        server
            .remembered_workspace
            .retain(|_, id| !dropped.contains(id));
    }
}

/// Hyprland's `r+1` / `r-1`: the next (or previous) number for output `out`,
/// counting numbers no workspace has yet and skipping those that belong to
/// another output. Upwards it does not wrap; downwards it stops at the lowest
/// such number.
pub(crate) fn relative(server: &Server, out: usize, forward: bool) -> usize {
    let name = &server.outputs[out].name;
    let current = server.outputs[out].workspace;
    let taken = |id: usize| {
        server
            .workspaces
            .get(id)
            .is_some_and(|ws| &ws.monitor != name)
    };
    if forward {
        (current + 1..MAX_WORKSPACE)
            .find(|&id| !taken(id))
            .unwrap_or(current)
    } else {
        (0..current)
            .rev()
            .find(|&id| !taken(id))
            .or_else(|| (0..MAX_WORKSPACE).find(|&id| !taken(id)))
            .unwrap_or(current)
    }
}

/// An output's usable area.
fn usable(server: &Server, out: usize) -> (i32, i32, i32, i32) {
    let o = &server.outputs[out];
    (o.ux, o.uy, o.uw, o.uh)
}

/// Keep a floating window in the same place relative to its output when its
/// workspace changes output, and on screen if the new output is smaller.
/// `from` is the old output's usable area; `None` when that output is gone,
/// in which case the position is wrapped into the new one, as Hyprland does.
///
/// Only the recorded position changes: `refresh` places floating windows
/// from it.
unsafe fn carry_floating(
    tl: *mut Toplevel,
    from: Option<(i32, i32, i32, i32)>,
    to: (i32, i32, i32, i32),
) {
    let (tx, ty, tw, th) = to;
    let (x, y) = match from {
        Some((fx, fy, _, _)) => ((*tl).x - fx + tx, (*tl).y - fy + ty),
        None => (
            tx + ((*tl).x - tx).rem_euclid(tw.max(1)),
            ty + ((*tl).y - ty).rem_euclid(th.max(1)),
        ),
    };
    (*tl).x = x.clamp(tx, (tx + tw - (*tl).w).max(tx));
    (*tl).y = y.clamp(ty, (ty + th - (*tl).h).max(ty));
}

/// A floating window placed for output `from` is going to workspace `id`: if
/// that workspace belongs to another output, take the window along, keeping
/// its place relative to the output.
pub(crate) unsafe fn carry_floating_to(server: &Server, tl: *mut Toplevel, from: usize, id: usize) {
    if let Some(to) = owner(server, id).filter(|&to| to != from) {
        carry_floating(tl, Some(usable(server, from)), usable(server, to));
    }
}

/// Give workspace `id` to output `to`, as Hyprland's `moveWorkspaceToMonitor`
/// does. Floating windows keep their place relative to the output. With
/// `replace`, an old output that was showing it shows another of its own
/// workspaces instead, or a new one; without, the caller is about to remove
/// the old output and it does not matter.
pub(crate) fn move_to_output(server: &mut Server, id: usize, to: usize, replace: bool) {
    let from = owner(server, id);
    if from == Some(to) {
        return;
    }
    if let Some(f) = from {
        if replace && server.outputs[f].workspace == id {
            let other = server
                .workspaces
                .iter()
                .find(|&(other, ws)| other != id && ws.monitor == server.outputs[f].name)
                .map(|(other, _)| other);
            let next = match other {
                Some(other) => other,
                None => {
                    let fresh = lowest_free(server);
                    ensure(server, fresh, f);
                    fresh
                }
            };
            server.outputs[f].workspace = next;
        }
    }

    let from_area = from.map(|f| usable(server, f));
    let to_area = usable(server, to);
    let ws = &mut server.workspaces[id];
    ws.monitor = server.outputs[to].name.clone();
    for &tl in &ws.windows {
        // SAFETY: windows in a workspace are live `Toplevel`s; they are
        // dropped from it before they are freed (`toplevel::remove_window`).
        unsafe {
            if (*tl).floating && !(*tl).fullscreen {
                carry_floating(tl, from_area, to_area);
            }
        }
    }
}

/// A new output is in `server.outputs` at `out` and shows nothing yet; pick
/// what it shows. In Hyprland's order: workspaces that were on this connector
/// when it was unplugged come back to it; if it is the only output, any
/// workspace left without one is taken in; then the workspace it was showing
/// when unplugged is shown again, and failing that, the lowest free number.
pub(crate) fn output_connected(server: &mut Server, out: usize) {
    let name = server.outputs[out].name.clone();
    let alone = server.outputs.len() == 1;

    let ids: Vec<usize> = server.workspaces.ids().collect();
    for id in ids {
        let returning = server.workspaces[id].return_to.as_deref() == Some(name.as_str());
        let orphaned = owner(server, id).is_none();
        let adopted = orphaned && alone;
        if !returning && !adopted {
            continue;
        }
        move_to_output(server, id, out, true);
        if returning {
            server.workspaces[id].return_to = None;
        }
    }

    if let Some(&id) = server.remembered_workspace.get(&name) {
        if server.workspaces.contains(id) {
            move_to_output(server, id, out, true);
            server.outputs[out].workspace = id;
            return;
        }
    }

    let id = lowest_free(server);
    ensure(server, id, out);
    server.outputs[out].workspace = id;
}

/// Output `out` is about to be removed: remember what it was showing, mark
/// its workspaces to return to it, and give them, hidden, to the first other
/// output. With no other output they wait, without one, until an output
/// arrives (`output_connected` takes them in).
pub(crate) fn output_disconnecting(server: &mut Server, out: usize) {
    let name = server.outputs[out].name.clone();
    let showing = server.outputs[out].workspace;
    server.remembered_workspace.insert(name.clone(), showing);
    let backup = (0..server.outputs.len()).find(|&i| i != out);

    let ids: Vec<usize> = server.workspaces.ids().collect();
    for id in ids {
        let here = server.workspaces[id].monitor == name;
        let orphaned = owner(server, id).is_none();
        if !here && !orphaned {
            continue;
        }
        if here && server.workspaces[id].return_to.is_none() {
            server.workspaces[id].return_to = Some(name.clone());
        }
        if let Some(b) = backup {
            move_to_output(server, id, b, false);
        }
    }
}

/// Give keyboard focus to workspace `id`'s focused window, or to nothing when
/// it is empty: keys must not keep going to a window that just left the
/// screen. Never while locked — the lock surface owns the keyboard.
pub(crate) unsafe fn focus_windows_of(server: &mut Server, id: usize) {
    if server.locked {
        return;
    }
    let ws = &server.workspaces[id];
    match ws.windows.get(ws.focused) {
        Some(&tl) => oxide_focus_toplevel(server.seat, (*tl).xdg_toplevel),
        None => oxide_seat_clear_keyboard_focus(server.seat),
    }
}

/// Move the cursor to the middle of output `out`, which makes it the focused
/// output (`tiling::active_output` follows the cursor).
pub(crate) unsafe fn warp_to(server: &Server, out: usize) {
    let o = &server.outputs[out];
    oxide_cursor_warp(
        server.cursor,
        f64::from(o.x) + f64::from(o.w) / 2.0,
        f64::from(o.y) + f64::from(o.h) / 2.0,
    );
}

/// Hyprland's `workspace N`: show workspace `id` on the output it belongs to
/// — making it on the focused output if it does not exist — and focus it.
/// When that is another output, focus and the cursor go there.
pub(crate) unsafe fn focus(server: &mut Server, id: usize) {
    if server.outputs.is_empty() {
        return;
    }
    let here = active_output(server);
    let from = server.outputs[here].workspace;
    ensure(server, id, here);
    // Only possible between the last output leaving and the next arriving,
    // which `output_connected` normally resolves first; adopt it rather than
    // show it nowhere.
    let target = match owner(server, id) {
        Some(o) => o,
        None => {
            move_to_output(server, id, here, true);
            here
        }
    };
    if target == here && from == id {
        return;
    }
    server.outputs[target].workspace = id;
    refresh(server);
    if target != here {
        warp_to(server, target);
    }
    focus_windows_of(server, id);
    eprintln!(
        "0xin: output {} -> workspace {}",
        server.outputs[target].name,
        id + 1
    );
    crate::events::emit(
        server,
        "workspace",
        &format!("{} {}", id + 1, server.outputs[target].name),
    );
    if from != id {
        crate::lua::run_workspace(server, from, id);
    }
}

/// Move the focused window of the focused workspace to workspace `id`, made
/// on the focused output if it does not exist. With `follow`, switch to it as
/// `focus` does and keep the window focused — Hyprland's `movetoworkspace`;
/// without, stay where you are — its `movetoworkspacesilent`.
pub(crate) unsafe fn move_focused_window(server: &mut Server, id: usize, follow: bool) {
    if server.outputs.is_empty() {
        return;
    }
    let here = active_output(server);
    let from = server.outputs[here].workspace;
    if id == from || server.workspaces[from].windows.is_empty() {
        return;
    }
    ensure(server, id, here);

    let src = &server.workspaces[from];
    let tl = src.windows[src.focused];
    let tiled = !(*tl).floating && !(*tl).fullscreen;
    detach(server, tl);

    let dst = &mut server.workspaces[id];
    dst.windows.push(tl);
    dst.focused = dst.windows.len() - 1;
    if tiled {
        tree_track(dst, tl);
    }
    if (*tl).floating && !(*tl).fullscreen {
        carry_floating_to(server, tl, here, id);
    }
    eprintln!("0xin: moved window to workspace {}", id + 1);

    if follow {
        focus(server, id);
    } else {
        // The destination may or may not be on screen somewhere.
        refresh(server);
        focus_windows_of(server, from);
    }
}

/// Take `tl` out of whichever workspace holds it: its leaf in the split tree,
/// solo, and its place in the window list, keeping that workspace's
/// `focused` on the window it was on (or the nearest one, if that was `tl`).
/// Returns the workspace it was on, or `None` if it was on none. Re-tiling
/// and focus are left to the caller, since each wants something different
/// next.
pub(crate) unsafe fn detach(server: &mut Server, tl: *mut Toplevel) -> Option<usize> {
    let id = crate::tiling::workspace_of(server, tl)?;
    let ws = &mut server.workspaces[id];
    let pos = ws.windows.iter().position(|&w| w == tl)?;
    // While `tl` is still listed: tiled_position finds its leaf from there.
    if !(*tl).floating && !(*tl).fullscreen {
        tree_untrack(ws, tl);
    }
    if ws.solo == Some(tl) {
        ws.solo = None;
    }
    ws.windows.remove(pos);
    if pos < ws.focused {
        ws.focused -= 1;
    } else if ws.focused >= ws.windows.len() && !ws.windows.is_empty() {
        ws.focused = ws.windows.len() - 1;
    }
    Some(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{Output, Workspaces, REPAINT_FRAMES};
    use crate::tiling::tests::{server_with, toplevel_at};
    use std::ptr;

    /// An output named `name` with box `(x, y, w, h)`, showing workspace `ws`.
    fn output(name: &str, x: i32, w: i32, ws: usize) -> Output {
        Output {
            wlr_output: ptr::null_mut(),
            name: name.into(),
            x,
            y: 0,
            w,
            h: 1000,
            transform: 0,
            ux: x,
            uy: 0,
            uw: w,
            uh: 1000,
            workspace: ws,
            frame_listener: ptr::null_mut(),
            destroy_listener: ptr::null_mut(),
            background: ptr::null_mut(),
            wallpaper: ptr::null_mut(),
            gesture_handle: ptr::null_mut(),
            lock_fallback: ptr::null_mut(),
            frame_ctx: ptr::null_mut(),
            repaint_frames: REPAINT_FRAMES,
        }
    }

    /// A server with no workspaces and no outputs.
    unsafe fn empty() -> Server {
        let mut server = server_with(Vec::new());
        server.workspaces = Workspaces::default();
        server
    }

    /// Plug in `name` the way `output::handle_new_output` does.
    fn plug(server: &mut Server, name: &str, x: i32, w: i32) -> usize {
        server.outputs.push(output(name, x, w, usize::MAX));
        let out = server.outputs.len() - 1;
        output_connected(server, out);
        prune(server);
        out
    }

    /// Unplug `name` the way `output::handle_output_destroy` does.
    fn unplug(server: &mut Server, name: &str) {
        let out = server.outputs.iter().position(|o| o.name == name).unwrap();
        output_disconnecting(server, out);
        server.outputs.remove(out);
        prune(server);
    }

    /// Put a window on workspace `id` (0-based), which must exist.
    unsafe fn add_window(server: &mut Server, id: usize) -> *mut Toplevel {
        let tl = toplevel_at(0, 0, 100, 100);
        server.workspaces[id].windows.push(tl);
        tl
    }

    fn showing(server: &Server, name: &str) -> usize {
        server
            .outputs
            .iter()
            .find(|o| o.name == name)
            .unwrap()
            .workspace
    }

    #[test]
    fn a_new_output_gets_the_lowest_free_workspace_not_a_hidden_one() {
        unsafe {
            let mut s = empty();
            plug(&mut s, "eDP-1", 0, 1000);
            assert_eq!(showing(&s, "eDP-1"), 0);
            // Workspace 2 hidden on the laptop, with a window on it.
            ensure(&mut s, 1, 0);
            add_window(&mut s, 1);

            plug(&mut s, "HDMI-A-1", 1000, 1000);
            // Not workspace 2 (it exists, on the laptop): a fresh 3.
            assert_eq!(showing(&s, "HDMI-A-1"), 2);
            assert_eq!(s.workspaces[1].monitor, "eDP-1");
        }
    }

    #[test]
    fn empty_workspaces_stop_existing_once_off_screen() {
        unsafe {
            let mut s = empty();
            plug(&mut s, "eDP-1", 0, 1000);
            ensure(&mut s, 4, 0);
            s.outputs[0].workspace = 4;
            prune(&mut s);
            // 1 was empty and is no longer shown; 5 is shown.
            assert_eq!(s.workspaces.ids().collect::<Vec<_>>(), [4]);
        }
    }

    #[test]
    fn unplugging_hands_workspaces_over_and_replugging_brings_them_back() {
        unsafe {
            let mut s = empty();
            plug(&mut s, "eDP-1", 0, 1000);
            add_window(&mut s, 0);
            plug(&mut s, "HDMI-A-1", 1000, 1000);
            // HDMI shows 2 and has 3 hidden, both with windows.
            add_window(&mut s, 1);
            ensure(&mut s, 2, 1);
            add_window(&mut s, 2);

            unplug(&mut s, "HDMI-A-1");
            assert_eq!(showing(&s, "eDP-1"), 0, "the laptop keeps its own");
            assert_eq!(s.workspaces[1].monitor, "eDP-1");
            assert_eq!(s.workspaces[2].monitor, "eDP-1");

            plug(&mut s, "HDMI-A-1", 1000, 1000);
            assert_eq!(showing(&s, "HDMI-A-1"), 1, "shows what it showed");
            assert_eq!(s.workspaces[1].monitor, "HDMI-A-1");
            assert_eq!(s.workspaces[2].monitor, "HDMI-A-1");
            assert_eq!(s.workspaces[1].return_to, None);
        }
    }

    #[test]
    fn a_returning_workspace_shown_elsewhere_is_replaced_there() {
        unsafe {
            let mut s = empty();
            plug(&mut s, "eDP-1", 0, 1000);
            add_window(&mut s, 0);
            plug(&mut s, "HDMI-A-1", 1000, 1000);
            add_window(&mut s, 1);
            unplug(&mut s, "HDMI-A-1");
            // While it was away, the laptop switched to HDMI's workspace.
            s.outputs[0].workspace = 1;
            prune(&mut s);

            plug(&mut s, "HDMI-A-1", 1000, 1000);
            assert_eq!(showing(&s, "HDMI-A-1"), 1);
            assert_eq!(showing(&s, "eDP-1"), 0, "back to its own workspace");
        }
    }

    #[test]
    fn with_every_output_gone_the_first_one_back_takes_everything_in() {
        unsafe {
            let mut s = empty();
            plug(&mut s, "DP-1", 0, 1000);
            add_window(&mut s, 0);
            plug(&mut s, "DP-2", 1000, 1000);
            add_window(&mut s, 1);
            unplug(&mut s, "DP-1");
            unplug(&mut s, "DP-2");
            assert!(s.outputs.is_empty());
            // The windows are not lost while nothing is connected.
            assert_eq!(s.workspaces.ids().collect::<Vec<_>>(), [0, 1]);

            plug(&mut s, "DP-2", 0, 1000);
            assert_eq!(s.workspaces[0].monitor, "DP-2");
            assert_eq!(showing(&s, "DP-2"), 1, "its own remembered workspace");
            plug(&mut s, "DP-1", 1000, 1000);
            assert_eq!(showing(&s, "DP-1"), 0, "and DP-1 gets its own back");
        }
    }

    #[test]
    fn a_different_monitor_after_none_takes_in_the_orphans() {
        unsafe {
            let mut s = empty();
            plug(&mut s, "DP-1", 0, 1000);
            add_window(&mut s, 0);
            unplug(&mut s, "DP-1");

            // A different connector: nothing returns to it, but as the only
            // output it adopts what has nowhere else to go.
            plug(&mut s, "HDMI-A-1", 0, 1000);
            assert_eq!(s.workspaces[0].monitor, "HDMI-A-1");
            assert_eq!(showing(&s, "HDMI-A-1"), 1, "a fresh one on top");

            // DP-1 still gets its own back when it reappears.
            plug(&mut s, "DP-1", 1000, 1000);
            assert_eq!(showing(&s, "DP-1"), 0);
            assert_eq!(s.workspaces[0].monitor, "DP-1");
        }
    }

    #[test]
    fn next_and_previous_skip_workspaces_on_other_outputs() {
        unsafe {
            let mut s = empty();
            plug(&mut s, "eDP-1", 0, 1000);
            plug(&mut s, "HDMI-A-1", 1000, 1000);
            add_window(&mut s, 1); // 2 is HDMI's
            ensure(&mut s, 2, 1);
            add_window(&mut s, 2); // and so is 3
            assert_eq!(relative(&s, 0, true), 3, "1 -> 4, past HDMI's 2 and 3");
            s.outputs[0].workspace = 3;
            ensure(&mut s, 3, 0);
            assert_eq!(relative(&s, 0, false), 0, "4 -> 1, the same way back");
            s.outputs[0].workspace = 0;
            assert_eq!(relative(&s, 0, false), 0, "no going below the first");
        }
    }

    #[test]
    fn a_floating_window_keeps_its_place_on_the_new_output() {
        unsafe {
            let mut s = empty();
            plug(&mut s, "eDP-1", 0, 1000);
            plug(&mut s, "HDMI-A-1", 1000, 1000);
            let tl = add_window(&mut s, 1);
            (*tl).floating = true;
            ((*tl).x, (*tl).y) = (1200, 300);

            move_to_output(&mut s, 1, 0, true);
            assert_eq!(((*tl).x, (*tl).y), (200, 300));
        }
    }

    #[test]
    fn detaching_a_window_keeps_focus_on_the_window_that_had_it() {
        unsafe {
            let (a, b, c) = (
                toplevel_at(0, 0, 10, 10),
                toplevel_at(0, 0, 10, 10),
                toplevel_at(0, 0, 10, 10),
            );
            let mut s = server_with(vec![a, b, c]);
            // b tiled, holding the only leaf; a and c floating.
            (*a).floating = true;
            (*c).floating = true;
            tree_track(&mut s.workspaces[0], b);
            s.workspaces[0].focused = 2; // c

            // One before the focused window: the index follows c.
            assert_eq!(detach(&mut s, a), Some(0));
            assert_eq!(s.workspaces[0].windows, vec![b, c]);
            assert_eq!(s.workspaces[0].focused, 1);

            // The focused one itself: focus falls to its neighbour.
            assert_eq!(detach(&mut s, c), Some(0));
            assert_eq!(s.workspaces[0].focused, 0);

            // A tiled one gives its leaf back.
            assert_eq!(detach(&mut s, b), Some(0));
            assert!(s.workspaces[0].tree.is_none());

            // On no workspace (a hidden scratchpad): nothing to do.
            assert_eq!(detach(&mut s, a), None);
        }
    }
}
