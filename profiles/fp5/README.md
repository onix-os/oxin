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

The thin pill at the bottom is a compositor-owned gesture handle. Swipe upward
from it to show wvkbd. To hide wvkbd, make a deliberate downward swipe on the
keyboard that reaches the bottom edge. Ordinary taps reach wvkbd first and
remain keys; only the completed swipe is claimed by 0xin, matching SXMO's
bottom-edge gesture style.

Swipe inward from the left edge for the previous workspace or from the right
edge for the next workspace. The nine-workspace ring wraps at either end.
Touches must begin within the narrow edge activation strip; horizontal gestures
started elsewhere remain application input. When wvkbd is visible, these strips
stop at its top edge so the keyboard's left- and right-edge keys remain usable.

These meanings come from the profile's `gesture = TRIGGER, ACTION` mappings,
not hardcoded phone policy. The same `workspacenext`, `workspaceprev`, and
virtual-keyboard actions can be assigned to ordinary `bind =` keyboard chords
on desktops and convertibles.

The first SXMO-inspired hardware-button mappings are:

- **Volume up:** open the Fuzzel application menu.
- **Volume down:** toggle wvkbd.

The profile binds the standard xkb names `XF86AudioRaiseVolume` and
`XF86AudioLowerVolume`; it does not depend on FP5 input-device paths or raw
event codes. Other Wayland devices whose buttons expose those standard key
symbols can use the same mappings. These are currently single-press actions.
Multi-press and hold recognition will belong to a later, generic input-trigger
layer, where volume up can select context, main, and window menus supplied by
the shell toolkit.
