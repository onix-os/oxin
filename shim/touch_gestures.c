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

// --- touch -----------------------------------------------------------------

struct oxide_touch_point *touch_point_find(
        struct oxide_pointer *p, int32_t touch_id) {
    struct oxide_touch_point *point;
    wl_list_for_each(point, &p->touch_points, link) {
        if (point->touch_id == touch_id) {
            return point;
        }
    }
    return NULL;
}

bool keyboard_gesture_hit(struct oxide_pointer *p, double lx, double ly) {
    if (p->keyboard_visible || (p->gesture_mask & (1u << 0)) == 0
            || p->output_layout == NULL) {
        return false;
    }
    struct wlr_output *output =
            wlr_output_layout_output_at(p->output_layout, lx, ly);
    if (output == NULL) {
        return false;
    }
    struct wlr_box box;
    wlr_output_layout_get_box(p->output_layout, output, &box);
    double center_x = box.x + box.width / 2.0;
    // The hidden handle sits at the physical bottom edge and needs a larger
    // upward-only acquisition target.
    double handle_y = box.y + box.height - 10;
    return lx >= center_x - 100 && lx <= center_x + 100
            && ly >= handle_y - 45 && ly <= handle_y + 45;
}

int workspace_gesture_edge(struct oxide_pointer *p,
        double lx, double ly) {
    // Bits 2/3: horizontal edge-in swipes. Bits 17/18: left-edge vertical
    // volume swipes. Bits 21/22: right-edge vertical workspace-step swipes.
    // Each vertical pair claims its 28px strip even if the horizontal
    // edge-in trigger on that same side isn't configured.
    if ((p->gesture_mask
                    & ((1u << 2) | (1u << 3) | (1u << 17) | (1u << 18)
                            | (1u << 21) | (1u << 22)))
                    == 0
            || p->output_layout == NULL) {
        return 0;
    }
    struct wlr_output *output =
            wlr_output_layout_output_at(p->output_layout, lx, ly);
    if (output == NULL) {
        return 0;
    }
    struct wlr_box box;
    wlr_output_layout_get_box(p->output_layout, output, &box);
    // The virtual keyboard owns its full surface, including edge-column keys
    // such as Tab, Backspace, P, and Return. Keep workspace-edge policy above
    // it while visible instead of stealing those touches.
    if (p->keyboard_visible
            && ly >= box.y + box.height - p->keyboard_height) {
        return 0;
    }
    if (lx <= box.x + 28
            && (p->gesture_mask & ((1u << 2) | (1u << 17) | (1u << 18)))
                    != 0) {
        return -1;
    }
    if (lx >= box.x + box.width - 28
            && (p->gesture_mask & ((1u << 3) | (1u << 21) | (1u << 22)))
                    != 0) {
        return 1;
    }
    return 0;
}

bool top_gesture_hit(struct oxide_pointer *p, double lx, double ly) {
    if ((p->gesture_mask & ((1u << 4) | (1u << 5) | (1u << 6))) == 0
            || p->output_layout == NULL) {
        return false;
    }
    struct wlr_output *output =
            wlr_output_layout_output_at(p->output_layout, lx, ly);
    if (output == NULL) {
        return false;
    }
    struct wlr_box box;
    wlr_output_layout_get_box(p->output_layout, output, &box);
    return ly <= box.y + 28;
}

// True for a touch-down landing on the visible keyboard while the
// swipe-down-to-hide gesture is configured — see gesture_kind 5's doc
// comment above and OXIDE_KEYBOARD_HOLD_MS.
bool keyboard_hide_candidate(struct oxide_pointer *p, double lx,
        double ly) {
    if (!p->keyboard_visible || (p->gesture_mask & (1u << 1)) == 0
            || p->output_layout == NULL) {
        return false;
    }
    struct wlr_output *output =
            wlr_output_layout_output_at(p->output_layout, lx, ly);
    if (output == NULL) {
        return false;
    }
    struct wlr_box box;
    wlr_output_layout_get_box(p->output_layout, output, &box);
    return ly >= box.y + box.height - p->keyboard_height;
}

// Forwards the touch-down a kind-5 point held back (its original position
// and true down time), then catches the client up to the current position
// if the finger has already moved since. Transitions the point to an
// ordinary gesture_kind == 0 client touch so all further motion/up events
// forward normally from here — the caller must already have cancelled any
// pending hold timer. A no-op (point left at kind 5) if no surface is
// found where the touch originally landed, e.g. the keyboard was hidden by
// something else mid-hold.
void release_hold(struct oxide_pointer *p,
        struct oxide_touch_point *point) {
    double sx, sy;
    struct wlr_surface *surface = surface_at_coords(p, point->start_lx,
            point->start_ly, &sx, &sy);
    if (surface == NULL) {
        return;
    }
    focus_surface(p, surface);
    wlr_seat_touch_notify_down(p->seat, surface, point->hold_time_msec,
            point->touch_id, sx, sy);
    struct wlr_touch_point *seat_point =
            wlr_seat_touch_get_point(p->seat, point->touch_id);
    if (seat_point == NULL) {
        return;
    }
    point->client = seat_point->client;
    point->offset_x = point->start_lx - sx;
    point->offset_y = point->start_ly - sy;
    point->gesture_kind = 0;
    if (point->last_lx != point->start_lx || point->last_ly != point->start_ly) {
        wlr_seat_touch_notify_motion(p->seat, point->last_time_msec,
                point->touch_id, point->last_lx - point->offset_x,
                point->last_ly - point->offset_y);
    }
}

// Fires once OXIDE_KEYBOARD_HOLD_MS elapses without the swipe-down gesture
// committing or the touch lifting — release it as a (slightly delayed)
// ordinary keypress.
void handle_keyboard_hold_timeout(void *userdata, void *data) {
    struct oxide_touch_point *point = userdata;
    oxide_event_source_remove(data);
    point->hold_timer = NULL;
    release_hold(point->owner, point);
}

void touch_cancel_client(struct oxide_pointer *p,
        struct wlr_seat_client *client) {
    wlr_seat_touch_notify_cancel(p->seat, client);
    struct oxide_touch_point *point, *tmp;
    wl_list_for_each_safe(point, tmp, &p->touch_points, link) {
        if (point->client == client) {
            wl_list_remove(&point->link);
            free(point);
        }
    }
}

