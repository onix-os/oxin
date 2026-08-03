//! Tiling orchestration: bridges live compositor state (`Oxin`, `Workspace`,
//! `Window`) to the split tree in `layout.rs` — keeping each workspace's tree in
//! sync as windows come and go, and applying its output to the desktop space.
//! Also directional focus/move and layer-shell arrangement.

use smithay::desktop::{layer_map_for_output, Window};
use smithay::output::Output;
use smithay::utils::{Logical, Point, Rectangle, Size};

use crate::config::Direction;
use crate::layout::{tree_insert_at, tree_leaf_count, tree_rects, tree_remove};
use crate::state::{Oxin, Workspace, WORKSPACE_COUNT};
use crate::window;

/// Recompute the whole picture: unmap windows whose workspace isn't on any
/// output, then tile each output's workspace from its split tree. Called after
/// any change to windows, workspaces or outputs.
pub fn refresh(state: &mut Oxin) {
    // A window is visible iff its workspace is currently shown on some output.
    let mut shown = [false; WORKSPACE_COUNT];
    for entry in &state.outputs {
        shown[entry.workspace] = true;
    }

    let mut hide: Vec<Window> = Vec::new();
    for (index, ws) in state.workspaces.iter().enumerate() {
        for window in &ws.windows {
            let visible = shown[index]
                && match &ws.solo {
                    Some(solo) => window == solo || window::is_fullscreen(window),
                    None => true,
                };
            if !visible {
                hide.push(window.clone());
            }
        }
    }
    for window in hide {
        state.space.unmap_elem(&window);
    }

    let gap = state.config.gap;
    // Collected first: placing a window borrows the space mutably, and the
    // output/workspace lookups below borrow `state` immutably.
    let mut placements: Vec<(Window, Rectangle<i32, Logical>)> = Vec::new();

    for entry in &state.outputs {
        let ws = &state.workspaces[entry.workspace];
        if ws.windows.is_empty() {
            continue;
        }

        // Three kinds of window: fullscreen ones cover the output's full box
        // (over bars); floating ones keep whatever rect they already have (we
        // never place them here — their size is the client's own); the rest
        // tile in the usable area as usual.
        for window in ws.windows.iter().filter(|w| window::is_fullscreen(w)) {
            placements.push((window.clone(), entry.geometry));
        }

        if let Some(solo) = &ws.solo {
            // `solo` is never fullscreen (set_solo excludes it) — the branch
            // above already placed any genuinely-fullscreen window on this
            // workspace. Give the solo target the usable area (not the full
            // output box — solo isn't meant to be true fullscreen) and skip
            // the normal tiled placement entirely for this output.
            placements.push((solo.clone(), entry.usable));
            continue;
        }

        for window in ws.windows.iter().filter(|w| window::is_floating(w)) {
            // Floating windows keep their remembered rect; they still have to
            // be (re)mapped into the space so they stay visible.
            if !window::is_fullscreen(window) {
                placements.push((window.clone(), window::rect(window)));
            }
        }

        let tiled = tiled_windows(ws);
        let rects = match &ws.tree {
            Some(tree) => tree_rects(
                tree,
                entry.usable.loc.x,
                entry.usable.loc.y,
                entry.usable.size.w,
                entry.usable.size.h,
                gap,
            ),
            None => Vec::new(),
        };
        for (window, &(x, y, w, h)) in tiled.iter().zip(&rects) {
            placements.push((window.clone(), rect(x, y, w, h)));
        }
    }

    for (window, geometry) in placements {
        place(state, &window, geometry);
    }
}

/// Put one window at `geometry`: map it into the space at that position and,
/// unless it is floating (whose size is the client's own), configure it to
/// that size.
pub fn place(state: &mut Oxin, window: &Window, geometry: Rectangle<i32, Logical>) {
    state.space.map_element(window.clone(), geometry.loc, false);
    window::set_rect(window, geometry);
    if let Some(toplevel) = window.toplevel() {
        let size: Size<i32, Logical> = geometry.size;
        let changed = toplevel.with_pending_state(|pending| {
            let changed = pending.size != Some(size);
            pending.size = Some(size);
            changed
        });
        if changed {
            toplevel.send_pending_configure();
        }
    }
}

pub fn rect(x: i32, y: i32, w: i32, h: i32) -> Rectangle<i32, Logical> {
    Rectangle::new(Point::from((x, y)), Size::from((w, h)))
}

/// The windows of a workspace that are tiled — neither fullscreen nor
/// floating — in stacking order; the same order the split tree's leaves are
/// in. Shared by `refresh()`, `tiled_position`, and the initial-configure tile
/// prediction, so nothing can drift apart.
pub fn tiled_windows(ws: &Workspace) -> Vec<Window> {
    ws.windows
        .iter()
        .filter(|window| window::is_tiled(window))
        .cloned()
        .collect()
}

/// `window`'s index among its workspace's tiled windows right now — its leaf
/// position in the split tree — or `None` if it's floating or fullscreen.
pub fn tiled_position(ws: &Workspace, window: &Window) -> Option<usize> {
    tiled_windows(ws).iter().position(|w| w == window)
}

/// Remove `window`'s leaf from `ws`'s tree. Call *before* flipping whatever
/// flag (`floating`/`fullscreen`) is about to take it out of the tiled set —
/// the lookup needs the old state to still find it.
pub fn tree_untrack(ws: &mut Workspace, window: &Window) {
    if let Some(position) = tiled_position(ws, window) {
        ws.tree = tree_remove(ws.tree.take(), position);
    }
}

/// Insert a leaf for `window` into `ws`'s tree, at the position its (already
/// updated) tiled state puts it among the workspace's other tiled windows.
/// Call *after* flipping the flag that just made it tiled again.
pub fn tree_track(ws: &mut Workspace, window: &Window) {
    if let Some(position) = tiled_position(ws, window) {
        ws.tree = Some(tree_insert_at(
            ws.tree.take(),
            position,
            ws.first_split_vertical,
        ));
    }
}

/// The rect a new tiled window would get if it mapped onto `ws` right now:
/// simulates the append on a clone of the tree, leaving the real one
/// untouched, so the very first configure a client gets already matches the
/// size it will actually receive once it maps (avoids a resize jump on the
/// client's first frame — see `shell::xdg`).
pub fn predict_tile_rect(
    ws: &Workspace,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    gap: i32,
) -> (i32, i32, i32, i32) {
    let leaves = ws.tree.as_ref().map_or(0, tree_leaf_count);
    let candidate = tree_insert_at(ws.tree.clone(), leaves, ws.first_split_vertical);
    *tree_rects(&candidate, x, y, w, h, gap).last().unwrap()
}

/// Find whichever window in workspace `ws_idx` is spatially adjacent to
/// `from_idx` in direction `dir` (by their rects as of the last `refresh()`),
/// or `None` if nothing qualifies (no wraparound).
pub fn spatial_neighbor(state: &Oxin, ws_idx: usize, from_idx: usize, dir: Direction) -> Option<usize> {
    let rects: Vec<Rectangle<i32, Logical>> = state.workspaces[ws_idx]
        .windows
        .iter()
        .map(window::rect)
        .collect();
    neighbor_of_rects(&rects, from_idx, dir)
}

/// The directional-focus heuristic itself, over plain rects.
///
/// Filters to windows whose center lies on the correct side, then — like
/// i3/sway's directional focus — prefers whichever candidate shares the most
/// overlapping border with the focused window on the axis perpendicular to
/// `dir` (most overlap wins; primary-axis gap breaks ties). That's a much
/// stronger signal for "the window actually next to me" than raw
/// center-to-center distance: the dwindle spiral often puts one window
/// spanning much more area than its neighbors, and center-distance alone can
/// pick a window that doesn't really border the focused one, in a way that
/// isn't even reversible (A's right neighbor being B doesn't imply B's left
/// neighbor is A). Falls back to raw center-distance only when no candidate
/// has any border overlap at all (e.g. windows that meet only at a corner).
pub fn neighbor_of_rects(
    rects: &[Rectangle<i32, Logical>],
    from_idx: usize,
    dir: Direction,
) -> Option<usize> {
    let focused = *rects.get(from_idx)?;
    let (fx, fy, fw, fh) = (
        focused.loc.x,
        focused.loc.y,
        focused.size.w,
        focused.size.h,
    );
    let (fcx, fcy) = (fx + fw / 2, fy + fh / 2);

    let candidates: Vec<(usize, i32, i32)> = rects
        .iter()
        .enumerate()
        .filter(|&(i, _)| i != from_idx)
        .filter_map(|(i, candidate)| {
            let (cx, cy, cw, ch) = (
                candidate.loc.x,
                candidate.loc.y,
                candidate.size.w,
                candidate.size.h,
            );
            let (ccx, ccy) = (cx + cw / 2, cy + ch / 2);
            let (dx, dy) = (ccx - fcx, ccy - fcy);
            let on_side = match dir {
                Direction::Left => dx < 0,
                Direction::Right => dx > 0,
                Direction::Up => dy < 0,
                Direction::Down => dy > 0,
            };
            if !on_side {
                return None;
            }
            let overlap = match dir {
                Direction::Left | Direction::Right => (fy + fh).min(cy + ch) - fy.max(cy),
                Direction::Up | Direction::Down => (fx + fw).min(cx + cw) - fx.max(cx),
            }
            .max(0);
            let gap = match dir {
                Direction::Left | Direction::Right => dx.abs(),
                Direction::Up | Direction::Down => dy.abs(),
            };
            Some((i, overlap, gap))
        })
        .collect();

    if candidates.iter().any(|&(_, overlap, _)| overlap > 0) {
        candidates
            .into_iter()
            .max_by_key(|&(_, overlap, gap)| (overlap, -gap))
            .map(|(i, ..)| i)
    } else {
        candidates
            .into_iter()
            .min_by_key(|&(_, _, gap)| gap)
            .map(|(i, ..)| i)
    }
}

/// Recompute one output's layer-shell placement and the usable area left over.
/// Smithay's per-output layer map does the anchor/margin/exclusive-zone work;
/// we copy the resulting non-exclusive zone onto our `OutputEntry` so
/// `refresh()` can tile app windows within it.
pub fn arrange_layers(state: &mut Oxin, output: &Output) {
    let usable = {
        let mut map = layer_map_for_output(output);
        map.arrange();
        map.non_exclusive_zone()
    };
    let Some(entry) = state
        .outputs
        .iter_mut()
        .find(|entry| &entry.output == output)
    else {
        return;
    };
    // The layer map works in output-local coordinates; ours are global.
    entry.usable = Rectangle::new(entry.geometry.loc + usable.loc, usable.size);

    // Once the on-screen keyboard's layer surface has mapped, its exclusive
    // zone is the real boundary — better than the config's startup estimate,
    // which cannot know the device's scale.
    if state.keyboard_visible {
        let geometry = entry.geometry;
        let reserved = (geometry.loc.y + geometry.size.h) - (entry.usable.loc.y + entry.usable.size.h);
        state.gestures.set_keyboard_height(reserved);
    }
}

/// The output the pointer is currently on (the target for new windows and
/// workspace switches). Falls back to output 0 if the pointer is off all
/// outputs.
pub fn active_output(state: &Oxin) -> usize {
    let location = state.pointer_location.to_i32_round();
    state
        .outputs
        .iter()
        .position(|entry| entry.geometry.contains(location))
        .unwrap_or(0)
}

/// The workspace currently displayed on the active (pointer's) output.
pub fn active_workspace(state: &Oxin) -> usize {
    if state.outputs.is_empty() {
        return 0;
    }
    state.outputs[active_output(state)].workspace
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::spiral_rects;

    fn rects_from(spiral: Vec<(i32, i32, i32, i32)>) -> Vec<Rectangle<i32, Logical>> {
        spiral.into_iter().map(|(x, y, w, h)| rect(x, y, w, h)).collect()
    }

    // The bug this test guards against: at 4+ windows the spiral produces one
    // window (W2) whose only positive-overlap neighbor above it is W1, but a
    // plain center-distance heuristic picks W0 instead (W0 spans the full
    // output height, so its center can be "closer" even with zero shared
    // border) — not reversible with W1's own Down pick.
    #[test]
    fn spatial_neighbor_prefers_overlapping_border_at_4_windows() {
        let rects = rects_from(spiral_rects(4, 0, 0, 1280, 720, 0));
        assert_eq!(neighbor_of_rects(&rects, 2, Direction::Up), Some(1));
        assert_eq!(neighbor_of_rects(&rects, 1, Direction::Down), Some(3));
    }

    // Known, accepted limitation: the dwindle spiral can put two windows that
    // only touch at a single point (a "corner"), not a shared border — no
    // geometric heuristic makes that reversible, since neither window is
    // really "beside" the other. W1 and W3 meet only at (1280, 360) here, so
    // W1's Right neighbor (only candidate: W3) doesn't imply W3's Left
    // neighbor is W1 (it has real overlapping-border candidates, W0 and W2,
    // and correctly prefers one of those instead). Documented so a future
    // change to this heuristic doesn't have to silently re-discover this.
    #[test]
    fn spatial_neighbor_corner_touch_is_not_reversible() {
        let rects = rects_from(spiral_rects(4, 0, 0, 1280, 720, 0));
        assert_eq!(neighbor_of_rects(&rects, 1, Direction::Right), Some(3));
        assert_ne!(neighbor_of_rects(&rects, 3, Direction::Left), Some(1));
    }

    #[test]
    fn spatial_neighbor_2x2_grid() {
        // top-left(0) top-right(1)
        // bot-left(2) bot-right(3)
        let rects = vec![
            rect(0, 0, 100, 100),
            rect(100, 0, 100, 100),
            rect(0, 100, 100, 100),
            rect(100, 100, 100, 100),
        ];

        assert_eq!(neighbor_of_rects(&rects, 0, Direction::Right), Some(1));
        assert_eq!(neighbor_of_rects(&rects, 0, Direction::Down), Some(2));
        assert_eq!(neighbor_of_rects(&rects, 3, Direction::Left), Some(2));
        assert_eq!(neighbor_of_rects(&rects, 3, Direction::Up), Some(1));
        // No neighbor further right/up from the top-right window.
        assert_eq!(neighbor_of_rects(&rects, 1, Direction::Right), None);
        assert_eq!(neighbor_of_rects(&rects, 1, Direction::Up), None);
    }

    #[test]
    fn spatial_neighbor_prefers_aligned_over_diagonal() {
        // focused(0) at left; a slightly-offset-down neighbor(1) directly
        // right, and a far-diagonal neighbor(2) — same primary distance but
        // larger perpendicular offset. Right should pick (1).
        let rects = vec![
            rect(0, 0, 100, 100),
            rect(100, 10, 100, 100),
            rect(100, 500, 100, 100),
        ];
        assert_eq!(neighbor_of_rects(&rects, 0, Direction::Right), Some(1));
    }
}
