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
from it to show wvkbd. While wvkbd is visible the handle moves just above its
top edge; swipe downward from there to hide it. Its close target stays tightly
above the keyboard around the pill, so it is easy to acquire without covering
keyboard buttons. Touches that begin outside the handle target continue to
applications unchanged.

Swipe inward from the left edge for the previous workspace or from the right
edge for the next workspace. The nine-workspace ring wraps at either end.
Touches must begin within the narrow edge activation strip; horizontal gestures
started elsewhere remain application input. When wvkbd is visible, these strips
stop at its top edge so the keyboard's left- and right-edge keys remain usable.

These meanings come from the profile's `gesture = TRIGGER, ACTION` mappings,
not hardcoded phone policy. The same `workspacenext`, `workspaceprev`, and
virtual-keyboard actions can be assigned to ordinary `bind =` keyboard chords
on desktops and convertibles.
