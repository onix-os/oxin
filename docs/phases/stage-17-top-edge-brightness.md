# Stage 17 — Top-edge Shell Gestures

**What it is.** This stage adds opt-in horizontal and downward swipes starting
in a thin top-edge strip, plus an upward swipe that reaches that strip. They
enter the same configurable action dispatcher as keyboard chords and the
existing bottom and side-edge gestures.

**Gate:** *On the FP5 reference profile, a top-edge swipe from left to right
increases display brightness by 5%, while a swipe from right to left decreases
it by 5%. A downward swipe opens Fuzzel and an upward swipe to the top closes
it. Profiles without these mappings reserve no top-edge touch area.*

## Configuration

The physical triggers are direction names rather than brightness or menu
actions:

```ini
gesture = top-right, spawn, brightnessctl set +5%
gesture = top-left, spawn, brightnessctl set 5%-
gesture = top-down, spawn, pgrep -x fuzzel >/dev/null || fuzzel
gesture = to-top, spawn, pkill -x fuzzel
```

This separation is intentional. The triggers may launch any shared action,
while brightness and menu commands remain profile policy. The FP5 uses
`brightnessctl` and Fuzzel, but another device can select different controllers
or map the gestures to unrelated shell-toolkit actions.

## Recognition and application input

A configured top gesture must start within 28 logical pixels of an output's
top edge and travel at least 70 logical pixels horizontally in its configured
direction. A downward top-edge swipe uses the same start strip and threshold.
The top gesture is tested before the side-edge workspace strips, giving the
small top corners unambiguous top-edge behavior.

The recognizer owns a top-edge sequence from touch-down because a compositor
cannot retract an isolated Wayland touch from one client without cancelling
that client's other active points. Central touches continue directly to
applications. If neither top trigger is configured, no top strip is claimed,
which keeps the behavior opt-in for desktops and other Wayland devices.

The `to-top` trigger behaves differently: a central touch is initially
forwarded to its application and becomes a gesture only after moving upward by
70 logical pixels and reaching the top 28 pixels. At that point the compositor
cancels the client touch sequence and dispatches the configured close command.

## Verification

- Configuration parser tests cover both trigger names, independent bits in
  the enabled-trigger mask, and commands containing percentage syntax.
- `cargo test` exercises the mapping path.
- The FP5 profile is built natively and tested against its standard backlight
  device through `brightnessctl`.
