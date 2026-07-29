# Stage 14 — Mobile Keyboard Gestures

**What it is.** This stage adds an opt-in, compositor-owned bottom gesture
handle for showing and hiding an on-screen keyboard without changing keyboard
focus.

**Phase gate.** *On the FP5 reference profile, a one-finger upward swipe from
the bottom handle shows wvkbd and a downward swipe from the handle above the
visible keyboard hides it.*

## Gesture recognition

The input shim reserves an enlarged touch target around the small bottom pill
while the keyboard is hidden:

- an upward movement of 60 logical pixels while hidden requests **show**;
- a keyboard touch moving at least 70 logical pixels downward and reaching the
  bottom 28-pixel edge requests **hide**;
- taps and shorter movements remain ordinary client input.

All touches beginning outside the target retain the existing native Wayland
touch path. This avoids interpreting application scrolling as compositor
policy.

When the keyboard is hidden the handle sits at the bottom edge and an upward
swipe is captured immediately. Hiding follows SXMO-style delayed arbitration:
the touch-down first reaches wvkbd, so taps remain usable keys. Only when the
motion qualifies as a downward swipe ending at the bottom does 0xin cancel that
client touch sequence and run the mapped `bottom-down` action.

## Keyboard control

The original Stage 14 configuration was process-specific. Stage 16 replaces it
with a generic virtual-keyboard controller plus input mappings:

```ini
virtual_keyboard_show = pkill -USR2 -x wvkbd-mobintl
virtual_keyboard_hide = pkill -USR1 -x wvkbd-mobintl
virtual_keyboard_height = 125
gesture_handle = hidden

gesture = bottom-up, keyboardshow
gesture = bottom-down, keyboardhide
```

`gesture_handle` accepts `visible` (the default) or `hidden`. Hiding it removes
only the rendered pill; the configured bottom gesture target remains active.
The FP5 profile now hides the pill after its gesture layout has become familiar,
while another touch profile can retain the discoverability hint.

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
