#define WLR_USE_UNSTABLE
#include <math.h>
#include <stdlib.h>
#include <wlr/types/wlr_cursor.h>
#include <wlr/types/wlr_pointer.h>
#include <wlr/types/wlr_pointer_gestures_v1.h>
#include <wlr/types/wlr_seat.h>
#include <wlr/util/log.h>

#include "input_internal.h"

// --- touchpad gestures -------------------------------------------------------
//
// Unlike the touchscreen path next door, there is no recognizer here. libinput
// already decided that these fingers are a swipe or a pinch and counted them;
// wlr_cursor re-emits that verbatim. All this file does is accumulate one
// gesture's travel, classify it once when it ends, and decide who the gesture
// belonged to.
//
// **Two fingers never appears as a swipe.** On a touchpad libinput treats two
// fingers as scrolling and delivers axis events, so swipes start at three
// fingers and pinch is a separate event type. That asymmetry is why the config
// triggers are their own `pad-` family rather than the touchscreen names.

// Minimum travel, in accumulated device units, before a swipe counts as
// deliberate rather than a wobble while lifting fingers off. Deliberately
// generous: a missed gesture is retried in a moment, a spurious workspace
// switch is not.
#define OXIDE_PAD_SWIPE_MIN 60.0
// How much one axis must dominate for the swipe to be that axis. Below this the
// gesture was diagonal enough that guessing would be worse than ignoring it.
#define OXIDE_PAD_AXIS_RATIO 1.5
// Scale change either side of 1.0 before a pinch is a pinch.
#define OXIDE_PAD_PINCH_IN 0.8
#define OXIDE_PAD_PINCH_OUT 1.25

static bool gesture_bound(struct oxide_pointer *p, uint32_t trigger) {
    return (p->gesture_mask & (1u << trigger)) != 0;
}

// True if any trigger this gesture could possibly resolve to is bound. Decided
// once, at begin: claiming halfway through would leave a client holding a begin
// with no matching end.
static bool swipe_wanted(struct oxide_pointer *p, uint32_t fingers) {
    if (fingers != 3) {
        return false;
    }
    return gesture_bound(p, 23) || gesture_bound(p, 24)
            || gesture_bound(p, 25) || gesture_bound(p, 26);
}

static bool pinch_wanted(struct oxide_pointer *p) {
    return gesture_bound(p, 27) || gesture_bound(p, 28);
}

static void fire(struct oxide_pointer *p, uint32_t trigger) {
    if (p->gesture_callback != NULL && gesture_bound(p, trigger)) {
        p->gesture_callback(p->gesture_userdata, trigger);
    }
}

static void handle_swipe_begin(void *userdata, void *data) {
    struct oxide_pointer *p = userdata;
    struct wlr_pointer_swipe_begin_event *event = data;

    p->swipe_dx = 0;
    p->swipe_dy = 0;
    p->swipe_claimed = swipe_wanted(p, event->fingers);
    if (!p->swipe_claimed && p->pointer_gestures != NULL) {
        wlr_pointer_gestures_v1_send_swipe_begin(p->pointer_gestures, p->seat,
                event->time_msec, event->fingers);
    }
}

static void handle_swipe_update(void *userdata, void *data) {
    struct oxide_pointer *p = userdata;
    struct wlr_pointer_swipe_update_event *event = data;

    if (p->swipe_claimed) {
        p->swipe_dx += event->dx;
        p->swipe_dy += event->dy;
        return;
    }
    if (p->pointer_gestures != NULL) {
        wlr_pointer_gestures_v1_send_swipe_update(p->pointer_gestures, p->seat,
                event->time_msec, event->dx, event->dy);
    }
}

static void handle_swipe_end(void *userdata, void *data) {
    struct oxide_pointer *p = userdata;
    struct wlr_pointer_swipe_end_event *event = data;

    if (!p->swipe_claimed) {
        if (p->pointer_gestures != NULL) {
            wlr_pointer_gestures_v1_send_swipe_end(p->pointer_gestures, p->seat,
                    event->time_msec, event->cancelled);
        }
        return;
    }
    p->swipe_claimed = false;
    if (event->cancelled) {
        return;
    }

    double adx = fabs(p->swipe_dx);
    double ady = fabs(p->swipe_dy);
    if (adx < OXIDE_PAD_SWIPE_MIN && ady < OXIDE_PAD_SWIPE_MIN) {
        return;
    }
    if (adx >= ady * OXIDE_PAD_AXIS_RATIO) {
        fire(p, p->swipe_dx < 0 ? 23 : 24);          // pad-three-left / -right
    } else if (ady >= adx * OXIDE_PAD_AXIS_RATIO) {
        fire(p, p->swipe_dy < 0 ? 25 : 26);          // pad-three-up / -down
    }
    // Too diagonal to call: deliberately nothing.
}

static void handle_pinch_begin(void *userdata, void *data) {
    struct oxide_pointer *p = userdata;
    struct wlr_pointer_pinch_begin_event *event = data;

    p->pinch_scale = 1.0;
    p->pinch_claimed = pinch_wanted(p);
    if (!p->pinch_claimed && p->pointer_gestures != NULL) {
        wlr_pointer_gestures_v1_send_pinch_begin(p->pointer_gestures, p->seat,
                event->time_msec, event->fingers);
    }
}

static void handle_pinch_update(void *userdata, void *data) {
    struct oxide_pointer *p = userdata;
    struct wlr_pointer_pinch_update_event *event = data;

    if (p->pinch_claimed) {
        // Absolute scale relative to begin, so the last one is the answer —
        // no accumulation.
        p->pinch_scale = event->scale;
        return;
    }
    if (p->pointer_gestures != NULL) {
        wlr_pointer_gestures_v1_send_pinch_update(p->pointer_gestures, p->seat,
                event->time_msec, event->dx, event->dy, event->scale,
                event->rotation);
    }
}

static void handle_pinch_end(void *userdata, void *data) {
    struct oxide_pointer *p = userdata;
    struct wlr_pointer_pinch_end_event *event = data;

    if (!p->pinch_claimed) {
        if (p->pointer_gestures != NULL) {
            wlr_pointer_gestures_v1_send_pinch_end(p->pointer_gestures, p->seat,
                    event->time_msec, event->cancelled);
        }
        return;
    }
    p->pinch_claimed = false;
    if (event->cancelled) {
        return;
    }
    if (p->pinch_scale <= OXIDE_PAD_PINCH_IN) {
        fire(p, 27);                                  // pad-pinch-in
    } else if (p->pinch_scale >= OXIDE_PAD_PINCH_OUT) {
        fire(p, 28);                                  // pad-pinch-out
    }
}

// Hold has no trigger of its own, but it still has to reach clients: libinput
// emits it for a resting finger, and a client that asked for gestures expects
// the pair. Always forwarded, never claimed.
static void handle_hold_begin(void *userdata, void *data) {
    struct oxide_pointer *p = userdata;
    struct wlr_pointer_hold_begin_event *event = data;
    if (p->pointer_gestures != NULL) {
        wlr_pointer_gestures_v1_send_hold_begin(p->pointer_gestures, p->seat,
                event->time_msec, event->fingers);
    }
}

static void handle_hold_end(void *userdata, void *data) {
    struct oxide_pointer *p = userdata;
    struct wlr_pointer_hold_end_event *event = data;
    if (p->pointer_gestures != NULL) {
        wlr_pointer_gestures_v1_send_hold_end(p->pointer_gestures, p->seat,
                event->time_msec, event->cancelled);
    }
}

void pointer_gestures_init(struct oxide_pointer *p) {
    signal_add(&p->cursor->events.swipe_begin, handle_swipe_begin, p);
    signal_add(&p->cursor->events.swipe_update, handle_swipe_update, p);
    signal_add(&p->cursor->events.swipe_end, handle_swipe_end, p);
    signal_add(&p->cursor->events.pinch_begin, handle_pinch_begin, p);
    signal_add(&p->cursor->events.pinch_update, handle_pinch_update, p);
    signal_add(&p->cursor->events.pinch_end, handle_pinch_end, p);
    signal_add(&p->cursor->events.hold_begin, handle_hold_begin, p);
    signal_add(&p->cursor->events.hold_end, handle_hold_end, p);
}

void oxide_cursor_set_pointer_gestures(struct wlr_cursor *cursor,
        struct wlr_pointer_gestures_v1 *gestures) {
    struct oxide_pointer *p = cursor->data;
    p->pointer_gestures = gestures;
}
