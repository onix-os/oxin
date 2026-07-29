# Stage 18 — Multi-finger Window Gestures

**What it is.** This stage adds opt-in two- and three-finger directional
touchscreen triggers to the shared input-action mapping layer.

**Gate:** *On the FP5 reference profile, a two-finger directional swipe swaps
the focused tiled window with its spatial neighbor, a three-finger vertical
swipe closes it, and a three-finger horizontal swipe sends it around the
workspace ring.*

## Configuration

```ini
gesture = two-up, movewindow, u
gesture = two-down, movewindow, d
gesture = two-left, movewindow, l
gesture = two-right, movewindow, r

gesture = three-up, close
gesture = three-down, close
gesture = three-left, movetoworkspaceprev
gesture = three-right, movetoworkspacenext
```

The `movewindow` and `close` actions are the same actions available to keyboard
bindings. `movetoworkspacenext` and `movetoworkspaceprev` extend that shared
action vocabulary with relative, wrapping workspace targets. Moving a window
does not switch the displayed workspace.

The FP5 maps a leftward three-finger swipe to the previous workspace and a
rightward swipe to the next workspace, so the window follows the physical
direction of the fingers.

## Touch arbitration

One-finger central touch remains native application input. If a second finger
arrives while multi-finger triggers are enabled, 0xin cancels the client's
Wayland touch sequence and promotes both points to a compositor gesture. A
third finger promotes that same group to the three-finger trigger set.

Recognition uses centroid travel of at least 70 logical pixels. Every finger
must travel at least 35 logical pixels in the selected direction, preventing a
single moving finger plus stationary taps from firing an action. A gesture
fires at most once and remains compositor-owned until all fingers lift.

Profiles without any `two-*` or `three-*` mappings do not enable this
arbitration path.

## Scope

These triggers describe physical input, not FP5 policy. Other touchscreen
Wayland devices can reuse them with different shared actions. Touchpad gesture
protocols remain separate future input sources because they arrive through
libinput gesture events rather than touchscreen contact points.
