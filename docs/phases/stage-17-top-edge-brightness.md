# Stage 17 — Top-edge Brightness Gestures

**What it is.** This stage adds opt-in leftward and rightward swipes starting
in a thin top-edge strip. They enter the same configurable action dispatcher
as keyboard chords and the existing bottom and side-edge gestures.

**Gate:** *On the FP5 reference profile, a top-edge swipe from left to right
increases display brightness by 5%, while a swipe from right to left decreases
it by 5%. Profiles without these mappings reserve no top-edge touch area.*

## Configuration

The two new physical triggers are direction names rather than brightness
actions:

```ini
gesture = top-right, spawn, brightnessctl set +5%
gesture = top-left, spawn, brightnessctl set 5%-
```

This separation is intentional. `top-right` and `top-left` may launch any
shared action, and brightness commands remain profile policy. The FP5 uses
`brightnessctl`, but another device can select a different controller or map
the gestures to unrelated shell-toolkit actions.

## Recognition and application input

A configured top gesture must start within 28 logical pixels of an output's
top edge and travel at least 70 logical pixels horizontally in its configured
direction. The top gesture is tested before the side-edge workspace strips,
giving the small top corners unambiguous horizontal behavior.

The recognizer owns a top-edge sequence from touch-down because a compositor
cannot retract an isolated Wayland touch from one client without cancelling
that client's other active points. Central touches continue directly to
applications. If neither top trigger is configured, no top strip is claimed,
which keeps the behavior opt-in for desktops and other Wayland devices.

## Verification

- Configuration parser tests cover both trigger names, independent bits in
  the enabled-trigger mask, and commands containing percentage syntax.
- `cargo test` exercises the mapping path.
- The FP5 profile is built natively and tested against its standard backlight
  device through `brightnessctl`.
