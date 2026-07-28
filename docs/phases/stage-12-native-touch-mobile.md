# Stage 12 — Native Touch & Mobile

**What it is.** This stage extends the input path from keyboard and pointer
devices to native Wayland touch. It establishes the compositor capabilities
needed by Linux phones and introduces the first device profile used to verify
them.

**Phase gate.** *A standalone 0xin DRM/KMS session on a phone accepts
independent multitouch contacts, returns safely to the session chooser, and
remains recoverable over SSH.*

## Native touch, not mouse emulation

Touch devices are attached to the existing `wlr_cursor`, which maps their
absolute device coordinates into the output layout. On touch-down, 0xin
hit-tests the scene and forwards the sequence to the selected surface with the
seat touch API:

- `wlr_seat_touch_notify_down`
- `wlr_seat_touch_notify_motion`
- `wlr_seat_touch_notify_up`
- `wlr_seat_touch_notify_cancel`
- `wlr_seat_touch_notify_frame`

Each contact keeps its own touch ID, target client, device, and
layout-to-surface offset. A contact remains attached to the surface where it
started, as required by Wayland, even when the finger moves across another
window.

Contacts are never converted into pointer events. Applications therefore
receive real multitouch: one-finger scrolling and two-finger pinch gestures can
coexist, and additional fingers remain independent.

## Device lifecycle

The seat advertises touch capability only while at least one touch device is
present. Device destruction removes listeners, detaches the device from the
cursor, and cancels affected client sequences. This matters during VT switches
and session pauses, where a backend can remove an input device without first
sending every expected touch-up event.

## Device profiles

Phone-specific hardware and session defaults stay outside the compositor core.
The first reference profile, `profiles/fp5/`, targets a Fairphone 5 running
postmarketOS and provides:

- `DSI-1` at scale `2.4`
- a portrait-first split direction
- a standalone greetd/Phrog session entry
- a wrapper that selects the DRM and libinput backends
- an isolated postmarketOS build/runtime library path

That reference device remains recoverable with:

```sh
ssh fp5 'pkill -TERM -x 0xin'
```

The session wrapper exits with the compositor, returning control to greetd.

## Reference-hardware verification

The implementation was tested both nested and directly on the current FP5
reference display.
`weston-simple-touch` was used only as a diagnostic Wayland client—not as the
compositor. It displayed a separate color for every simultaneous contact,
verified from one through seven fingers. Foot also received native one-finger
scrolling.

The standalone session was then tested on its `DSI-1` output at 1224×2700,
including SSH recovery back to the Phrog chooser.

## Boundary

Touch delivery is a general compositor capability, not a device-specific fork.
Other phones with supported wlroots backends and standard Wayland/libinput
devices should use the same core implementation and supply their own profile
only where hardware or session defaults differ.

Gesture policy—such as edge-swiping between workspaces—belongs in a separate
optional touch-policy module rather than in this low-level forwarding path.
