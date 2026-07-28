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
from it to show wvkbd. While wvkbd is visible the handle moves to its top edge;
swipe downward from there to hide it. Touches that begin outside the enlarged
handle target continue to applications unchanged.
