# Stage 5 — Window Management (the heart of it)

**What it is.** The stage that turns 0xin from "a compositor that can show
one window" into an actual **tiling window manager**: multiple windows
sharing the screen automatically, workspaces, a config file, and
keybindings to drive all of it. This is the largest stage by far, and the
one that's kept growing well past its original deliverable.

**Why it matters.** This is the whole point of the project's shape — a
dynamic tiler, not a floating WM. Everything here is policy
(see [Architecture](../architecture.md)), which is why it all lives in Rust
with no shim involvement beyond the scene-node calls tiling needs to
position windows.

**Deliverable** (from `KICKOFF.md`): *multiple windows, a tiling layout +
workspaces, move/resize, keybindings, a config file. Usable tiling WM
behavior.*

## The tiling layout: spiral / dwindle

`src/tiling.rs`'s `refresh()` implements a **spiral (dwindle) layout**: each
new window recursively splits the remaining space, alternating vertical
(left/right) and horizontal (top/bottom) by depth. There's no persisted
tree structure — the whole layout is recomputed from `Workspace.windows: Vec<*mut
Toplevel>`'s list order on every call. That's a deliberate simplicity
tradeoff: it makes the layout trivially reproducible from state, at the cost
of some geometric ambiguity explored below.

## Workspaces and multi-output

Nine workspaces, switchable and movable-to from the keyboard. Once
multi-monitor entered the picture, tiling became **per-output**: each
`Output` tracks which workspace it's showing, `refresh()` hides any
workspace not displayed on any output and tiles each output's workspace
within that output's own box, and switching to a workspace already shown on
another monitor **swaps** the two outputs' workspaces rather than showing
one workspace on two screens at once. New outputs claim the lowest-numbered
free workspace, and get **focus-follows-monitor**: new windows open on
whichever monitor the cursor is currently over.

Per-output monitor **position and scale** are config-driven
(`monitor = NAME, XxY[, SCALE]` in `0xin.conf`) — an output with no
matching config entry keeps wlroots' default auto-placement. This was kept
deliberately simple: explicit pixel coordinates per named connector, no
relative-position keywords, no DPI-based auto-scale heuristic — the config
author computes and writes the offsets themselves.

## Config file

`src/config.rs` is a dependency-free, line-based parser — no external crate
— for `key = value` lines plus a compact `bind = MODS, KEY, ACTION[, ARG]`
syntax. Keybinding config merges with, rather than
replaces, the built-in defaults: `Config::load()` always seeds the full
default bind set first, then each `bind` line in the user's config overrides
just that one key combination and leaves every other default in place — so
a config with two or three `bind` lines still has working workspace
switches, close, and quit. An unparseable line warns on stderr and is
skipped; nothing in config parsing is ever fatal to startup.

## Keybindings: from cyclic to spatial

Window navigation went through a real design change mid-stage. It started
as cyclic `Mod+J`/`Mod+K` (next/previous in list order) and was replaced
with **spatial** `Mod+H/J/K/L` — focus or move to whichever tiled window is
actually left/down/up/right of the current one on screen, no wraparound.

The first implementation picked a directional neighbor by nearest
center-point (weighted toward the primary axis). That worked for two or
three windows but broke down at four or more: because the spiral layout can
produce one large window opposite several smaller stacked ones, center-
distance could pick a window with **no actual shared border** — pressing
`Up` from a bottom-right pane could land in a large far-left pane instead of
the pane directly above it, and the relation wasn't even symmetric (`Right`
from A could reach C, without `Left` from C reaching back to A). The fix,
confirmed with a real computed spiral fixture rather than hand-derived
geometry, switched to an **edge/overlap-based** heuristic (i3/sway-style):
prefer whichever candidate shares the largest overlapping border on the axis
perpendicular to the movement direction, falling back to center-distance
only when nothing overlaps at all. One residual limitation is understood and
accepted rather than silently swept under the rug: two windows that touch at
only a single corner point (not a real shared edge) can't be made fully
reversible by any geometric heuristic on a flat, list-order-driven layout —
fixing that fully would mean representing the layout as an explicit
split-tree instead, out of scope for now.

## Status

**Done**, in the sense of meeting and exceeding the original deliverable —
tiling, workspaces, config, and keybindings are all in daily use — but this
stage is the one most likely to keep growing (more layouts, resize, floating
exceptions) rather than being considered permanently closed the way Stages
0–3 are.
