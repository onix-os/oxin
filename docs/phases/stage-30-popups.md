# Stage 30 — Popups and Menus

**What it is.** `xdg_popup` surfaces — right-click menus, dropdowns, tooltips,
a bar's tray menu — drawn, placed, and kept on screen, for application windows
and for layer-shell surfaces alike.

**Gate:** *An application's menu and its submenu appear where the client put
them; a menu anchored past a screen edge is moved back onto it; a menu from a
bottom-layer bar draws over the tiled windows it drops across; and none of it
can take the compositor down when a client opens, moves, or abandons a menu.*

## Every menu was invisible

wlroots' scene helper `wlr_scene_xdg_surface_create` covers a surface "and all
of its sub-surfaces". A popup is not a sub-surface: it is a separate
`xdg_surface` with its own role. So 0xin configured popups (they mapped, and
the client believed its menu was open) and nothing was ever drawn.

Each popup now gets its own scene node, created in `shim/xdg_shell.c`'s
`new_popup` handler under its parent's tree. The parent is a toplevel for a
first-level menu and another popup for a submenu, so the parent's tree is
carried on `wlr_xdg_surface.data`: set for toplevels in
`oxide_scene_add_xdg_toplevel`, and for every popup as its node is made. A
submenu finds its parent menu's tree the same way a menu finds its window's.

## Kept on screen

A popup arrives with a positioner: an anchor rectangle on its parent, a size,
and the client's rules for what to do if that does not fit (flip, slide,
resize). wlroots applies those rules only when the compositor asks, with
`wlr_xdg_popup_unconstrain_from_box` and the output's box. Without that, a menu
opened near the right edge simply runs off it.

Three details mattered:

- **Not at creation.** Unconstraining ends in
  `wlr_xdg_surface_schedule_configure`, which asserts the surface is
  initialized, and that is only true after its first commit. Called from
  `new_popup` it aborts the compositor. It runs from the popup's initial
  commit instead.
- **The box is in the root surface's coordinates.** wlroots walks a submenu's
  chain up to the root itself, so the box has to be relative to the root (the
  toplevel or the layer surface), not to the immediate parent. And it is
  relative to the root's *surface* origin, not its window-geometry origin: 0xin
  positions a toplevel's tree at its window geometry, so the geometry offset (a
  client-drawn shadow, say) is added back.
- **Again on every reposition.** `xdg_popup.reposition` (xdg-shell v3) lets a
  client move an open menu without tearing it down. wlroots recomputes the
  geometry, but the constraint rules are once again applied only on request,
  so each reposition re-runs the unconstrain.

## Layer-shell menus, and a NULL parent

A bar's popup is created through `xdg_surface.get_popup` with **no parent**.
It is given one afterwards by `zwlr_layer_surface_v1.get_popup`. wlroots still
emits xdg-shell's `new_popup` for it in between, with `popup->parent == NULL`,
and `wlr_xdg_surface_try_from_wlr_surface` does not accept NULL: its first
instruction reads the surface's role. The first version of the popup scene code
passed the parent straight through, so the first menu any bar opened segfaulted
the session. That was reproduced headless with a quickshell `PopupWindow` before
it was fixed.

The xdg-shell handler now leaves a parentless popup alone. The layer surface
raises its own `new_popup` once `get_popup` has attached the parent, and
`shim/layer_shell.c` places the popup from there. `oxide_layer_surface_setup_popups`
is called once per layer surface, right after its scene node is made. Its state
hangs off `wlr_layer_surface_v1.data`, which wlroots leaves to the compositor,
and that same pointer lets the unconstrain code find a bar's position and output
when the root of a popup chain turns out to be a layer surface.

### Lifted above the windows

A menu placed under its layer surface's own tree is painted in that layer. For
a bar on the *bottom* layer (waybar's default), that is underneath the tiled
windows the menu drops over. So for background and bottom surfaces, Rust
(`src/layer_shell.rs`) hands the shim the top-layer tree instead. Each popup
gets a holder node there, positioned at the layer surface's origin, with the
popup's own scene node inside it. Submenus nest inside that node and follow it.
Holders are resynced whenever the layer surface is configured, so a bar that
moves (an output change, an exclusive zone elsewhere) takes its open menus with
it. Top and overlay surfaces skip the holder; their menus already paint above
windows and follow the surface for free.

### Destroy order is not yours

The holder belongs to the popup, not to the layer surface: wlroots does not
promise which of the two goes first. The per-surface state keeps the
`wlr_layer_surface_v1` pointer directly rather than reading it back through
the `wlr_scene_layer_surface_v1`. The scene helper frees itself on the same
`destroy` signal and its listener is registered first, so by the time 0xin's
runs, `scene_ls` is gone. Hiding a bar with its menu open hit exactly this
use-after-free in the headless test below before the pointer was moved.

## How it was verified

Headless, with no display and no input, per [Running & Verifying](../running.md):
0xin on `WLR_BACKENDS=headless`, a quickshell config, and `grim` against the
headless socket. The scene had a `FloatingWindow` (tiled full-screen) with a
menu and submenu, a top-layer bar with a menu anchored at x = 1200 on a
1280-wide output, and a bottom-layer bar with a menu over the window. A `Timer`
then moved the edge menu and hid the bottom bar with its menu open.

- Before the fix, the parentless popup segfaulted 0xin (exit 139) as soon as
  quickshell created it.
- After it, the screenshot showed every menu drawn, the edge menu slid back to
  x = 1080, and the bottom bar's menu painted over the window.
- `WAYLAND_DEBUG=server` showed real `xdg_popup.reposition` requests, each
  answered by a constrained `configure`, and the hidden bar's popup destroyed
  cleanly. 0xin stayed up through the bar's teardown and the client's exit.

## Boundaries

- **Menus follow their bar on configure, not on every frame.** A bottom-layer
  bar that moved without being configured would leave a lifted menu behind.
  Layer surfaces are only moved by configures, so this is theoretical.
- **A surface moved between layers keeps its popups' tree.** `set_layer` on a
  live surface is rare, and its menus are short-lived.
- **Popups whose parent is neither an xdg surface nor a layer surface** (none
  exist in the protocols 0xin implements) are configured so the client is not
  left waiting, but given no scene node. An unplaced menu is better than one
  attached to the wrong part of the scene.
