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

// Fires `increase_trigger`/`decrease_trigger` enough times to walk
// point->gesture_steps to `target` (clamped to [-20, 20]) — one call per
// step, regardless of direction. Used by kind 2's stepped gestures so a
// single continuous touch can move freely back and forth without lifting:
// reversing direction mid-swipe compensates already-fired steps via the
// paired trigger (e.g. a "forward" step undoing a "back" one), rather than a
// one-way commit that can only ever advance further in whichever direction
// was moved first.
static void step_toward(struct oxide_pointer *p, struct oxide_touch_point *point,
        int target, uint32_t increase_trigger, uint32_t decrease_trigger) {
    if (target > 20) {
        target = 20;
    } else if (target < -20) {
        target = -20;
    }
    while (point->gesture_steps < target) {
        point->gesture_steps++;
        if ((p->gesture_mask & (1u << increase_trigger)) != 0
                && p->gesture_callback != NULL) {
            p->gesture_callback(p->gesture_userdata, increase_trigger);
        }
    }
    while (point->gesture_steps > target) {
        point->gesture_steps--;
        if ((p->gesture_mask & (1u << decrease_trigger)) != 0
                && p->gesture_callback != NULL) {
            p->gesture_callback(p->gesture_userdata, decrease_trigger);
        }
    }
}

void handle_touch_down(void *userdata, void *data) {
    struct oxide_pointer *p = userdata;
    struct wlr_touch_down_event *e = data;
    if (touch_point_find(p, e->touch_id) != NULL) {
        wlr_log(WLR_ERROR, "0xin: duplicate touch ID %d ignored",
                e->touch_id);
        return;
    }
    double lx, ly, sx, sy;
    wlr_cursor_absolute_to_layout_coords(p->cursor, &e->touch->base,
            e->x, e->y, &lx, &ly);
    if (p->multi_active) {
        multi_add(p, e->touch_id, lx, ly);
        return;
    }
    if (p->multi_pending) {
        // A third finger arrives before the pending pair resolved — this is
        // unambiguously the three-finger gesture, not a pinch (that's a
        // strictly two-finger shape). Commit now: promote_to_active cancels
        // the first finger and drops the held second, then this finger joins
        // as the third, same end state the old always-immediate design had.
        multi_promote_to_active(p);
        multi_add(p, e->touch_id, lx, ly);
        return;
    }
    if (multi_gestures_enabled(p)) {
        struct oxide_touch_point *candidate = NULL;
        int candidates = 0;
        struct oxide_touch_point *existing;
        wl_list_for_each(existing, &p->touch_points, link) {
            if (existing->gesture_kind == 0 || existing->gesture_kind == 4) {
                candidate = existing;
                candidates++;
            }
        }
        if (candidates == 1) {
            multi_pending_begin(p, candidate, e, lx, ly);
            return;
        }
    }
    if (keyboard_gesture_hit(p, lx, ly)) {
        struct oxide_touch_point *point = calloc(1, sizeof(*point));
        point->touch_id = e->touch_id;
        point->touch = e->touch;
        point->gesture_kind = 1;
        point->start_lx = lx;
        point->start_ly = ly;
        wl_list_insert(&p->touch_points, &point->link);
        return;
    }
    if (top_gesture_hit(p, lx, ly)) {
        struct oxide_touch_point *point = calloc(1, sizeof(*point));
        point->touch_id = e->touch_id;
        point->touch = e->touch;
        point->gesture_kind = 3;
        point->start_lx = lx;
        point->start_ly = ly;
        wl_list_insert(&p->touch_points, &point->link);
        return;
    }
    int gesture_edge = workspace_gesture_edge(p, lx, ly);
    if (gesture_edge != 0) {
        struct oxide_touch_point *point = calloc(1, sizeof(*point));
        point->touch_id = e->touch_id;
        point->touch = e->touch;
        point->gesture_kind = 2;
        point->gesture_edge = gesture_edge;
        point->start_lx = lx;
        point->start_ly = ly;
        wl_list_insert(&p->touch_points, &point->link);
        return;
    }
    struct wlr_surface *surface =
            surface_at_coords(p, lx, ly, &sx, &sy);
    if (surface == NULL) {
        if (multi_gestures_enabled(p)) {
            struct oxide_touch_point *point = calloc(1, sizeof(*point));
            point->touch_id = e->touch_id;
            point->touch = e->touch;
            point->gesture_kind = 4;
            point->start_lx = lx;
            point->start_ly = ly;
            wl_list_insert(&p->touch_points, &point->link);
        }
        return;
    }

    // Touches landing on the visible keyboard are ambiguous: a normal
    // keypress, or the start of a swipe-down-to-hide gesture over the same
    // surface. Forwarding immediately (as every other touch does, below)
    // would let the keyboard register a keypress before we know which — a
    // cancel arriving later can't un-type a character the client already
    // committed. Hold these briefly instead; see keyboard_hide_candidate,
    // release_hold, and handle_keyboard_hold_timeout.
    if (keyboard_hide_candidate(p, lx, ly)) {
        struct oxide_touch_point *point = calloc(1, sizeof(*point));
        point->touch_id = e->touch_id;
        point->touch = e->touch;
        point->gesture_kind = 5;
        point->owner = p;
        point->start_lx = lx;
        point->start_ly = ly;
        point->last_lx = lx;
        point->last_ly = ly;
        point->hold_time_msec = e->time_msec;
        point->last_time_msec = e->time_msec;
        wl_list_insert(&p->touch_points, &point->link);
        point->hold_timer = oxide_event_loop_add_timer(p->event_loop,
                OXIDE_KEYBOARD_HOLD_MS, handle_keyboard_hold_timeout, point);
        return;
    }

    focus_surface(p, surface);
    wlr_seat_touch_notify_down(p->seat, surface, e->time_msec,
            e->touch_id, sx, sy);
    struct wlr_touch_point *seat_point =
            wlr_seat_touch_get_point(p->seat, e->touch_id);
    if (seat_point == NULL) {
        return;
    }

    struct oxide_touch_point *point = calloc(1, sizeof(*point));
    point->touch_id = e->touch_id;
    point->offset_x = lx - sx;
    point->offset_y = ly - sy;
    point->client = seat_point->client;
    point->touch = e->touch;
    point->start_lx = lx;
    point->start_ly = ly;
    point->last_lx = lx;
    point->last_ly = ly;
    if ((p->gesture_mask & (1u << 7)) != 0 && p->output_layout != NULL) {
        struct wlr_output *output =
                wlr_output_layout_output_at(p->output_layout, lx, ly);
        if (output != NULL) {
            struct wlr_box box;
            wlr_output_layout_get_box(p->output_layout, output, &box);
            point->to_top_candidate = ly >= box.y + 70;
        }
    }
    if ((p->gesture_mask & ((1u << 19) | (1u << 20))) != 0
            && p->output_layout != NULL) {
        struct wlr_output *output =
                wlr_output_layout_output_at(p->output_layout, lx, ly);
        if (output != NULL) {
            struct wlr_box box;
            wlr_output_layout_get_box(p->output_layout, output, &box);
            point->to_edge_candidate =
                    lx >= box.x + 70 && lx <= box.x + box.width - 70;
        }
    }
    wl_list_insert(&p->touch_points, &point->link);
}

void handle_touch_motion(void *userdata, void *data) {
    struct oxide_pointer *p = userdata;
    struct wlr_touch_motion_event *e = data;
    if (p->multi_active && multi_index(p, e->touch_id) >= 0) {
        double lx, ly;
        wlr_cursor_absolute_to_layout_coords(p->cursor, &e->touch->base,
                e->x, e->y, &lx, &ly);
        multi_motion(p, e->touch_id, lx, ly);
        return;
    }
    struct oxide_touch_point *point = touch_point_find(p, e->touch_id);
    if (point == NULL) {
        return;
    }
    double lx, ly;
    wlr_cursor_absolute_to_layout_coords(p->cursor, &e->touch->base,
            e->x, e->y, &lx, &ly);
    if (point->gesture_kind == 5) {
        point->last_lx = lx;
        point->last_ly = ly;
        point->last_time_msec = e->time_msec;
        struct wlr_output *output =
                wlr_output_layout_output_at(p->output_layout, lx, ly);
        if (output != NULL) {
            // Downward travel alone is enough to commit — the keyboard is
            // only ~125 logical px tall, so also requiring the touch to
            // reach within 28px of the physical bottom edge (on top of 70px
            // of travel) demanded a swipe covering nearly the whole
            // keyboard, which reads as unreliable/"hard to close" in
            // practice.
            if (ly - point->start_ly >= 45) {
                // Confirmed swipe. The touch was never forwarded to any
                // client (that's the whole point of holding it), so
                // there's nothing to cancel — just fire the gesture.
                if (point->hold_timer != NULL) {
                    oxide_event_source_remove(point->hold_timer);
                    point->hold_timer = NULL;
                }
                if (p->gesture_callback != NULL) {
                    p->gesture_callback(p->gesture_userdata, 1);
                }
                wl_list_remove(&point->link);
                free(point);
            }
        }
        return;
    }
    if (point->gesture_kind == 6) {
        // The held second finger of an undecided pair. Update the position
        // multi_pending_motion classifies against and its own last-seen
        // fields (needed if it turns out to be a pinch — release_hold uses
        // them for the catch-up motion), then re-run classification. Never
        // forwarded directly from here: a swipe frees this point entirely
        // (multi_promote_to_active), a pinch delivers it via release_hold
        // inside multi_pending_motion, and "still undecided" means it stays
        // held. All three outcomes are handled there.
        point->last_lx = lx;
        point->last_ly = ly;
        point->last_time_msec = e->time_msec;
        int i = multi_index(p, e->touch_id);
        if (i >= 0) {
            p->multi_x[i] = lx;
            p->multi_y[i] = ly;
            multi_pending_motion(p);
        }
        return;
    }
    if (point->to_top_candidate) {
        struct wlr_output *output =
                wlr_output_layout_output_at(p->output_layout, lx, ly);
        if (output != NULL) {
            struct wlr_box box;
            wlr_output_layout_get_box(p->output_layout, output, &box);
            if (point->start_ly - ly >= 70 && ly <= box.y + 28) {
                struct wlr_seat_client *client = point->client;
                if (p->pending_first == point) {
                    multi_pending_abandon(p);
                }
                touch_cancel_client(p, client);
                if (p->gesture_callback != NULL) {
                    p->gesture_callback(p->gesture_userdata, 7);
                }
                return;
            }
        }
    }
    if (point->to_edge_candidate) {
        struct wlr_output *output =
                wlr_output_layout_output_at(p->output_layout, lx, ly);
        if (output != NULL) {
            struct wlr_box box;
            wlr_output_layout_get_box(p->output_layout, output, &box);
            double dx = lx - point->start_lx;
            bool left = dx <= -70 && lx <= box.x + 28
                    && (p->gesture_mask & (1u << 19)) != 0;
            bool right = dx >= 70 && lx >= box.x + box.width - 28
                    && (p->gesture_mask & (1u << 20)) != 0;
            if (left || right) {
                struct wlr_seat_client *client = point->client;
                if (p->pending_first == point) {
                    multi_pending_abandon(p);
                }
                touch_cancel_client(p, client);
                if (p->gesture_callback != NULL) {
                    p->gesture_callback(p->gesture_userdata, left ? 19 : 20);
                }
                return;
            }
        }
    }
    if (point->gesture_kind == 1) {
        double dy = ly - point->start_ly;
        if (!point->gesture_fired && dy <= -60) {
            point->gesture_fired = true;
            if (p->gesture_callback != NULL) {
                p->gesture_callback(p->gesture_userdata, 0);
            }
        }
        return;
    }
    if (point->gesture_kind == 2) {
        double dx = lx - point->start_lx;
        double dy = ly - point->start_ly;
        // Commit to vertical (volume/workspace) after a small deliberate
        // movement, same pattern as the top-edge brightness gesture (kind 3
        // below) just rotated 90°. Uses gesture_vlock rather than
        // gesture_edge for the commit, since gesture_edge already carries
        // left/right zone identity for this kind. Left and right each have
        // their own trigger pair and purpose (volume, workspace step) but
        // share this same mechanic. Unlike the old one-way lock, this is
        // only a commit to *an axis* — direction within it stays fully
        // reversible (see step_toward).
        bool vlock_eligible =
                (point->gesture_edge == -1
                        && (p->gesture_mask & ((1u << 17) | (1u << 18))) != 0)
                || (point->gesture_edge == 1
                        && (p->gesture_mask & ((1u << 21) | (1u << 22)))
                                != 0);
        if (vlock_eligible && point->gesture_vlock == 0
                && fabs(dy) >= 30 && fabs(dy) > fabs(dx)) {
            point->gesture_vlock = 1;
        }
        if (point->gesture_vlock != 0) {
            struct wlr_output *output =
                    wlr_output_layout_output_at(p->output_layout, lx, ly);
            if (output != NULL) {
                struct wlr_box box;
                wlr_output_layout_get_box(p->output_layout, output, &box);
                // Each 5% of output height crossed is one step; downward is
                // positive (matches the *Down triggers), upward negative.
                int target = box.height > 0
                        ? (int)(dy * 20.0 / box.height) : 0;
                uint32_t down_trigger = point->gesture_edge == -1 ? 18 : 22;
                uint32_t up_trigger = point->gesture_edge == -1 ? 17 : 21;
                step_toward(p, point, target, down_trigger, up_trigger);
            }
            return;
        }
        // Horizontal edge-in: back (left edge, swipe right/inward) or
        // forward (right edge, swipe left/inward). Each 5% of output width
        // crossed is one step; "inward" is positive so reversing back
        // toward the edge — even past the start, into the other edge's
        // territory — fires the paired action to compensate, letting a
        // single touch dial back and forth between the two without lifting.
        if (point->gesture_edge == -1 || point->gesture_edge == 1) {
            struct wlr_output *output =
                    wlr_output_layout_output_at(p->output_layout, lx, ly);
            if (output != NULL) {
                struct wlr_box box;
                wlr_output_layout_get_box(p->output_layout, output, &box);
                double inward = point->gesture_edge == -1 ? dx : -dx;
                int target = box.width > 0
                        ? (int)(inward * 20.0 / box.width) : 0;
                uint32_t in_trigger = point->gesture_edge == -1 ? 2 : 3;
                uint32_t out_trigger = point->gesture_edge == -1 ? 3 : 2;
                step_toward(p, point, target, in_trigger, out_trigger);
            }
        }
        return;
    }
    if (point->gesture_kind == 3) {
        double dx = lx - point->start_lx;
        double dy = ly - point->start_ly;
        // Commit to horizontal (brightness) after a small deliberate
        // movement. Each 5% of output width crossed is one step; the FP5
        // maps a step to 5%, making an edge-to-edge swipe span 100%.
        // gesture_edge here just marks the commit (not a locked direction,
        // unlike kind 2's reuse of the same field) — direction stays
        // reversible, same as kind 2's stepped gestures (see step_toward):
        // reversing mid-swipe fires the paired trigger to walk brightness
        // back down rather than only ever advancing.
        if (point->gesture_edge == 0 && fabs(dx) >= 30
                && fabs(dx) > fabs(dy)) {
            point->gesture_edge = 1;
        }
        if (point->gesture_edge != 0) {
            struct wlr_output *output =
                    wlr_output_layout_output_at(p->output_layout, lx, ly);
            if (output != NULL) {
                struct wlr_box box;
                wlr_output_layout_get_box(p->output_layout, output, &box);
                // Rightward is positive (matches top-right/brightness+).
                int target = box.width > 0
                        ? (int)(dx * 20.0 / box.width) : 0;
                step_toward(p, point, target, 4, 5);
            }
            return;
        }
        bool down = dy >= 70 && (p->gesture_mask & (1u << 6)) != 0;
        if (!point->gesture_fired && down) {
            point->gesture_fired = true;
            if (p->gesture_callback != NULL) {
                p->gesture_callback(p->gesture_userdata, 6);
            }
        }
        return;
    }
    // The first finger of an undecided pair: still an entirely ordinary
    // client touch (forwarded below, same as always), but also feeds
    // classification so a swipe or pinch can still be recognized against the
    // held second finger. A swipe frees this point via multi_promote_to_active
    // — bail out immediately rather than touch it again.
    if (p->multi_pending && p->pending_first == point) {
        int i = multi_index(p, e->touch_id);
        if (i >= 0) {
            p->multi_x[i] = lx;
            p->multi_y[i] = ly;
            multi_pending_motion(p);
            if (p->multi_active) {
                return;
            }
        }
    }
    point->last_lx = lx;
    point->last_ly = ly;
    wlr_seat_touch_notify_motion(p->seat, e->time_msec, e->touch_id,
            lx - point->offset_x, ly - point->offset_y);
}

void handle_touch_up(void *userdata, void *data) {
    struct oxide_pointer *p = userdata;
    struct wlr_touch_up_event *e = data;
    if (p->multi_active) {
        int i = multi_index(p, e->touch_id);
        if (i >= 0) {
            p->multi_down[i] = false;
            p->multi_active_count--;
            if (p->multi_active_count == 0) {
                multi_two_finger_tap_check(p, e->time_msec);
                multi_reset(p);
            }
            return;
        }
    }
    if (p->multi_pending) {
        int i = multi_index(p, e->touch_id);
        if (i >= 0) {
            p->multi_down[i] = false;
            p->multi_active_count--;
            if (p->pending_first != NULL
                    && p->pending_first->touch_id == e->touch_id) {
                // Was a live client touch the whole time — it needs its own
                // real up, same as any ordinary gesture_kind-0 point.
                wlr_seat_touch_notify_up(p->seat, e->time_msec, e->touch_id);
                wl_list_remove(&p->pending_first->link);
                free(p->pending_first);
                p->pending_first = NULL;
            } else if (p->pending_second != NULL
                    && p->pending_second->touch_id == e->touch_id) {
                // Never delivered — nothing to notify.
                wl_list_remove(&p->pending_second->link);
                free(p->pending_second);
                p->pending_second = NULL;
            }
            // Mirrors the multi_active branch above: only act once *both*
            // fingers are up. Fingers essentially never lift at the same
            // instant, so acting on the first one alone would tear down the
            // pair (and skip the tap check below) before the second finger's
            // up event — which is the ordinary shape of every two-finger tap
            // — even arrives. Whichever finger is still down after this one
            // lifts is simply left as-is (pending_first, if it's the one
            // remaining, keeps running as an ordinary live touch;
            // pending_second, if it's the one remaining, just sits held
            // until it too lifts).
            if (p->multi_active_count == 0) {
                // Both lifted without ever resolving to a swipe or a pinch —
                // check for a two-finger tap exactly as the promoted path
                // does; neither finger reached the client, so there's
                // nothing else this sequence could still become.
                multi_two_finger_tap_check(p, e->time_msec);
                p->multi_pending = false;
                multi_reset(p);
            }
            return;
        }
    }
    struct oxide_touch_point *point = touch_point_find(p, e->touch_id);
    if (point == NULL) {
        return;
    }
    if (point->gesture_kind == 5) {
        // Lifted before the hold timer fired or the swipe committed — a
        // quick tap. Release it (forwarding the held-back down, using its
        // real down time) and, if that found a surface, immediately
        // follow with the up too, completing the tap.
        if (point->hold_timer != NULL) {
            oxide_event_source_remove(point->hold_timer);
            point->hold_timer = NULL;
        }
        release_hold(p, point);
    }
    if (point->gesture_kind == 0) {
        wlr_seat_touch_notify_up(p->seat, e->time_msec, e->touch_id);
    }
    wl_list_remove(&point->link);
    free(point);
}

void handle_touch_cancel(void *userdata, void *data) {
    struct oxide_pointer *p = userdata;
    struct wlr_touch_cancel_event *e = data;
    if (p->multi_active && multi_index(p, e->touch_id) >= 0) {
        multi_reset(p);
        return;
    }
    if (p->multi_pending && multi_index(p, e->touch_id) >= 0) {
        bool cancelled_first = p->pending_first != NULL
                && p->pending_first->touch_id == e->touch_id;
        if (cancelled_first) {
            // pending_first has a real seat-side touch point (it was
            // delivered normally) — stop tracking the pair first, then let
            // the ordinary per-client cancel path free it, exactly as an
            // ordinary gesture_kind-0 touch would.
            struct wlr_seat_client *client = p->pending_first->client;
            multi_pending_abandon(p);
            if (client != NULL) {
                touch_cancel_client(p, client);
            }
        } else {
            // pending_second was never delivered to any client — nothing
            // else needs to run for it.
            multi_pending_abandon(p);
        }
        return;
    }
    struct wlr_touch_point *seat_point =
            wlr_seat_touch_get_point(p->seat, e->touch_id);
    if (seat_point == NULL) {
        struct oxide_touch_point *point = touch_point_find(p, e->touch_id);
        if (point != NULL && point->gesture_kind != 0) {
            if (point->gesture_kind == 5 && point->hold_timer != NULL) {
                oxide_event_source_remove(point->hold_timer);
            }
            wl_list_remove(&point->link);
            free(point);
        }
        return;
    }
    // A Wayland cancel ends every point belonging to that seat client.
    touch_cancel_client(p, seat_point->client);
}

void handle_touch_frame(void *userdata, void *data) {
    (void)data;
    struct oxide_pointer *p = userdata;
    wlr_seat_touch_notify_frame(p->seat);
}

void handle_touch_device_destroy(void *userdata, void *data) {
    (void)data;
    struct oxide_touch_device *td = userdata;
    struct oxide_pointer *p = td->pointer;
    // A group can span only the currently attached touchscreen in practice;
    // abandon it if that device disappears mid-gesture. The loops below free
    // the underlying points as usual (pending_first via the kind-0 client
    // cancel, pending_second via the kind-!=0 sweep) — this just clears the
    // bookkeeping pointers so nothing is left dangling.
    multi_reset(p);
    p->multi_pending = false;
    p->pending_first = NULL;
    p->pending_second = NULL;

    // Device destruction is allowed without a preceding cancel (notably while
    // a session is paused). Cancel each affected client once; canceling a
    // client removes all of its points, including any from another device.
    while (true) {
        struct oxide_touch_point *point;
        struct wlr_seat_client *client = NULL;
        wl_list_for_each(point, &p->touch_points, link) {
            if (point->touch == td->touch && point->gesture_kind == 0) {
                client = point->client;
                break;
            }
        }
        if (client == NULL) {
            break;
        }
        touch_cancel_client(p, client);
    }
    struct oxide_touch_point *point, *tmp;
    wl_list_for_each_safe(point, tmp, &p->touch_points, link) {
        if (point->touch == td->touch && point->gesture_kind != 0) {
            if (point->gesture_kind == 5 && point->hold_timer != NULL) {
                oxide_event_source_remove(point->hold_timer);
            }
            wl_list_remove(&point->link);
            free(point);
        }
    }

    wlr_cursor_detach_input_device(p->cursor, td->device);
    oxide_listener_remove(td->destroy_listener);
    if (p->touch_device_count > 0) {
        p->touch_device_count--;
    }
    if (p->touch_device_count == 0) {
        wlr_seat_set_capabilities(p->seat,
                p->seat->capabilities & ~WL_SEAT_CAPABILITY_TOUCH);
    }
    free(td);
    wlr_log(WLR_INFO, "0xin: touch removed");
}

