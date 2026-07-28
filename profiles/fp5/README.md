# Fairphone 5 touch test

This profile is intentionally temporary: it starts 0xide directly on DRM/KMS
with Weston’s touch visualizer, without a bar, keyboard, gestures, or autologin.
Phosh and Hyprland remain separate sessions.

Build 0xide in `~/Projects/0xide-touch`, then install the chooser entry:

```sh
sudo install -m 0644 \
  ~/Projects/0xide-touch/profiles/fp5/0xide-touch-test.desktop \
  /usr/share/wayland-sessions/0xide-touch-test.desktop
```

Confirm SSH works before logging out. Select **0xide Touch Test** in Phrog.
End the test from another machine:

```sh
ssh fp5 'pkill -TERM -x 0xide'
```

The wrapper exits with 0xide, returning control to greetd. Remove the temporary
entry when it is no longer needed:

```sh
sudo rm /usr/share/wayland-sessions/0xide-touch-test.desktop
```
