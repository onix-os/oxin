//! The split-tree layout: `Node` and the pure operations over it.
//!
//! A workspace's tiled windows sit in an explicit binary tree of splits,
//! each with a persisted `ratio` — the piece of state a plain `Vec`-order
//! layout had no room for (Stage 10). Every function here is pure: no
//! `Server`/`Workspace`/`Toplevel`, no unsafe, no FFI — just the tree.
//! `tiling.rs` is what plugs this into live compositor state.

use crate::config::Direction;

/// The original flat-list spiral (dwindle) layout: `n` rects filling the
/// `x,y,w,h` box with `gap` pixels around and between windows. Each window
/// except the last splits the remaining rect, alternating vertical
/// (left/right) then horizontal (top/bottom); the window takes the first
/// half, the rest recurse into the second. `refresh()` no longer calls this
/// (Stage 10 moved it to the split tree below) — it survives as the known-
/// good reference the tree's output is checked against.
#[cfg(test)]
pub(crate) fn spiral_rects(
    n: usize,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    gap: i32,
) -> Vec<(i32, i32, i32, i32)> {
    let (mut rx, mut ry) = (x + gap, y + gap);
    let (mut rw, mut rh) = ((w - gap * 2).max(1), (h - gap * 2).max(1));
    let mut split_vertical = true;
    let mut rects = Vec::with_capacity(n);
    for i in 0..n {
        let (x, y, w, h);
        if i == n - 1 {
            // Last window fills whatever rect is left.
            (x, y, w, h) = (rx, ry, rw, rh);
        } else if split_vertical {
            let half = ((rw - gap) / 2).max(1);
            (x, y, w, h) = (rx, ry, half, rh);
            rx += half + gap;
            rw = (rw - half - gap).max(1);
        } else {
            let half = ((rh - gap) / 2).max(1);
            (x, y, w, h) = (rx, ry, rw, half);
            ry += half + gap;
            rh = (rh - half - gap).max(1);
        }
        split_vertical = !split_vertical;
        rects.push((x, y, w, h));
    }
    rects
}

/// One node of a workspace's split tree: a window (`Leaf`) or a divider
/// between two children. `ratio` is the fraction of the split's box the
/// `first` child gets — the piece of state the flat `Vec`-order spiral had
/// no room for, and the reason this tree exists (Stage 10: per-window
/// resize). Shape mirrors the old spiral's alternating dwindle exactly, so
/// `build_dwindle` + `tree_rects` reproduce it bit-for-bit at the default
/// ratio. Lives on `Workspace.tree`, one per workspace; `Clone` is for
/// `predict_tile_rect`, which simulates an insert without touching the real
/// tree.
#[derive(Clone)]
pub(crate) enum Node {
    Leaf,
    Split {
        vertical: bool,
        ratio: f32,
        first: Box<Node>,
        second: Box<Node>,
    },
}

/// Build the tree for `n` windows in the same shape `spiral_rects` computes
/// iteratively: window 0 splits off the first half (alternating
/// vertical/horizontal), the rest recurse into the second half, and the
/// last window is a bare leaf that fills whatever's left. `None` for `n == 0`
/// (nothing to tile).
pub(crate) fn build_dwindle(n: usize) -> Option<Node> {
    fn go(n: usize, vertical: bool) -> Node {
        if n <= 1 {
            Node::Leaf
        } else {
            Node::Split {
                vertical,
                ratio: 0.5,
                first: Box::new(Node::Leaf),
                second: Box::new(go(n - 1, !vertical)),
            }
        }
    }
    (n > 0).then(|| go(n, true))
}

/// Render a split tree to rects, in the same left-to-right/top-to-bottom
/// in-order as `spiral_rects` — leaf `i` here is window `i` of the same
/// list. Gap semantics match exactly: one `gap` margin around the outer box,
/// one `gap` between each split's two children, none inside a leaf.
pub(crate) fn tree_rects(
    tree: &Node,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    gap: i32,
) -> Vec<(i32, i32, i32, i32)> {
    fn go(
        node: &Node,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        gap: i32,
        out: &mut Vec<(i32, i32, i32, i32)>,
    ) {
        match node {
            Node::Leaf => out.push((x, y, w.max(1), h.max(1))),
            Node::Split {
                vertical,
                ratio,
                first,
                second,
            } => {
                if *vertical {
                    let fw = (((w - gap) as f32) * ratio) as i32;
                    let (fw, sw) = (fw.max(1), (w - gap - fw).max(1));
                    go(first, x, y, fw, h, gap, out);
                    go(second, x + fw + gap, y, sw, h, gap, out);
                } else {
                    let fh = (((h - gap) as f32) * ratio) as i32;
                    let (fh, sh) = (fh.max(1), (h - gap - fh).max(1));
                    go(first, x, y, w, fh, gap, out);
                    go(second, x, y + fh + gap, w, sh, gap, out);
                }
            }
        }
    }
    let mut out = Vec::with_capacity(tree_leaf_count(tree));
    go(
        tree,
        x + gap,
        y + gap,
        (w - gap * 2).max(1),
        (h - gap * 2).max(1),
        gap,
        &mut out,
    );
    out
}

/// Number of leaves (windows) a tree holds — the capacity hint for
/// `tree_rects`, and how `tiling::predict_tile_rect` finds the current count.
pub(crate) fn tree_leaf_count(tree: &Node) -> usize {
    match tree {
        Node::Leaf => 1,
        Node::Split { first, second, .. } => tree_leaf_count(first) + tree_leaf_count(second),
    }
}

/// Insert a new leaf so it becomes tiled-position `i` (0-indexed, in-order)
/// once inserted. Every split the new leaf doesn't pass through keeps its
/// `ratio` exactly as it was — only the split that gains the new leaf as a
/// child is freshly created, at the default 0.5, same as any leaf
/// `build_dwindle` creates. Passing the tree's current leaf count as `i`
/// appends at the end, which is what a newly mapped window always does; any
/// other `i` is a window rejoining the tiled set (after a float/fullscreen
/// toggle) at whichever position `Workspace.windows` order puts it among the
/// other tiled ones.
pub(crate) fn tree_insert_at(tree: Option<Node>, i: usize, first_vertical: bool) -> Node {
    fn go(node: Node, i: usize, axis: bool) -> Node {
        match node {
            Node::Leaf => Node::Split {
                vertical: axis,
                ratio: 0.5,
                first: Box::new(Node::Leaf),
                second: Box::new(Node::Leaf),
            },
            Node::Split {
                vertical,
                ratio,
                first,
                second,
            } => {
                let first_n = tree_leaf_count(&first);
                if i < first_n {
                    Node::Split {
                        vertical,
                        ratio,
                        first: Box::new(go(*first, i, !vertical)),
                        second,
                    }
                } else {
                    Node::Split {
                        vertical,
                        ratio,
                        first,
                        second: Box::new(go(*second, i - first_n, !vertical)),
                    }
                }
            }
        }
    }
    match tree {
        None => Node::Leaf,
        Some(t) => go(t, i, first_vertical),
    }
}

/// Remove tiled-position `i` (0-indexed, in-order) from the tree. The
/// removed leaf's sibling — and everything under it, ratios untouched —
/// reclaims the freed space by collapsing the now-empty parent split away.
/// `None` once the last leaf is gone.
pub(crate) fn tree_remove(tree: Option<Node>, i: usize) -> Option<Node> {
    match tree? {
        Node::Leaf => None,
        Node::Split {
            vertical,
            ratio,
            first,
            second,
        } => {
            let first_n = tree_leaf_count(&first);
            if i < first_n {
                match tree_remove(Some(*first), i) {
                    Some(f) => Some(Node::Split {
                        vertical,
                        ratio,
                        first: Box::new(f),
                        second,
                    }),
                    None => Some(*second),
                }
            } else {
                match tree_remove(Some(*second), i - first_n) {
                    Some(s) => Some(Node::Split {
                        vertical,
                        ratio,
                        first,
                        second: Box::new(s),
                    }),
                    None => Some(*first),
                }
            }
        }
    }
}

/// Ratio bounds a resize can't push past — keeps either side of a split from
/// collapsing away under repeated presses.
const MIN_RATIO: f32 = 0.1;
const MAX_RATIO: f32 = 0.9;

/// Resize tiled-position `i`'s window by `delta` in direction `dir`: walk up
/// from its leaf to the *nearest* ancestor split whose axis matches `dir`
/// (vertical for Left/Right, horizontal for Up/Down) — deeper splits along
/// the other axis don't affect this edge at all, so the first matching one
/// found is the one actually bordering the window in that direction —
/// deliberately local, no cascading to a farther split even if one exists
/// further up. That split has exactly one edge shared between its two
/// windows, and *both* directions on the matching axis move it — Right/Down
/// give `first` more of the box (ratio up), Left/Up give it less (ratio
/// down) — so every direction does something; which one grows the *focused*
/// window versus shrinks it just falls out of which side of the split it's
/// on (`first` grows on Right/Down, `second` grows on Left/Up).
pub(crate) fn tree_resize(tree: &mut Node, i: usize, dir: Direction, delta: f32) {
    fn go(node: &mut Node, i: usize, dir: Direction, delta: f32) -> bool {
        let Node::Split {
            vertical,
            ratio,
            first,
            second,
        } = node
        else {
            return false;
        };
        let first_n = tree_leaf_count(first);
        let found = if i < first_n {
            go(first, i, dir, delta)
        } else {
            go(second, i - first_n, dir, delta)
        };
        if found {
            return true; // a nearer matching-axis split already handled this
        }
        let axis_matches = matches!(
            (*vertical, dir),
            (true, Direction::Left | Direction::Right) | (false, Direction::Up | Direction::Down)
        );
        if !axis_matches {
            return false; // keep looking further up for the right axis
        }
        let step = if matches!(dir, Direction::Right | Direction::Down) {
            delta
        } else {
            -delta
        };
        *ratio = (*ratio + step).clamp(MIN_RATIO, MAX_RATIO);
        true // this was the nearest matching-axis split — stop here
    }
    go(tree, i, dir, delta);
}

#[cfg(test)]
mod tests {
    use super::*;

    // The tree must reproduce today's spiral exactly at the default ratio —
    // this is what lets Stage 10 land without changing anything a user can
    // see until resize (a later step) actually sets a non-0.5 ratio.
    #[test]
    fn dwindle_tree_matches_spiral_rects() {
        for n in 0..=8 {
            let spiral = spiral_rects(n, 10, 20, 1280, 720, 3);
            let tree = match build_dwindle(n) {
                Some(t) => tree_rects(&t, 10, 20, 1280, 720, 3),
                None => Vec::new(),
            };
            assert_eq!(tree, spiral, "mismatch at n={n}");
        }
    }

    // The real insert path (append one at a time, as every window map does)
    // must land on the exact same shape `build_dwindle` computes in one shot
    // — otherwise a workspace built window-by-window would tile differently
    // than the parity test above assumes.
    #[test]
    fn tree_insert_at_end_matches_build_dwindle() {
        let mut tree: Option<Node> = None;
        for n in 1..=8 {
            let count_before = tree.as_ref().map_or(0, tree_leaf_count);
            tree = Some(tree_insert_at(tree, count_before, true));
            let expected = build_dwindle(n).unwrap();
            assert_eq!(
                tree_rects(tree.as_ref().unwrap(), 0, 0, 1280, 720, 4),
                tree_rects(&expected, 0, 0, 1280, 720, 4),
                "mismatch at n={n}"
            );
        }
    }

    #[test]
    fn horizontal_first_split_stacks_first_two_windows() {
        let tree = tree_insert_at(Some(Node::Leaf), 1, false);
        assert_eq!(
            tree_rects(&tree, 0, 0, 500, 1000, 0),
            vec![(0, 0, 500, 500), (0, 500, 500, 500)]
        );
    }

    // The whole point of the tree over the old spiral: removing one window
    // must not disturb another split's ratio. A|[B|C] with B|C customized to
    // 0.7 — removing A must leave B|C exactly as it was.
    #[test]
    fn tree_remove_collapses_sibling_and_preserves_other_ratios() {
        let tree = Node::Split {
            vertical: true,
            ratio: 0.5,
            first: Box::new(Node::Leaf),
            second: Box::new(Node::Split {
                vertical: false,
                ratio: 0.7,
                first: Box::new(Node::Leaf),
                second: Box::new(Node::Leaf),
            }),
        };
        match tree_remove(Some(tree), 0).unwrap() {
            Node::Split {
                vertical, ratio, ..
            } => {
                assert!(!vertical);
                assert_eq!(ratio, 0.7);
            }
            Node::Leaf => panic!("expected the B|C split to survive removal of A"),
        }
    }

    #[test]
    fn tree_remove_last_leaf_empties_the_tree() {
        assert!(tree_remove(Some(Node::Leaf), 0).is_none());
    }

    // A|B, vertical split: A is first(left), B is second(right). Whichever
    // window is focused, both directions on the matching axis move the one
    // shared edge — one direction grows it, the other shrinks it back — so a
    // grow is always fully undone by the opposite key, on either window.
    #[test]
    fn tree_resize_either_direction_moves_the_shared_edge() {
        let mut tree = Node::Split {
            vertical: true,
            ratio: 0.5,
            first: Box::new(Node::Leaf),
            second: Box::new(Node::Leaf),
        };

        // 0.25 (not the real RESIZE_STEP) because it's exactly representable
        // in binary floating point, so the assertions below can compare
        // against literals without worrying about rounding drift.
        tree_resize(&mut tree, 0, Direction::Right, 0.25); // A grows: ratio up
        assert_ratio(&tree, 0.75);
        tree_resize(&mut tree, 0, Direction::Left, 0.25); // A shrinks back: ratio down
        assert_ratio(&tree, 0.5);
        tree_resize(&mut tree, 1, Direction::Left, 0.25); // B grows: ratio down
        assert_ratio(&tree, 0.25);
        tree_resize(&mut tree, 1, Direction::Right, 0.25); // B shrinks back: ratio up
        assert_ratio(&tree, 0.5);
    }

    #[test]
    fn tree_resize_either_direction_moves_the_shared_edge_horizontal() {
        let mut tree = Node::Split {
            vertical: false,
            ratio: 0.5,
            first: Box::new(Node::Leaf),
            second: Box::new(Node::Leaf),
        };

        tree_resize(&mut tree, 0, Direction::Down, 0.25); // top grows: ratio up
        assert_ratio(&tree, 0.75);
        tree_resize(&mut tree, 0, Direction::Up, 0.25); // top shrinks back: ratio down
        assert_ratio(&tree, 0.5);
        tree_resize(&mut tree, 1, Direction::Up, 0.25); // bottom grows: ratio down
        assert_ratio(&tree, 0.25);
        tree_resize(&mut tree, 1, Direction::Down, 0.25); // bottom shrinks back: ratio up
        assert_ratio(&tree, 0.5);
    }

    #[test]
    fn tree_resize_clamps_at_bounds() {
        let mut tree = Node::Split {
            vertical: true,
            ratio: 0.5,
            first: Box::new(Node::Leaf),
            second: Box::new(Node::Leaf),
        };
        for _ in 0..20 {
            tree_resize(&mut tree, 0, Direction::Right, 0.1);
        }
        assert_ratio(&tree, MAX_RATIO);
    }

    // A|[B|C]: resizing B (the nearer, inner vertical split) must leave the
    // outer A|[B|C] split's ratio untouched — locality, not cascading.
    #[test]
    fn tree_resize_only_touches_the_nearest_matching_split() {
        let mut tree = Node::Split {
            vertical: true,
            ratio: 0.5,
            first: Box::new(Node::Leaf), // A, leaf 0
            second: Box::new(Node::Split {
                vertical: true,
                ratio: 0.5,
                first: Box::new(Node::Leaf),  // B, leaf 1
                second: Box::new(Node::Leaf), // C, leaf 2
            }),
        };
        tree_resize(&mut tree, 1, Direction::Right, 0.25); // grow B into C
        match &tree {
            Node::Split { ratio, second, .. } => {
                assert_eq!(*ratio, 0.5, "outer A|[B|C] ratio must be untouched");
                assert_ratio(second, 0.75);
            }
            Node::Leaf => panic!("expected a split"),
        }
    }

    fn assert_ratio(node: &Node, expected: f32) {
        match node {
            Node::Split { ratio, .. } => assert_eq!(*ratio, expected),
            Node::Leaf => panic!("expected a split, got a leaf"),
        }
    }
}
