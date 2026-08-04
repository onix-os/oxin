# Fairphone 5 touch test

This profile is intentionally temporary: it starts 0xin directly on DRM/KMS
with Patin and a gesture-controlled wvkbd, without text-input-driven automatic
keyboard activation or autologin. Phosh and Hyprland remain separate sessions.

The wrapper now starts only 0xin. Session clients are declared in the profile's
`config/0xin/0xin.conf` with repeated `exec_once` lines: Patin and wvkbd. Each
is launched once per compositor process after `WAYLAND_DISPLAY` is ready. Edit
those lines to change the shell/session composition without rewriting the
session wrapper.

## Wallpaper

0xin renders wallpapers internally; the FP5 does not need swaybg, Hyprpaper,
or another layer-shell wallpaper process. Add a persistent image to
`config/0xin/0xin.conf`:

```ini
wallpaper = ~/Pictures/wallpaper.jpg
```

PNG and JPEG images use cover scaling. From a terminal inside the running 0xin
session, change or clear it immediately:

```sh
install -m 0755 ~/proj/0xin/target/debug/0xinctl ~/.local/bin/0xinctl
0xinctl wallpaper ~/Pictures/another.png
0xinctl wallpaper clear
```

The live command does not rewrite the configuration. The configured image
returns after the next login unless its `wallpaper =` line is changed too.

## Auto-rotate

`0xinctl rotate NAME normal|90|180|270` rotates a live output without
restarting 0xin — it re-commits the output's transform, re-tiles every
window, and resizes the background/wallpaper/gesture handle to match. On the
FP5, `profiles/fp5/bin/0xin-auto-rotate` drives this automatically: it claims
the accelerometer from `iio-sensor-proxy` (`net.hadess.SensorProxy` on the
system D-Bus) and calls `0xinctl rotate DSI-1 ...` whenever
`AccelerometerOrientation` changes. It is launched by the profile's
`exec_once = ~/.local/bin/0xin-auto-rotate` line, so install it alongside the
other helpers:

```sh
install -m 0755 ~/proj/0xin/profiles/fp5/bin/0xin-auto-rotate ~/.local/bin/
install -m 0755 ~/proj/0xin/target/debug/0xinctl ~/.local/bin/0xinctl
```

Touch input is mapped to the output (`wlr_cursor_map_input_to_output`) so it
tracks the rotation too — this only happens automatically when a profile has
exactly one output, so desktop/multi-monitor setups are unaffected.

## Window opacity

The FP5 profile sets `window_opacity = 0.8`, so application windows reveal
some of the wallpaper. This is general 0xin configuration rather than
phone-specific behavior: other profiles can use any value from `0.0`
(invisible) to `1.0` (fully opaque, the default). Layer-shell UI such as Patin
and wvkbd remains fully opaque.

## Camera storage

GNOME Snapshot follows the XDG Pictures directory and creates its `Camera`
subdirectory there. The FP5 test account uses:

```sh
xdg-user-dirs-update --set PICTURES "$HOME/pics"
```

New captures therefore go to `~/pics/Camera` instead of `~/Pictures/Camera`.

Build 0xin in `~/proj/0xin`, then install the chooser entry:

```sh
sudo install -m 0644 \
  ~/proj/0xin/profiles/fp5/0xin-touch-test.desktop \
  /usr/share/wayland-sessions/0xin-touch-test.desktop
```

Confirm SSH works before logging out. Select **0xin Touch Test** in Phrog.
End the test from another machine:

```sh
ssh fp5 'pkill -TERM -x 0xin'
```

The wrapper exits with 0xin, returning control to greetd. Remove the temporary
entry when it is no longer needed:

```sh
sudo rm /usr/share/wayland-sessions/0xin-touch-test.desktop
```

## Power button and logout

Holding the power button for two seconds opens the profile's fuzzel session
menu. It offers **Log out to Phrog**, **Reboot**, **Shut down**, and
**Cancel**. Logout calls `0xinctl quit`, which terminates the Wayland display
cleanly and lets the session wrapper return to greetd. Reboot and shutdown use
systemd-logind through `systemctl`, so authorization follows the active local
session.

Install the profile helper and current control client:

```sh
install -m 0755 ~/proj/0xin/profiles/fp5/bin/0xin-session-menu ~/.local/bin/
install -m 0755 ~/proj/0xin/target/debug/0xinctl ~/.local/bin/0xinctl
```

A shorter power-button press launches the independently installed,
touch-capable Patin lock client; reaching the two-second hold threshold launches
the session menu instead. The `pgrep` guard prevents another locker from being
started while Patin's supervisor or lock worker is already running.

```ini
bind = , XF86PowerOff, spawn, pgrep -x patin-lock >/dev/null || patin-lock
hold = , XF86PowerOff, 2000, spawn, ~/.local/bin/0xin-session-menu
```

`patin-lock` owns its touch password keyboard because an ordinary layer-shell
wvkbd cannot be placed above the lock surface without weakening the lock
boundary. Install and validate Patin's binary and PAM policy before enabling
this profile.

Swipe upward from the bottom-center gesture area to show wvkbd. The FP5 profile
sets `gesture_handle = hidden`, so this target has no visible pill. To hide
wvkbd, make a deliberate downward swipe on the keyboard that reaches the
bottom edge. Ordinary taps reach wvkbd first and remain keys; only the completed
swipe is claimed by 0xin, matching SXMO's bottom-edge gesture style.

Swipe inward from the left edge for the previous workspace or from the right
edge for the next workspace. The nine-workspace ring wraps at either end.
Touches must begin within the narrow edge activation strip; horizontal gestures
started elsewhere remain application input. When wvkbd is visible, these strips
stop at its top edge so the keyboard's left- and right-edge keys remain usable.

Swipe horizontally in the thin top-edge strip to adjust display brightness:
left-to-right increases it, and right-to-left decreases it. Every 5% of output
width crossed dispatches the profile's 5% `brightnessctl` step, so an
edge-to-edge swipe spans approximately the full 0–100% range and shorter
swipes adjust proportionally. The recognizer itself contains no FP5 backlight
path or brightness policy.

Swipe downward from the top edge to open the temporary Fuzzel application
menu. An upward swipe that travels at least 70 logical pixels and reaches the
top edge closes it. These are the `top-down` and `to-top` triggers; the profile
maps them to a duplicate-safe Fuzzel launch and a targeted Fuzzel termination.

Window management is available directly on the central application surface:

- Swipe with two fingers in any direction to swap the focused tiled window
  with its spatial neighbor in that direction.
- Swipe up or down with three fingers to close the focused window.
- Swipe left with three fingers to send the focused window to the previous
  workspace; swipe right to send it to the next workspace. The workspace
  ring wraps at either end, and the display stays on its current workspace.

The first finger initially remains application input. When the second finger
arrives, 0xin cancels that client touch sequence and owns the multi-finger
gesture. All participating fingers must move in the chosen direction, which
avoids triggering from a stationary second or third tap.

These meanings come from the profile's `gesture = TRIGGER, ACTION` mappings,
not hardcoded phone policy. The same `workspacenext`, `workspaceprev`, and
virtual-keyboard actions can be assigned to ordinary `bind =` keyboard chords
on desktops and convertibles.

The hardware-button mappings are:

- **Volume up:** increase the default audio sink by 5%.
- **Volume down:** decrease the default audio sink by 5%.

The profile binds the standard xkb names `XF86AudioRaiseVolume` and
`XF86AudioLowerVolume`; it does not depend on FP5 input-device paths or raw
event codes. Other Wayland devices whose buttons expose those standard key
symbols can use the same mappings after selecting an appropriate audio-control
command. These are currently single-press actions.
