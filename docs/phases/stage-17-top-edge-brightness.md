# Stage 17 — Top-edge Shell Gestures

**What it is.** This stage adds opt-in horizontal and downward swipes starting
in a thin top-edge strip, plus an upward swipe that reaches that strip. They
enter the same configurable action dispatcher as keyboard chords and the
existing bottom and side-edge gestures.

**Gate:** *On the FP5 reference profile, a horizontal edge-to-edge top swipe
spans approximately the full brightness range, while shorter swipes adjust it
proportionally. A downward swipe opens Fuzzel and an upward swipe to the top
closes it. Profiles without these mappings reserve no top-edge touch area.*

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
top edge. Horizontal direction locks after 30 logical pixels of deliberate
movement. The recognizer then dispatches one configured directional action for
every 5% of output width crossed, up to 20 times. With the FP5 profile's 5%
relative brightness commands this makes a full-width swipe cover 100%, while
retaining device-specific brightness policy in configuration. A downward
top-edge swipe uses the same start strip and a 70-logical-pixel threshold. The
top gesture is tested before the side-edge workspace strips, giving the small
top corners unambiguous top-edge behavior.

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

- Configuration parser tests cover both horizontal trigger names, independent
  bits in the enabled-trigger mask, and commands containing percentage syntax.
- `cargo test` exercises the mapping path.
- The FP5 profile is built natively and tested against its standard backlight
  device through `brightnessctl`.
