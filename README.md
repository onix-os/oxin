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
- **Keyboard-driven**, configured and scripted in **Lua** (`~/.config/0xin/init.lua`)
  through an embedded pure-Rust interpreter: settings are assigned, keybindings and
  behaviour are registered, and a binding can be a Lua function. A broken config is
  reported with its line and 0xin starts on defaults anyway.
- **Plugins** — a neovim-shaped runtimepath (`plugin/` runs, `lua/` is required,
  `after/` last), so config somebody else wrote drops in as a directory.
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
- **Real display power-off** (`wlr-output-power-management-unstable-v1`) — a client
  (e.g. [Patin](https://github.com/termworks/patin)'s `patin-lock`) can request an
  actual DPMS on/off per output, distinct from the opaque lock cover below.

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
  config, and overall flow (`src/main.rs`, `src/config/`, `src/lua/`).
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

### Install

`cargo build` leaves the binaries in `target/` and nothing else. `make install` puts
them where a system expects them, using Hyprland's layout:

```sh
make               # cargo build --release
sudo make install  # PREFIX=/usr/local
```

That places `0xin` and `0xinctl` in `$PREFIX/bin` and the session entry
(`dist/0xin.desktop`) in `$PREFIX/share/wayland-sessions/`, so a display manager offers
**0xin** as a session to log into. `PREFIX=/usr` and `DESTDIR=` are honoured for
packaging, and `make uninstall` removes exactly what was installed.

Cargo is still the build system; the Makefile exists only to place the files cargo
cannot — `cargo install` handles binaries alone, and only into `~/.cargo/bin`.

### Prebuilt releases

Each [release](https://github.com/onix-os/oxin/releases) carries
`0xin-linux-arm64-musl.tar.gz`, built on Alpine for aarch64 — for devices that cannot
build 0xin themselves, such as a postmarketOS phone (see
[`profiles/fp5/`](profiles/fp5/README.md)). Unpack it and run the included
`install.sh`, which installs the same files to the same places as `make install`.

### With Nix

A flake is committed, so 0xin builds without any of the above on a machine that has
Nix — and can be declared as a session on NixOS. The Rust version is not repeated in
`flake.nix`; it is read out of `rust-toolchain.toml`, so both builds use the same
compiler.

```sh
nix build     # the compositor, 0xinctl, and a wayland-sessions entry
nix develop   # wlroots 0.19, clang and the pinned toolchain, then cargo
nix flake check
```

On a non-NixOS host the Nix-built binary builds but won't render — it links Nix's
libglvnd and can't see the system's GPU driver. Build with Nix, run with `cargo`; on
NixOS the module turns on `hardware.graphics` and it runs as a real session.

On NixOS, import the module and the session appears in your display manager:

```nix
{
  inputs.oxin.url = "github:onix-os/oxin";

  # in configuration.nix
  imports = [ inputs.oxin.nixosModules.default ];
  programs.oxin = {
    enable = true;
    extraPackages = [ pkgs.kitty ];   # Mod+Return spawns kitty by default
    config = ''
      local oxin = require("oxin")
      oxin.gap = 10
    '';
  };
}
```

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
LIBSEAT_BACKEND=logind ~/proj/0xin/target/debug/0xin kitty 2>~/0xin-tty.log
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

0xin reads `~/.config/0xin/init.lua` (or `$XDG_CONFIG_HOME/0xin/init.lua`), which
is ordinary Lua run by an embedded interpreter. With no config file it uses the
built-in defaults above.

The shape is: **assign the settings, register the behaviour, return nothing.**
Bindings are keyed by chord, so yours override the built-ins one at a time and
every unmentioned default stays active — a two-line config still has working
workspace switches, close and quit.

```lua
local oxin = require("oxin")

-- Set the modifier before any MOD+ binding: chords resolve MOD as written.
oxin.modifier       = "super"
oxin.gap            = 10
oxin.background     = { 0.0, 0.6, 0.6 }
oxin.wallpaper      = "~/Pictures/wallpaper.jpg"
oxin.window_opacity = 1.0
oxin.corner_radius  = 0

-- Explicit position for a named output (connector name, as logged:
-- "output <name> online..."). Unlisted outputs keep auto-placement.
oxin.monitors["HDMI-A-1"] = { x = 0, y = -1080, scale = 1.0 }

oxin.keys["MOD+Return"]  = oxin.action.spawn("kitty")
oxin.keys["MOD+q"]       = oxin.action.close
oxin.keys["MOD+SHIFT+q"] = oxin.action.quit
oxin.keys["MOD+h"]       = oxin.action.focus("left")
oxin.keys["MOD+SHIFT+h"] = oxin.action.move("left")

-- It is a real language, so nine workspaces take two lines.
for i = 1, 9 do
  oxin.keys["MOD+" .. i]       = oxin.action.workspace(i)
  oxin.keys["MOD+SHIFT+" .. i] = oxin.action.move_to_workspace(i)
end

-- A chord can have both a tap and a hold; they are separate tables.
oxin.keys["XF86PowerOff"] = oxin.action.spawn("swaylock")
oxin.hold["XF86PowerOff"] = { ms = 2000, action = oxin.action.spawn("session-menu") }

-- Assigning nil removes a binding, including a built-in one.
oxin.keys["MOD+f"] = nil

-- A binding can be a function, and then it can do anything Lua can.
oxin.keys["MOD+g"] = function()
  oxin.gap = (oxin.gap == 0) and 12 or 0
end

-- Optional virtual-keyboard controller. Focused clients using Wayland
-- text-input-v3 invoke the same provider-neutral commands automatically.
oxin.keyboard = {
  show   = "pkill -USR2 -x wvkbd-mobintl",
  hide   = "pkill -USR1 -x wvkbd-mobintl",
  height = 125,
}
oxin.gesture_handle = "hidden"   -- visible (default) | hidden; the visual pill only

-- Touch gestures reach the same actions.
oxin.gestures["bottom-up"]     = oxin.action.keyboard_show
oxin.gestures["edge-left-in"]  = oxin.action.workspace_prev
oxin.gestures["edge-right-in"] = oxin.action.workspace_next
oxin.gestures["top-right"]     = oxin.action.spawn("brightnessctl set +5%")
oxin.gestures["three-left"]    = oxin.action.move_to_workspace_prev
oxin.gestures["double-tap"]    = oxin.action.solo

-- Behaviour: handlers can be registered repeatedly and run in order. If one
-- raises it is reported and the rest still run.
local function autostart(cmd)
  oxin.on.startup(function() oxin.spawn(cmd) end)
end
autostart("patin")

oxin.on.window(function(w)
  -- nil = not mine, carry on. A table = do this instead.
  if w.app_id == "pavucontrol" then return { float = true } end
  if w.app_id == "firefox"     then return { workspace = 2 } end
end)
```

A config that raises is reported with its file and line and 0xin starts on the
built-in defaults — it never refuses to start. `0xinctl config-error` repeats the
message once you are in. A config that loops forever is stopped by a fuel budget
and a wall-clock deadline rather than hanging the session. See
[`init.lua.example`](init.lua.example) for the full annotated example.

### Plugins

Config somebody else wrote drops in as a directory, following neovim's layout: an
ordered runtimepath (`/etc/xdg/0xin`, then `~/.config/0xin`), each root with
`plugin/` (run at startup, alphabetically), `lua/` (only answers `require`) and
`after/plugin/` (runs last, so overriding a plugin is not editing it). Packages
live at `pack/<any>/start/<name>/`. Load order is `init.lua`, then `plugin/`, then
`after/`. One plugin raising does not stop the others, and `--noplugin` (or
`OXIN_NOPLUGIN=1`) starts without any.

PNG and JPEG wallpapers are decoded by 0xin itself and cover-scaled per output;
no external wallpaper program is required. Change or clear the running
wallpaper without restarting:

```sh
0xinctl wallpaper ~/Pictures/another.png
0xinctl wallpaper clear
```

Runtime changes last until 0xin exits. Set `wallpaper =` in the config to make
the selection persistent across sessions.

End the running compositor cleanly (and return to its login/session chooser)
with:

```sh
0xinctl quit
```

Query workspace state — which workspace each output is showing, and which
workspaces have at least one window — for building status bars and scripts:

```sh
0xinctl workspaces
```

```
ok
output DSI-1 1
workspace 1 occupied
workspace 2 empty
...
```

0xin implements `ext-session-lock-v1` for secure lock clients such as Patin's
touch-capable `patin-lock` or `swaylock`. Accepting a lock immediately covers
every output with an opaque
compositor fallback and routes input exclusively to the lock client. If that
client crashes without unlocking, the fallback remains and the desktop stays
inaccessible.

Application windows can reveal that background with `window_opacity`, where
`1.0` is fully opaque (the default) and `0.0` is fully transparent. The value
applies to XDG application toplevels on any supported Wayland device;
layer-shell surfaces such as panels, Patin, and virtual keyboards are left
fully opaque.

`corner_radius` (pixels, `0` = disabled, the default) rounds tiled/floating
window corners with real per-pixel masking — a compositor-owned GLES2 shader
renders each window through a rounded-rect mask, so it looks correct over a
wallpaper image, another window, or a reduced `window_opacity`, not just a
flat background color. Fullscreen windows are unaffected. This costs a real
extra GPU pass per commit of every masked window while enabled.

Because the mask is a GLES2 shader, it needs wlroots' **GLES2 renderer** —
wlroots picks a renderer on its own, and on a Vulkan or pixman session
`corner_radius` has no effect and says so once in the log. `WLR_RENDERER=gles2`
forces it. See [Stage 26](docs/phases/stage-26-rounded-window-corners.md) for
the full tradeoffs.

0xin also implements `wlr-output-power-management-unstable-v1`, so a client
can request a real DPMS power-off/on per output — independent of, and
usable alongside, the session-lock cover above. wlroots handles the wire
protocol; 0xin just applies the on/off it reports and forces a repaint when
an output comes back on so windows that were visible before power-off
reappear correctly.

## Repository layout

| Path                      | What it is                                                |
| ------------------------- | --------------------------------------------------------- |
| `src/main.rs`             | Compositor orchestrator + all policy (layout, workspaces, input, keybindings) |
| `src/config/`             | Config vocabulary, built-in keymap, key/modifier name resolution |
| `src/lua/`                | Embedded Lua: VM, the `oxin` module, registrars, events, plugins |
| `src/wallpaper.rs`        | Internal PNG/JPEG decoder + wlroots wallpaper buffers      |
| `src/control.rs`          | Local runtime-control socket                               |
| `src/bin/0xinctl.rs`      | Runtime control command                                    |
| `shim/oxide_shim.{c,h}` | Thin C shim: wlroots listener glue + struct access        |
| `build.rs`, `wrapper.h`   | The FFI pipeline (pkg-config, wayland-scanner, cc, bindgen) |
| `notes/`                  | Architecture, toolchain, and run/verify notes (working reference) |
| `docs/`, `book.toml`      | The mdBook doc site source — narrative chapters + phase build log |
| `KICKOFF.md`              | The project's mission and learning-first working rules    |

---

0xin is a personal, learning-first project — built concept-by-concept with every
file and function understood rather than assembled. Its working rules live in
[`KICKOFF.md`](KICKOFF.md). No license yet!
