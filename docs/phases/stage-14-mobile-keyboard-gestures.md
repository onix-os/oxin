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
gesture it moves just above the configured keyboard top edge, so the close
gesture is physically possible without covering keyboard buttons. The close
target is a centered 220-by-56-logical-pixel strip entirely above the keyboard:
the downward sequence remains compositor-owned after crossing the boundary,
but no keyboard button loses its touch-down. The hidden bottom-edge target
remains larger for easy acquisition.

## Keyboard control

The original Stage 14 configuration was process-specific. Stage 16 replaces it
with a generic virtual-keyboard controller plus input mappings:

```ini
virtual_keyboard_show = pkill -USR2 -x wvkbd-mobintl
virtual_keyboard_hide = pkill -USR1 -x wvkbd-mobintl
virtual_keyboard_height = 125

gesture = bottom-up, keyboardshow
gesture = keyboard-top-down, keyboardhide
```

The configured commands use wvkbd's documented `SIGUSR2` show and `SIGUSR1`
hide controls. The keyboard stays alive and connected to the virtual-keyboard
protocol while hidden, avoiding protocol and startup latency on every gesture.

The height is in logical output pixels and must match the keyboard surface's
scaled portrait height. A client configured in buffer pixels needs conversion:
the FP5 uses `-H 300` at output scale 2.4, so its configured logical height is
125 as a startup fallback. Once the layer surface maps, Stage 16 replaces this
estimate with the actual bottom exclusive zone. Desktop configurations omit
the gesture mappings, so they get no handle and no intercepted edge touches;
they can still bind the keyboard actions to physical keys.

## FP5 reference profile

The FP5 wrapper now starts:

```sh
wvkbd-mobintl --hidden --no-popup -H 300 -L 200
```

Its configuration enables the gesture and declares the matching 125-logical-
pixel portrait height (`300 / 2.4`). This is a reference hardware profile, not
device-specific compositor behavior; another mobile profile can select a
different process, scale, and height.

Text-input and input-method protocols remain future work. They can eventually
show the keyboard automatically when a text field gains focus, while this
explicit gesture remains useful as a user override and recovery path.

## Client library isolation

The FP5 wrapper uses a private sysroot in `LD_LIBRARY_PATH` so the compositor
can load its pinned wlroots build. Spawned clients must not inherit that path:
doing so made Firefox ESR load incompatible compositor-side libraries and
crash before mapping a window. All 0xin spawn paths now remove
`LD_LIBRARY_PATH` from the child environment while the already-running
compositor retains its loaded libraries.
