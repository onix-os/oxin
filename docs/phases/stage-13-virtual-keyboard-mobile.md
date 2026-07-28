# Stage 13 — Virtual Keyboard & Mobile Scaling

**What it is.** This stage lets a Wayland on-screen keyboard inject keys into
the focused application and fixes the protocol and scaling details needed by
real mobile clients.

**Phase gate.** *In a standalone phone session, wvkbd accepts native touch,
types into Foot without stealing focus, and Firefox renders at the correct
fractional scale.*

## Virtual keyboard protocol

0xin publishes `zwp_virtual_keyboard_manager_v1`. When wvkbd creates a virtual
keyboard, the input shim attaches its `wlr_keyboard` to the existing seat.
Key and modifier events are forwarded through:

- `wlr_seat_keyboard_notify_key`
- `wlr_seat_keyboard_notify_modifiers`

Virtual keys bypass compositor keybindings. This prevents an on-screen Super,
Control, or letter key from accidentally triggering 0xin policy instead of
reaching the focused client.

This stage provides a manually visible keyboard. Automatic show/hide still
requires the text-input and input-method protocols and is a later capability.

## Layer surfaces must not steal application focus

wvkbd is a layer-shell surface with keyboard interactivity disabled. Tapping it
must deliver touch to the keyboard while leaving keyboard focus on Foot or
Firefox.

The focus policy therefore only moves keyboard focus to:

- an XDG toplevel; or
- a layer surface that explicitly requests keyboard interactivity.

Ordinary panels and on-screen keyboards can receive pointer/touch input without
becoming the seat's keyboard focus.

## The hidden XDG popup dependency

wvkbd 0.20 creates an XDG popup for key-preview feedback and refuses all input
until both its layer surface and popup receive their initial configure. Even
`--no-popup` disables only drawing; the protocol object is still created.

0xin now listens for new XDG popups and, after each popup's initial surface
commit, schedules its first configure with
`wlr_xdg_surface_schedule_configure`. Destroy listeners clean up a popup that
disappears before completing the handshake.

Without this step, touch reached wvkbd correctly but wvkbd intentionally emitted
no virtual key events.

## Fractional scale and Firefox

The current FP5 reference output uses scale 2.4. Integer buffer scale alone
caused Firefox's EGL subsurface to be interpreted at roughly three times its
intended logical size, leaving most browser chrome outside the display.

0xin now publishes:

- `wp_viewporter`
- `wp_fractional_scale_manager_v1`

wlroots' scene graph applies viewport state and advertises preferred fractional
scale to visible surfaces. Firefox can consequently submit integer-sized
buffers whose logical destination matches the 2.4-scaled output.

Fractional scaling is a general HiDPI compositor feature, not phone-specific
behavior.

## Portrait layout policy

The generic setting:

```ini
first_split = horizontal
```

changes the first dwindle division from left/right to top/bottom. The current
phone profile enables it so two applications retain the full portrait width.
Desktop 0xin keeps the original vertical first split by default.

Initial XDG configure prediction and final tree placement use the same setting,
so a newly mapped application receives its correct size before drawing its
first frame.

## Reference-hardware verification

On the current FP5 reference device:

- wvkbd typed letters, numbers, modifiers, Backspace, Enter, and Control
  combinations into Foot;
- tapping wvkbd did not take keyboard focus away from Foot;
- native touch scrolling continued to work;
- Firefox rendered at the correct fractional scale; and
- Foot and Firefox tiled top/bottom under the phone profile.

The temporary session currently starts Foot and a manually visible
`wvkbd-mobintl`. Automatic keyboard activation and the permanent phone shell
remain separate milestones.
