# Design & ideas

0xin is a dynamic tiling window manager for Wayland, written in Rust on
wlroots. This chapter documents its design decisions — how the layout,
configuration, and workspace model work and why — and the ideas planned
next. It is updated as decisions are made.

## Layout

Tiled windows sit in an explicit split tree (`layout::Node`: `Leaf` or
`Split { vertical, ratio, first, second }`), one per workspace, shaped like
the dwindle spiral described in
[Stage 5](phases/stage-5-window-management.md) by default but with a
**persisted ratio per split** — [Stage 10](phases/stage-10-split-tree.md)'s
addition. `Mod+Ctrl+hjkl` (`resizewindow`) adjusts the ratio of whichever
split borders the focused window in that direction; every other window's
ratio is untouched, including across opening and closing unrelated windows.

Windows themselves stay unaware of the tree: which window is tiled-position
*i* is still decided purely by `Workspace.windows`' order (now filtered to
non-floating, non-fullscreen ones), exactly as before. The tree only adds
the one thing a plain `Vec` had no room for — a number that survives between
`refresh()` calls. Directional navigation (`spatial_neighbor`) still uses
Stage 5's geometric heuristic rather than the tree's actual adjacency, so
the corner-touch ambiguity documented there is unchanged.

## Configuration

Three rules, applied throughout `src/config.rs`:

1. **Nothing is fatal.** A line that doesn't parse warns on stderr and is
   skipped. A missing config file means defaults. A config with zero `bind`
   lines still has every default binding. 0xin always starts.
2. **User config merges, never replaces.** A `bind` line overrides exactly
   that key combination; every unmentioned default stays active. A two-line
   config is a two-line diff, not a fork of the whole keymap.
3. **Explicit over implicit.** Monitor placement is literal pixel
   coordinates per named connector (`monitor = eDP-1, 0x0`) — no relative
   keywords, no DPI auto-scale heuristics. The config states what happens;
   nothing else does.

## Workspaces and outputs

Nine workspaces, with one invariant: **a workspace is never visible on two
outputs at once**. Switching to a workspace already shown on another monitor
swaps the two monitors' workspaces instead of duplicating it. New windows
open on the monitor the cursor is on (focus-follows-monitor). The model
stays predictable regardless of how many outputs are attached.

## Floating windows

Windows that shouldn't tile, don't: dialogs (a toplevel with a parent set —
file pickers, "Save as…"), windows that declare a fixed size, and anything
matched by a `float = <app_id>` config rule open floating instead — centered,
painted above the tiled layer. Dialogs and fixed-size windows keep their own
natural size (that's the point of floating them); rule windows and the
manual float toggle use the configured default size (`float_size`, a
percentage of the screen's usable area). Everything else tiles; floating is
the exception, decided per window, never a mode the whole workspace switches
into. Floating windows move and resize with `Mod+drag` (left moves, right
resizes) or keyboard nudges. The details are in the
[Stage 9 chapter](phases/stage-9-floating.md).

## Decorations

0xin always claims server-side decoration and draws nothing in its place:
every window is a bare, borderless rectangle. In a tiler the layout itself
conveys what title bars and borders would — window position and focus are
already visible from the arrangement.

## Planned ideas

Under consideration, not committed:

- **Runtime control** — a socket/IPC for querying and scripting the
  compositor without keybindings.
- **Exact tree-based directional navigation** — now that the split tree
  exists, `spatial_neighbor` could use its real adjacency instead of Stage
  5's geometric heuristic, fully resolving the corner-touch case. Not
  pursued in Stage 10 since it wasn't needed for the resize deliverable.

When one of these lands, it moves out of this list and into a stage chapter.
