#define WLR_USE_UNSTABLE
#include <math.h>
#include <stdlib.h>
#include <xkbcommon/xkbcommon.h>
#include <wlr/backend.h>
#include <wlr/types/wlr_compositor.h>
#include <wlr/types/wlr_cursor.h>
#include <wlr/types/wlr_data_device.h>
#include <wlr/types/wlr_input_device.h>
#include <wlr/types/wlr_keyboard.h>
#include <wlr/types/wlr_layer_shell_v1.h>
#include <wlr/types/wlr_pointer.h>
#include <wlr/types/wlr_primary_selection.h>
#include <wlr/types/wlr_scene.h>
#include <wlr/types/wlr_seat.h>
#include <wlr/types/wlr_session_lock_v1.h>
#include <wlr/types/wlr_touch.h>
#include <wlr/types/wlr_virtual_keyboard_v1.h>
#include <wlr/types/wlr_xcursor_manager.h>
#include <wlr/types/wlr_xdg_shell.h>
#include <wlr/util/log.h>

#include "input_internal.h"

bool multi_gestures_enabled(struct oxide_pointer *p) {
    // Bits 8-15: two/three-finger swipes. Bit 16: double-tap, now a
    // two-finger gesture too — a second finger must promote to a compositor
    // gesture for it to have a chance of recognizing a tap, same as swipes.
    return (p->gesture_mask & 0x1ff00u) != 0;
}

int multi_index(struct oxide_pointer *p, int32_t touch_id) {
    for (int i = 0; i < p->multi_count; i++) {
        if (p->multi_down[i] && p->multi_ids[i] == touch_id) {
            return i;
        }
    }
    return -1;
}

void multi_reset(struct oxide_pointer *p) {
    p->multi_active = false;
    p->multi_fired = false;
    p->multi_count = 0;
    p->multi_active_count = 0;
}

void multi_add(struct oxide_pointer *p, int32_t touch_id,
        double lx, double ly) {
    if (p->multi_count >= 3) {
        return;
    }
    int i = p->multi_count++;
    p->multi_ids[i] = touch_id;
    p->multi_down[i] = true;
    p->multi_start_x[i] = p->multi_x[i] = lx;
    p->multi_start_y[i] = p->multi_y[i] = ly;
    p->multi_active_count++;
}

// Starts the undecided window: the first finger (already a live client touch,
// or a bare-background candidate) is left completely undisturbed, and the
// second is tracked here but held back — not yet forwarded to any client —
// via a new gesture_kind-6 point. See multi_pending_motion for how this
// resolves into a swipe, a pinch, or a tap.
void multi_pending_begin(struct oxide_pointer *p,
        struct oxide_touch_point *first, struct wlr_touch_down_event *e,
        double second_lx, double second_ly) {
    struct oxide_touch_point *second = calloc(1, sizeof(*second));
    second->touch_id = e->touch_id;
    second->touch = e->touch;
    second->gesture_kind = 6;
    second->start_lx = second->last_lx = second_lx;
    second->start_ly = second->last_ly = second_ly;
    second->hold_time_msec = e->time_msec;
    second->last_time_msec = e->time_msec;
    wl_list_insert(&p->touch_points, &second->link);

    multi_reset(p);
    p->multi_pending = true;
    p->pending_first = first;
    p->pending_second = second;
    multi_add(p, first->touch_id, first->start_lx, first->start_ly);
    multi_add(p, e->touch_id, second_lx, second_ly);
}

// Commits the pending pair to a compositor gesture: cancels the first
// finger's client sequence (mirroring the old, always-immediate multi_begin),
// drops the held second finger (it was never delivered, so nothing to
// cancel), and hands off to the existing multi_active tracking. Leaves
// multi_x/y/count untouched — multi_pending_begin already populated them
// with both fingers' positions.
void multi_promote_to_active(struct oxide_pointer *p) {
    struct wlr_seat_client *client = p->pending_first->client;
    if (client != NULL) {
        touch_cancel_client(p, client);
    } else {
        wl_list_remove(&p->pending_first->link);
        free(p->pending_first);
    }
    wl_list_remove(&p->pending_second->link);
    free(p->pending_second);
    p->pending_first = NULL;
    p->pending_second = NULL;
    p->multi_pending = false;
    p->multi_active = true;
}

// Ends the pending window without ever having decided on a gesture: drops
// both tracked points without disturbing whichever is still a live client
// touch (the caller is responsible for that finger's own up/cancel — this
// just clears bookkeeping). Used when to-top or a stray teardown needs to
// abandon an in-progress pending pair.
void multi_pending_abandon(struct oxide_pointer *p) {
    if (p->pending_second != NULL) {
        wl_list_remove(&p->pending_second->link);
        free(p->pending_second);
    }
    p->pending_first = NULL;
    p->pending_second = NULL;
    p->multi_pending = false;
    multi_reset(p);
}

// Re-checks a pending pair's shape after one finger moved. Three outcomes:
// a pinch (the fingers' separation has changed by PINCH_PX or more) releases
// the held second finger to the client via release_hold, so real multitouch
// — e.g. an app's own pinch-to-zoom — takes over from here; a swipe (same
// centroid-direction test as multi_motion below) promotes to the existing
// compositor gesture; neither yet just keeps waiting. Only runs once both
// fingers are down (mirrors multi_motion's own active_count guard).
#define OXIDE_PINCH_PX 40
void multi_pending_motion(struct oxide_pointer *p) {
    if (p->multi_active_count != p->multi_count || p->multi_count != 2) {
        return;
    }
    double start_dist = hypot(p->multi_start_x[1] - p->multi_start_x[0],
            p->multi_start_y[1] - p->multi_start_y[0]);
    double cur_dist = hypot(p->multi_x[1] - p->multi_x[0],
            p->multi_y[1] - p->multi_y[0]);
    if (fabs(cur_dist - start_dist) >= OXIDE_PINCH_PX) {
        struct oxide_touch_point *second = p->pending_second;
        p->pending_first = NULL;
        p->pending_second = NULL;
        p->multi_pending = false;
        multi_reset(p);
        release_hold(p, second);
        return;
    }

    double dx = (p->multi_x[0] - p->multi_start_x[0]
                + p->multi_x[1] - p->multi_start_x[1]) / 2;
    double dy = (p->multi_y[0] - p->multi_start_y[0]
                + p->multi_y[1] - p->multi_start_y[1]) / 2;
    int direction;
    double distance;
    if (fabs(dx) > fabs(dy)) {
        direction = dx < 0 ? 2 : 3; // left, right
        distance = fabs(dx);
    } else {
        direction = dy < 0 ? 0 : 1; // up, down
        distance = fabs(dy);
    }
    if (distance < 70) {
        return;
    }
    for (int i = 0; i < 2; i++) {
        double finger_dx = p->multi_x[i] - p->multi_start_x[i];
        double finger_dy = p->multi_y[i] - p->multi_start_y[i];
        double projected = direction == 0 ? -finger_dy
                : direction == 1 ? finger_dy
                : direction == 2 ? -finger_dx : finger_dx;
        if (projected < 35) {
            return;
        }
    }
    uint32_t trigger = 8 + direction;
    if ((p->gesture_mask & (1u << trigger)) == 0) {
        return;
    }
    multi_promote_to_active(p);
    p->multi_fired = true;
    if (p->gesture_callback != NULL) {
        p->gesture_callback(p->gesture_userdata, trigger);
    }
}

void multi_motion(struct oxide_pointer *p, int32_t touch_id,
        double lx, double ly) {
    int changed = multi_index(p, touch_id);
    if (changed < 0) {
        return;
    }
    p->multi_x[changed] = lx;
    p->multi_y[changed] = ly;
    if (p->multi_fired || p->multi_active_count != p->multi_count
            || (p->multi_count != 2 && p->multi_count != 3)) {
        return;
    }

    double dx = 0, dy = 0;
    for (int i = 0; i < p->multi_count; i++) {
        dx += p->multi_x[i] - p->multi_start_x[i];
        dy += p->multi_y[i] - p->multi_start_y[i];
    }
    dx /= p->multi_count;
    dy /= p->multi_count;

    int direction;
    double distance;
    if (fabs(dx) > fabs(dy)) {
        direction = dx < 0 ? 2 : 3; // left, right
        distance = fabs(dx);
    } else {
        direction = dy < 0 ? 0 : 1; // up, down
        distance = fabs(dy);
    }
    if (distance < 70) {
        return;
    }

    // Require every finger to participate in the centroid direction. This
    // avoids treating one moving finger plus one stationary tap as a swipe.
    for (int i = 0; i < p->multi_count; i++) {
        double finger_dx = p->multi_x[i] - p->multi_start_x[i];
        double finger_dy = p->multi_y[i] - p->multi_start_y[i];
        double projected = direction == 0 ? -finger_dy
                : direction == 1 ? finger_dy
                : direction == 2 ? -finger_dx : finger_dx;
        if (projected < 35) {
            return;
        }
    }

    uint32_t trigger = (p->multi_count == 2 ? 8 : 12) + direction;
    if ((p->gesture_mask & (1u << trigger)) == 0) {
        return;
    }
    p->multi_fired = true;
    if (p->gesture_callback != NULL) {
        p->gesture_callback(p->gesture_userdata, trigger);
    }
}

// Called when a tracked two-finger touch fully releases. A tap gives no
// "in progress" signal to promote on (unlike the motion-based gestures
// above), so this checks retroactively: no swipe fired, and neither finger
// travelled past the tap threshold. Matches against the previous such tap
// for double-tap timing/position — the same window-identity gesture the
// single-finger version used to be, just promoted to two fingers so a
// plain one-finger tap is left alone for normal app interaction. Reuses
// the double-tap fields' doc comment above; unlike the old single-finger
// version, both taps are NOT delivered to the client — multi_begin already
// cancelled its touch sequence, same as any other two-finger gesture.
void multi_two_finger_tap_check(struct oxide_pointer *p,
        uint32_t time_msec) {
    if (p->multi_fired || p->multi_count != 2
            || (p->gesture_mask & (1u << 16)) == 0
            || p->double_tap_callback == NULL) {
        return;
    }
    for (int i = 0; i < 2; i++) {
        double travel = hypot(p->multi_x[i] - p->multi_start_x[i],
                p->multi_y[i] - p->multi_start_y[i]);
        if (travel > OXIDE_TAP_DRAG_PX) {
            return;
        }
    }
    double cx = (p->multi_x[0] + p->multi_x[1]) / 2;
    double cy = (p->multi_y[0] + p->multi_y[1]) / 2;
    double sx, sy;
    struct wlr_surface *surface = surface_at_coords(p, cx, cy, &sx, &sy);
    if (surface == NULL) {
        return;
    }
    struct wlr_surface *root = wlr_surface_get_root_surface(surface);
    bool matched = p->last_tap_surface == root
            && (time_msec - p->last_tap_time_msec) <= OXIDE_DOUBLE_TAP_MS
            && hypot(cx - p->last_tap_lx, cy - p->last_tap_ly)
                    <= OXIDE_DOUBLE_TAP_PX;
    if (matched) {
        // Consumed — a third tap starts a fresh pair rather than matching
        // again against this same recorded tap.
        p->last_tap_surface = NULL;
        p->double_tap_callback(p->double_tap_userdata, root);
    } else {
        p->last_tap_surface = root;
        p->last_tap_lx = cx;
        p->last_tap_ly = cy;
        p->last_tap_time_msec = time_msec;
    }
}

