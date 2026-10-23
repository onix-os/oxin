# Running & verifying

Building a compositor raises an obvious problem: how do you check that a
change actually works, when the thing you built *is* the environment
everything else renders inside? This chapter is less "here's the command"
(the [README](https://github.com/onix-os/oxin#readme) covers that) and more
about the verification habits that came out of building 0xin without a
synthetic-input tool available in the dev environment (no `wtype`/`ydotool`)
— every recipe here exists because "just try it and see" wasn't always
possible.

## Nested first, always

The nested Wayland backend — `cargo nested` — is the primary loop for a
reason: it's the only mode where 0xin runs *inside* something that can
already show you its output, with no modesetting, no VT, no real display
hardware involved. Every stage was built and mostly verified nested before
it was ever tried on a real TTY.

Two things about the nested backend are easy to get bitten by:

- **It only receives keys when the host compositor gives its window focus.**
  A "keybinding does nothing" bug is, more often than not, a focus bug in
  the *host* desktop, not in 0xin.
- **The host may already own the modifier you want.** `OXIN_MOD=alt`
  exists because the host compositor grabs Super-chords before a nested
  0xin window ever sees them.

## Verifying without synthetic input

With no way to script a keypress or click into the agent loop, verification
leans on two things instead:

1. **A real client's own behavior as a signal.** `wayland-info` connecting
   and listing 0xin's globals (`wl_shm`, `zwp_linux_dmabuf_v1`,
   `wl_compositor`, `wl_data_device_manager`, ...) proves the Wayland server
   is actually up, independent of anything visual. A real app (`foot`,
   `kitty`) refusing to start with "no seats available" is exactly as
   informative as it succeeding — it's how the missing-`wl_seat` gap at
   Stage 3 was caught.
2. **Log markers plus a screenshot**, for anything visual:
   ```sh
   target/debug/0xin >/tmp/0xin.log 2>&1 &
   PID=$!; sleep 3
   grim /tmp/0xin.png     # screenshot the host screen, including 0xin's window
   kill $PID
   ```
   wlroots' debug log (on via `oxide_log_init()`) is very verbose — the
   first few hundred lines are EGL/DMA-BUF format enumeration — so the
   useful signal is 0xin's own `println!` markers plus lines like
   `Allocated ... GBM buffer` / `DMA-BUF imported`, read alongside the PNG.

Keyboard *wiring* (as opposed to actual typing) is verified the same way —
log markers like "keyboard attached" / "keyboard focus -> toplevel" confirm
the plumbing is connected; actually typing into a client is checked by hand,
by focusing the nested window on the host and typing.

## Config, without a window

The Lua config doesn't need a display to verify at all. Point
`XDG_CONFIG_HOME` at a directory holding `0xin/init.lua` and run headless:

```sh
mkdir -p /tmp/cfg/0xin && cp init.lua.example /tmp/cfg/0xin/init.lua
XDG_CONFIG_HOME=/tmp/cfg WLR_BACKENDS=headless target/debug/0xin >log 2>&1 &
grep -E '^0xin:' log
```

A config that raises is reported with its file and line and 0xin starts on the
built-in defaults, so the grep above is the fastest way to confirm a
hand-edited `init.lua` did what you meant. `0xinctl config-error` asks the
running compositor the same question — useful when the log has scrolled away,
and the only way to tell a session that silently fell back to defaults from
one that loaded fine.

`print()` from a config goes to stderr as `0xin: lua: …`, which is usually
quicker than reasoning about what a setting resolved to.

Once 0xin is running, there is no need to restart it to try a change: saving
the file reloads it ([Stage 31](phases/stage-31-live-reload.md)), and the log
says `0xin: reloaded …` or prints the error and `keeping the previous config`.
`0xinctl reload` does the same on demand and answers `ok` or the error, which
makes it the scriptable form:

```sh
sed -i 's/oxin.gap = .*/oxin.gap = 20/' /tmp/cfg/0xin/init.lua   # reloads on its own
WAYLAND_DISPLAY=$SOCK 0xinctl reload                             # or explicitly
```

To check whether a problem is yours or a plugin's, start without them:

```sh
OXIN_NOPLUGIN=1 target/debug/0xin      # or: target/debug/0xin --noplugin
```

## Visual checks, headless

The headless backend still composites, and 0xin's screencopy works against it,
so `grim` can screenshot a session that never opens a window anywhere. Point
clients at 0xin's own socket (it logs `WAYLAND_DISPLAY=…` once ready), and use
quickshell as a scriptable test client: a `.qml` file can declare toplevels
(`FloatingWindow`), bars on any layer (`PanelWindow`), menus (`PopupWindow`),
and a `Timer` to change them mid-run.

```sh
XDG_CONFIG_HOME=/tmp/cfg WLR_BACKENDS=headless OXIN_NOPLUGIN=1 \
    target/debug/0xin >log 2>&1 &
until SOCK=$(grep -o 'WAYLAND_DISPLAY=wayland-[0-9]*' log | cut -d= -f2) && [ -n "$SOCK" ]; do
    sleep 0.1
done
WAYLAND_DISPLAY=$SOCK QT_QPA_PLATFORM=wayland qs -p scene.qml &
sleep 3; WAYLAND_DISPLAY=$SOCK grim shot.png
```

Add `WAYLAND_DEBUG=server` to 0xin's environment to see the protocol exchange
itself, which is how Stage 30 confirmed menus were really being repositioned
rather than recreated.

## Multi-output, nested

The nested Wayland backend honors `WLR_WL_OUTPUTS=2`, opening two host
windows — enough to verify per-output tiling, focus-follows-monitor, and
(later) config-driven monitor position/scale without touching real hardware:

```sh
WLR_WL_OUTPUTS=2 OXIN_MOD=alt target/debug/0xin foot
```

Two `output <name> online @ X,Y WxH — workspace N` log lines confirm both
outputs came up and where the layout placed them.

## Real hardware

On a bare TTY, `wlr_backend_autocreate` picks the DRM/KMS backend instead
(no `WAYLAND_DISPLAY` to detect). The one recurring hardware quirk worth
knowing before you hit it: on a machine with two GPUs, wlroots may pick the
wrong `/dev/dri/cardN` — `WLR_DRM_DEVICES=/dev/dri/cardN` forces the right
one.

VT switching (`Ctrl+Alt+F1..F12`) is its own verification loop: switch away,
switch back, and watch for a clean repaint rather than a black/frozen
screen. That specific failure mode — outputs come back black after a VT
resume — is what drove the session-active-signal handling described in
[Stage 6](phases/stage-6-real-display.md); a forced repaint on the first few
frames after resume is the fix, and the regression test for it is "does the
screen come back," not something a unit test can cover.
