# Stage 22 — Hold Bindings & Session Exit

**What it is.** Physical keys can trigger an action only after remaining
pressed for a configured duration, and `0xinctl quit` provides a clean,
compositor-owned session exit.

**Gate:** *Holding the FP5 power key for two seconds opens a fuzzel session
menu. Releasing it early does nothing. Choosing logout returns to Phrog.*

## Hold mappings

```ini
hold = , XF86PowerOff, 2000, spawn, session-menu
```

The format is `MODS, KEY, MILLISECONDS, ACTION[, ARG]`. Durations from 100
through 60000 milliseconds are accepted. A press arms a Wayland event-loop
timer; release cancels it. Both events are consumed so applications never see
an unmatched power-key press or release.

This mechanism is general input mapping and is not FP5-specific. The FP5
profile supplies the hardware policy: a two-second power-key hold launches
`~/.local/bin/0xin-session-menu`. Short press is deliberately reserved for a
future secure screen-lock action.

## Clean session exit

```sh
0xinctl quit
```

The control request asks 0xin to terminate its Wayland display loop normally.
The FP5 wrapper then exits and greetd/Phrog regains control. This is the
supported interactive equivalent of terminating 0xin from an SSH recovery
shell.

The included FP5 menu is a small fuzzel dmenu script with logout, reboot,
shutdown, and cancel choices. Reboot and shutdown call `systemctl` so logind
and the system policy layer remain responsible for authorization. The script
replaces the earlier Hyprland-specific `hypr-phone-menu`, whose logout action
depended on `hyprctl dispatch exit`.

## Locking boundary

Short press is not mapped to a fake lock. A secure lock needs a lock protocol,
an authentication surface that cannot be bypassed by ordinary clients, and
correct input/focus handling. Until that exists, leaving short press unused is
safer than presenting window hiding as device security.
