# Stage 19 — Declarative Session Startup

**What it is.** Repeated `exec_once` entries in `0xin.conf` launch shell and
session clients after the compositor's Wayland socket is ready.

**Gate:** *The FP5 session wrapper launches only 0xin. The profile config
starts Patin and wvkbd declaratively and in declaration order.*

## Configuration

```ini
exec_once = ~/.local/bin/patin
exec_once = wvkbd-mobintl --hidden --no-popup -H 300 -L 200
```

Unlike scalar settings, `exec_once` is intentionally repeatable. Commands are
preserved in file order and started as separate child processes. “Once” means
once per 0xin process start; restarting the compositor creates a new session
and launches the commands again. Runtime config reload is not currently
implemented, so reload duplication is not a concern.

## Client environment

0xin waits until its backend has started and `WAYLAND_DISPLAY` points to its
new socket before launching startup commands. Commands use the same shell spawn
path as configured input actions, including:

- shell expansion and quoting;
- inherited session variables such as `XDG_CURRENT_DESKTOP`;
- removal of the compositor-only private `LD_LIBRARY_PATH`;
- restoration of normal child signal handling.

This makes startup clients peers of applications launched later through Patin
or input mappings, rather than wrapper-owned special cases.

## Profile versus compositor policy

The mechanism is generic compositor configuration. The particular FP5 command
list is reference-profile policy: other Wayland devices may start a different
shell, bar, notification daemon, wallpaper process, or no session clients at
all.

The dedicated `run-touch-test.sh` remains responsible for environment required
before the compositor exists—DRM/libinput backend selection, libseat, and the
private wlroots runtime path. It no longer embeds the user-session process
list.
