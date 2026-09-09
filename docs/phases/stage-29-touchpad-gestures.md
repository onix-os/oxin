# Stage 29 — Touchpad Gestures

**What it is.** Laptop touchpad swipes and pinches, in their own `pad-` trigger
family, reaching the same actions the touchscreen gestures already did — plus
`zwp_pointer_gestures_v1`, so gestures 0xin does not claim reach the focused
application instead.

**Gate:** *A three-finger swipe on a touchpad fires its bound action; an
unbound pinch reaches the focused window so an application's own zoom keeps
working; two-finger scrolling is untouched.*

## Why this was small

Nothing like Stage 12's touch work. libinput has already recognised the gesture
and counted the fingers by the time it reaches the compositor, and `wlr_cursor`
re-emits it verbatim — the signals were simply never subscribed:

```c
struct wl_signal swipe_begin, swipe_update, swipe_end;   // wlr_cursor.h
struct wl_signal pinch_begin, pinch_update, pinch_end;
struct wl_signal hold_begin, hold_end;
```

The events carry `fingers` and `dx`/`dy` (`wlr_pointer_swipe_update_event`) and
`scale` (`wlr_pointer_pinch_update_event`). So `shim/pointer_gestures.c` has no
recognizer in it at all, unlike `shim/touch_multi.c` which reconstructs swipes
and pinches from raw contacts. It accumulates one gesture's travel, classifies
it once at the end, and decides whose gesture it was.

## Why the triggers are a separate family

The obvious idea — let `three-left` fire from either device — is wrong, because
the devices do not offer the same gestures:

- **Two fingers is never a swipe on a touchpad.** libinput treats it as
  scrolling and delivers axis events, so swipes start at three fingers.
  `two-left` would have been config that could never fire.
- **Pinch is a distinct event type**, not a swipe with two fingers.
- **The edge triggers have no meaning.** `edge-left-in`, `bottom-up`, `to-top`
  describe where a finger lands *on the screen*; a touchpad has no such
  relationship to what is displayed.

Hence `pad-three-left/right/up/down` and `pad-pinch-in/out`. The action side was
already device-agnostic, so no action work was needed.

## Claimed or forwarded, decided at begin

wlroots forwards nothing on its own — `wlr_pointer_gestures_v1_send_swipe_begin`
and its siblings are explicit calls — so the compositor chooses per gesture. A
gesture whose triggers are all unbound is forwarded to the focused client; one
that could resolve to a binding is swallowed.

That decision is made at `begin` and holds for the gesture's whole life. Deciding
per event would let a client receive a `begin` with no matching `end`, which is a
protocol error in spirit if not in letter. Hold gestures have no trigger of their
own and are always forwarded, because a client that asked for gestures expects
the pair.

## Thresholds

- A swipe must travel `OXIDE_PAD_SWIPE_MIN` in total, and one axis must beat the
  other by `OXIDE_PAD_AXIS_RATIO`, or it is ignored — a diagonal smudge should
  do nothing rather than pick a direction.
- Pinch classifies on the final `scale`: at or below 0.8 is in, at or above 1.25
  is out.

These are deliberately generous. A missed gesture is retried in a moment; a
spurious workspace switch is not.

## Boundaries

- **Discrete, not continuous.** The gesture fires once, at the end. The
  touchscreen path grew reversible mid-gesture triggers; a touchpad workspace
  switch does not want that, and continuous dragging would be a separate piece
  of work.
- **Three fingers only.** Four-finger swipes are a trigger away if wanted; the
  mask has room (`GestureTrigger` reaches 28, and `gesture_mask` is a `u32` —
  a unit test asserts every trigger still fits).
- **Session lock is honoured for free**, because the shared `gesture_mask` the
  handlers test is the one `oxide_cursor_set_locked` already zeroes.
