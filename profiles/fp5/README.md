# Fairphone 5 touch test

This profile is intentionally temporary: it starts 0xin directly on DRM/KMS
with Foot and a gesture-controlled wvkbd, without a bar, text-input-driven
automatic keyboard activation, or autologin. Phosh and Hyprland remain separate
sessions.

Build 0xin in `~/Projects/0xin`, then install the chooser entry:

```sh
sudo install -m 0644 \
  ~/Projects/0xin/profiles/fp5/0xin-touch-test.desktop \
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
left-to-right increases it by 5%, and right-to-left decreases it by 5%. The
profile implements these directions as `top-right` and `top-left` mappings
that spawn `brightnessctl`; the recognizer itself contains no FP5 backlight
path or brightness policy.

Swipe downward from the top edge to open the temporary Fuzzel application
menu. An upward swipe that travels at least 70 logical pixels and reaches the
top edge closes it. These are the `top-down` and `to-top` triggers; the profile
maps them to a duplicate-safe Fuzzel launch and a targeted Fuzzel termination.

Window management is available directly on the central application surface:

- Swipe with two fingers in any direction to swap the focused tiled window
  with its spatial neighbor in that direction.
- Swipe up or down with three fingers to close the focused window.
- Swipe left with three fingers to send the focused window to the next
  workspace; swipe right to send it to the previous workspace. The workspace
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
