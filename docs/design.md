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

The config is `~/.config/0xin/init.lua`, real Lua run by an embedded
interpreter ([luna](https://github.com/onix-os/luna) — stackless, pure Rust,
no C). Its shape: **assign the settings, register the behaviour, return
nothing.**

```lua
local oxin = require("oxin")

oxin.gap = 4                                       -- settings assigned
oxin.keys["MOD+Return"] = oxin.action.spawn("kitty")  -- behaviour registered

oxin.on.window(function(w)                         -- and reacted to
  if w.app_id == "pavucontrol" then return { float = true } end
end)
```

Three rules, unchanged in substance from the line-based format that came
before:

1. **Nothing is fatal.** A config that raises is reported with its file and
   line, and 0xin starts on the built-in defaults; `0xinctl config-error`
   repeats the message. A missing config means defaults. A config with no
   bindings still has every default binding. 0xin always starts — a
   compositor that refuses to is a black screen with nowhere to read the
   error.
2. **User config merges, never replaces.** `oxin.keys["MOD+q"] = …` overrides
   exactly that chord; every unmentioned default stays active. A two-line
   config is a two-line diff, not a fork of the whole keymap. Assigning `nil`
   removes a binding — including a built-in one, which the old format could
   not express at all.
3. **Explicit over implicit.** Monitor placement is literal pixel coordinates
   per named connector (`oxin.monitors["DP-1"] = { x = 0, y = 0 }`) — no
   relative keywords, no DPI auto-scale heuristics. The config states what
   happens; nothing else does.

Two properties fall out of the implementation rather than from discipline:

- **All-or-nothing.** Assignments land on a staged `Config` that is only
  adopted once the chunk reaches its end. There is no path on which a
  half-applied config reaches the compositor.
- **A runaway config cannot hang the session.** Lua runs on a fuel budget
  under a wall-clock deadline the host owns, so `while true do end` in a
  config costs one stutter and a log line rather than a wedged compositor.

Everything interpreter-shaped lives under `src/lua/`; `src/config/` holds only
the vocabulary, the built-in keymap and the name resolution the two share.
That boundary is deliberate — luna is pre-1.0 and says so, and it is what
keeps a breaking version bump a change to one directory.

## Plugins

A plugin is config somebody else wrote. 0xin follows neovim's model rather
than inventing one: an ordered **runtimepath** of roots (`/etc/xdg/0xin`, then
`~/.config/0xin`), each with `plugin/` (run at startup, alphabetically),
`lua/` (only answers `require`, never auto-run) and `after/plugin/` (runs
last, so overriding a plugin does not mean editing it). A package is a
directory laid out the same way under `pack/*/start/*` — installing one is
"put a directory here", with no registry and no manifest.

Load order is `init.lua`, then `plugin/`, then `after/plugin/`, matching
neovim. Failure policy differs by design: a raise in `init.lua` aborts the
whole load, while a raise in a plugin is reported and the others still load —
one plugin is not worth the rest of the session. `--noplugin` (or
`OXIN_NOPLUGIN=1`) starts without any of them, because the first question
when a compositor misbehaves is "is it me or a plugin?".

## Workspaces and outputs

Nine workspaces, with one invariant: **a workspace is never visible on two
outputs at once**. Switching to a workspace already shown on another monitor
swaps the two monitors' workspaces instead of duplicating it. New windows
open on the monitor the cursor is on (focus-follows-monitor). The model
stays predictable regardless of how many outputs are attached.

## Floating windows

Windows that shouldn't tile, don't: dialogs (a toplevel with a parent set —
file pickers, "Save as…"), windows that declare a fixed size, and anything
matched by an `oxin.on.window` rule returning `{ float = true }` open
floating instead — centered,
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

- **Runtime control** — the control socket exists (`0xinctl`), but its wire
  is line-based and one-request-per-connection. Giving it length-prefixed
  framing and exposing a named subset of the Lua API would let sibling tools
  query 0xin directly.
- **Exact tree-based directional navigation** — now that the split tree
  exists, `spatial_neighbor` could use its real adjacency instead of Stage
  5's geometric heuristic, fully resolving the corner-touch case. Not
  pursued in Stage 10 since it wasn't needed for the resize deliverable.

When one of these lands, it moves out of this list and into a stage chapter.
