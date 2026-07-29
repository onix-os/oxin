# 0xin

**A from-scratch tiling Wayland compositor, written in Rust on top of [wlroots](https://gitlab.freedesktop.org/wlroots/wlroots).**

0xin is a personal, learning-first compositor built directly on wlroots 0.19 rather
than on top of any desktop. It's a **dynamic tiling** compositor — windows are arranged
automatically to fill the screen instead of floating and overlapping.

> **Status:** early but real. It runs nested inside another Wayland session for
> development, and on actual hardware as a real DRM/KMS session on a TTY. It is being
> grown into something to daily-drive, one capability at a time.

## What works now

- **Spiral / dwindle tiling** — each new window splits the remaining space, alternating
  vertical (left/right) then horizontal (top/bottom).
- **9 workspaces** — switch between them and move windows across them from the keyboard.
- **Multi-monitor** with **focus-follows-monitor** — new windows open on the monitor
  your cursor is on; each monitor shows its own workspace. Position/scale per
  named output are configurable (`monitor =` lines), otherwise auto-placed.
- **Keyboard-driven**, configured by a small **Rust-parsed config file** (modifier,
  gaps, background colour, keybindings, terminal command).
- **Pointer + cursor** with click-to-focus.
- Runs real **xdg-shell apps** (terminals, browsers, …).
- Runs on a **real TTY** via libseat/logind, and **survives VT switching**
  (Ctrl+Alt+Fn away and back) without crashing or losing your windows.
- **Layer-shell** (`wlr-layer-shell-unstable-v1`) — bars, panels and wallpaper (e.g.
  [quickshell](https://quickshell.org)) render in the correct z-order and reserve their
  screen space, so tiled windows never sit underneath them.
- **Server-side decorations** (`xdg-decoration-unstable-v1`) — 0xin always claims
  decoration, so clients don't draw their own title bar/border: bare, borderless windows.
- **Screenshots/screen recording** (`wlr-screencopy-unstable-v1` + `xdg-output`) — tools
  like `grim` and `wf-recorder` capture 0xin's real composited output directly.

## Docs

The full story — architecture, environment/toolchain, and a phase-by-phase
build log (Stage 0 through Stage 8, each with its deliverable and how it
actually went) — lives in an [mdBook](https://rust-lang.github.io/mdBook/)
under [`docs/`](docs/introduction.md), published at
**[termworks.github.io/0xin](https://termworks.github.io/0xin/)**.
Preview it locally with:

```sh
mdbook serve
```

## Architecture

The split is deliberate:

- **Rust owns all policy** — the window list, tiling layout, workspaces, keybindings,
  config parsing, and overall flow (`src/main.rs`, `src/config.rs`).
- **A thin C shim** (`shim/oxide_shim.{c,h}`) owns the parts that are awkward or
  unsafe to model through FFI: the wlroots `wl_listener`/`wl_signal` glue (intrusive
  linked lists) and anything that needs to read wlroots struct fields directly. It
  exposes clean `(userdata, data)` callbacks to Rust.
- **wlroots** is the C library doing the heavy lifting (DRM/KMS modesetting, the GLES2
  renderer, libinput, the scene graph, protocol plumbing). We bind to it with
  `bindgen` + the shim; we don't rewrite it.

In short: **wlroots = mechanism, 0xin = policy.** See
[`notes/architecture.md`](notes/architecture.md) for the full division of labour.

## Build

Built and run on **Arch Linux**. System dependencies:

```
wlroots0.19 wayland wayland-protocols libxkbcommon libinput libdrm seatd mesa pixman pkgconf clang
```

The Rust toolchain is pinned in `rust-toolchain.toml`. Then:

```sh
cargo build
```

The build script (`build.rs`) finds wlroots via `pkg-config`, generates the
`xdg-shell` protocol header with `wayland-scanner`, compiles the C shim, and runs
`bindgen` over `wrapper.h`.

## Run

### Nested (the fast dev loop)

Inside an existing Wayland session, 0xin opens as a window:

```sh
OXIN_MOD=alt cargo nested -- kitty
```

`cargo nested` is an alias for `cargo run`. `OXIN_MOD=alt` makes the modifier key
**Alt** instead of Super, because the host compositor grabs Super-chords before 0xin
sees them. The trailing `-- kitty` launches a test client against 0xin's socket.

### On a real display (TTY / DRM-KMS)

From a free virtual terminal (e.g. Ctrl+Alt+F5), logged in:

```sh
LIBSEAT_BACKEND=logind ~/Projects/0xin/target/debug/0xin kitty 2>~/0xin-tty.log
```

`LIBSEAT_BACKEND=logind` lets logind grant the active VT its devices (no `seat` group
needed). Here the modifier is the real **Super** key. Ctrl+Alt+F1 gets you back to your
main session. More detail and verification recipes are in
[`notes/running-and-verifying.md`](notes/running-and-verifying.md).

## Default keybindings

`Mod` is **Super** by default (**Alt** when running nested with `OXIN_MOD=alt`).

| Keys                | Action                              |
| ------------------- | ----------------------------------- |
| `Mod + Return`      | Open the terminal                   |
| `Mod + Q`           | Close the focused window            |
| `Mod + Shift + Q`   | Quit 0xin                        |
| `Mod + H/J/K/L`     | Focus the window left/down/up/right |
| `Mod + Shift + H/J/K/L` | Move the focused window left/down/up/right |
| `Mod + F`           | Toggle fullscreen for the focused window |
| `Mod + V`           | Toggle floating for the focused window |
| `Mod + left-drag`   | Move a floating window              |
| `Mod + right-drag`  | Resize a floating window            |
| `Mod + 1…9`         | Switch to workspace 1–9             |
| `Mod + Shift + 1…9` | Move focused window to workspace 1–9 |
| `Ctrl + Alt + F1…F12` | Switch virtual terminal           |

## Configuration

0xin reads `~/.config/0xin/0xin.conf` (or `$XDG_CONFIG_HOME/0xin/0xin.conf`).
With no config file it uses the built-in defaults above. The format is `key = value`
with `#` comments, plus `bind` lines. Binds always start from the defaults above;
each `bind` line in your config overrides just that key combination and leaves every
other default bind in place — so a config with only a couple of `bind` lines still
has working workspace switches, close/quit, etc:

```
modifier   = super
gap        = 10
background = 0.0 0.6 0.6

bind = MOD, Return, spawn, kitty
bind = MOD, Q, close
bind = MOD SHIFT, Q, quit
bind = MOD, H, movefocus, l
bind = MOD SHIFT, H, movewindow, l
bind = MOD, 1, workspace, 1
bind = MOD SHIFT, 1, movetoworkspace, 1

# monitor = NAME, XxY[, SCALE] — explicit position for a named output
# (connector name, as logged: "output <name> online..."). Unlisted outputs
# keep the default auto-placement.
monitor = HDMI-A-1, 0x-1080, 1.0

# Optional virtual-keyboard controller. Actions can be mapped to keys or touch.
virtual_keyboard_show = pkill -USR2 -x wvkbd-mobintl
virtual_keyboard_hide = pkill -USR1 -x wvkbd-mobintl
virtual_keyboard_height = 125

gesture = bottom-up, keyboardshow
gesture = bottom-down, keyboardhide
gesture = edge-left-in, workspaceprev
gesture = edge-right-in, workspacenext

# The same actions are available on non-touch devices.
bind = MOD, bracketleft, workspaceprev
bind = MOD, bracketright, workspacenext
bind = MOD, K, keyboardtoggle

# Modifier-free media buttons are ordinary input mappings too.
bind = , XF86AudioRaiseVolume, spawn, fuzzel
bind = , XF86AudioLowerVolume, keyboardtoggle
```

A line 0xin can't parse is warned about on stderr and skipped — never fatal. See
[`0xin.conf.example`](0xin.conf.example) for the full annotated example.

## Repository layout

| Path                      | What it is                                                |
| ------------------------- | --------------------------------------------------------- |
| `src/main.rs`             | Compositor orchestrator + all policy (layout, workspaces, input, keybindings) |
| `src/config.rs`           | Dependency-free config-file parser                        |
| `shim/oxide_shim.{c,h}` | Thin C shim: wlroots listener glue + struct access        |
| `build.rs`, `wrapper.h`   | The FFI pipeline (pkg-config, wayland-scanner, cc, bindgen) |
| `notes/`                  | Architecture, toolchain, and run/verify notes (working reference) |
| `docs/`, `book.toml`      | The mdBook doc site source — narrative chapters + phase build log |
| `KICKOFF.md`              | The project's mission and learning-first working rules    |

---

0xin is a personal, learning-first project — built concept-by-concept with every
file and function understood rather than assembled. Its working rules live in
[`KICKOFF.md`](KICKOFF.md). No license yet!
