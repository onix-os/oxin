# Stage 14 — Mobile Keyboard Gestures

**What it is.** This stage adds an opt-in, compositor-owned bottom gesture
handle for showing and hiding an on-screen keyboard without changing keyboard
focus.

**Phase gate.** *On the FP5 reference profile, a one-finger upward swipe from
the bottom handle shows wvkbd and a downward swipe from the handle above the
visible keyboard hides it.*

## Gesture recognition

The input shim reserves an enlarged touch target around the small visible pill.
A touch sequence that starts in this target belongs to the compositor and is
not sent partly to an application:

- an upward movement of 60 logical pixels while hidden requests **show**;
- a downward movement of 60 logical pixels while visible requests **hide**;
- shorter movements end without an action.

All touches beginning outside the target retain the existing native Wayland
touch path. This avoids interpreting application scrolling as compositor
policy.

When the keyboard is hidden the handle sits at the bottom edge. After a show
gesture it moves to the configured keyboard top edge, so the close gesture is
physically possible and the handle remains visible.

## Keyboard control

The generic configuration is opt-in:

```ini
gesture_keyboard = wvkbd-mobintl
gesture_keyboard_height = 300
```

`gesture_keyboard` is a Linux process name of at most 15 characters. Upward and
downward gestures send that process `SIGUSR2` and `SIGUSR1`, respectively.
These are wvkbd's documented show and hide controls. The keyboard stays alive
and connected to the virtual-keyboard protocol while hidden, avoiding protocol
and startup latency on every gesture.

The height is in logical output pixels and must match the keyboard client's
configured portrait height. Desktop configurations omit `gesture_keyboard`, so
they get no handle and no intercepted edge touches.

## FP5 reference profile

The FP5 wrapper now starts:

```sh
wvkbd-mobintl --hidden --no-popup -H 300 -L 200
```

Its configuration enables the gesture and declares the matching 300-pixel
portrait height. This is a reference hardware profile, not device-specific
compositor behavior; another mobile profile can select a different process and
height.

Text-input and input-method protocols remain future work. They can eventually
show the keyboard automatically when a text field gains focus, while this
explicit gesture remains useful as a user override and recovery path.
