# Stage 16 — Unified Input Mappings

**What it is.** Keyboard chords and compositor touch gestures now resolve to
the same `Action` values and execute through one dispatcher. Device input
recognition is separate from compositor policy.

**Phase gate.** *Workspace and virtual-keyboard actions can be invoked from
either configured keys or configured touch gestures, while a desktop config
with no gestures reserves no touch regions.*

## Trigger, mapping, action

The input path has three layers:

1. the keyboard or touch recognizer produces a physical trigger;
2. configuration maps that trigger to an action;
3. the shared dispatcher performs the action.

The current named touch triggers are:

- `bottom-up`
- `bottom-down`
- `edge-left-in`
- `edge-right-in`

They use the Stage 14/15 logical-coordinate activation regions and thresholds.
Only triggers present in `gesture =` lines are enabled in the C recognizer.
Consequently an unconfigured desktop touchscreen loses no input to compositor
gesture policy.

Recognizer regions also respect active input surfaces: when the configured
virtual keyboard is visible, workspace edge triggers are limited to the area
above it so edge keys remain normal client input.

Each trigger has at most one mapping. A later line for the same trigger
replaces the earlier line, matching keyboard chord override semantics.

## Shared actions

The action parser and dispatcher add:

- `workspacenext`
- `workspaceprev`
- `keyboardshow`
- `keyboardhide`
- `keyboardtoggle`

For example, a touch-first profile can use:

```ini
gesture = edge-left-in, workspaceprev
gesture = edge-right-in, workspacenext
```

while a desktop profile can expose the same behavior through:

```ini
bind = MOD, bracketleft, workspaceprev
bind = MOD, bracketright, workspacenext
```

Workspace next/previous wraps around the existing nine-workspace ring and uses
the same focus, scene visibility, and multi-output rules as numbered keyboard
bindings.

## Virtual-keyboard controller

Virtual-keyboard policy is no longer tied to a particular gesture or process
name:

```ini
virtual_keyboard_show = pkill -USR2 -x wvkbd-mobintl
virtual_keyboard_hide = pkill -USR1 -x wvkbd-mobintl
virtual_keyboard_height = 125
```

The show/hide/toggle actions run those commands and update compositor keyboard
visibility, recognizer state, and handle position. Missing commands make the
corresponding action a logged no-op. A convertible or desktop setup may bind
`keyboardtoggle` to a key; the FP5 maps explicit show/hide gestures.

Hardware buttons use the same keybinding path when libinput/xkb exposes a
standard keysym. For example, the FP5 profile controls its default audio sink:

```ini
bind = , XF86AudioRaiseVolume, spawn, pactl set-sink-volume @DEFAULT_SINK@ +5%
bind = , XF86AudioLowerVolume, spawn, pactl set-sink-volume @DEFAULT_SINK@ -5%
```

There are deliberately no raw `/dev/input/event*` paths or Linux key codes in
the configuration. This keeps the mappings usable on other Wayland devices
with conventional media buttons. The present binding engine handles each key
press independently; recognizing SXMO-like double/triple presses and holds is
future work for a generic trigger layer, not FP5-specific compositor policy.

`virtual_keyboard_height` is only a startup fallback. After the keyboard's
layer surface maps, 0xin derives its real top edge from the bottom exclusive
zone already computed by layer-shell arrangement. The handle and recognizer
therefore follow the actual scaled surface rather than relying on a
device-specific pixel conversion.

The controller commands are intentionally generic. The current FP5 profile
uses wvkbd signals, while a future input-method implementation can replace the
controller without changing gesture or key mappings.

## Scope

This stage unifies keys and compositor touch gestures. Pointer buttons, axes,
stylus input, and touchpad gesture protocols can add trigger types later
without adding new policy dispatch paths.
