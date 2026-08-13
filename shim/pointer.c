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

// --- pointer / cursor ------------------------------------------------------

// Find the surface at layout coordinates (and its surface-local coords), via
// the scene graph. Returns NULL over the bare background.
struct wlr_surface *surface_at_coords(struct oxide_pointer *p,
        double lx, double ly, double *sx, double *sy) {
    struct wlr_scene_node *node = wlr_scene_node_at(&p->scene->tree.node,
            lx, ly, sx, sy);
    if (node == NULL || node->type != WLR_SCENE_NODE_BUFFER) {
        return NULL;
    }
    struct wlr_scene_surface *scene_surface =
            wlr_scene_surface_try_from_buffer(wlr_scene_buffer_from_node(node));
    return scene_surface ? scene_surface->surface : NULL;
}

static struct wlr_surface *surface_at(struct oxide_pointer *p,
        double *sx, double *sy) {
    return surface_at_coords(p, p->cursor->x, p->cursor->y, sx, sy);
}

// Keep both Wayland keyboard focus and Rust's focused-window bookkeeping in
// sync for pointer clicks and touch taps. Ordinary layer surfaces (notably an
// on-screen keyboard) must not take focus from the app they are typing into.
void focus_surface(struct oxide_pointer *p, struct wlr_surface *surface) {
    struct wlr_surface *root =
            surface != NULL ? wlr_surface_get_root_surface(surface) : NULL;
    struct wlr_layer_surface_v1 *layer = root != NULL
            ? wlr_layer_surface_v1_try_from_wlr_surface(root) : NULL;
    bool focusable = root != NULL
            && (wlr_xdg_toplevel_try_from_wlr_surface(root) != NULL
                || wlr_session_lock_surface_v1_try_from_wlr_surface(root) != NULL
                || (layer != NULL && layer->current.keyboard_interactive
                    != ZWLR_LAYER_SURFACE_V1_KEYBOARD_INTERACTIVITY_NONE));
    if (!focusable) {
        return;
    }

    // Focus must transfer even with no currently-active keyboard device (e.g.
    // right after the seat's last one was destroyed and nothing has claimed
    // active status since) — otherwise a click/tap can never move keyboard
    // focus again until some keyboard device happens to send a key first.
    struct wlr_keyboard *kb = wlr_seat_get_keyboard(p->seat);
    if (kb != NULL) {
        wlr_seat_keyboard_notify_enter(p->seat, root, kb->keycodes,
                kb->num_keycodes, &kb->modifiers);
    } else {
        wlr_seat_keyboard_notify_enter(p->seat, root, NULL, 0, NULL);
    }
    if (root != NULL && p->focus_callback != NULL) {
        p->focus_callback(p->focus_userdata, root);
    }
}

static void process_motion(struct oxide_pointer *p, uint32_t time) {
    // An active grab owns the cursor: the grabbed window follows it and no
    // client sees enter/motion until the grab ends.
    if (p->grab_motion_callback != NULL
            && p->grab_motion_callback(p->grab_userdata, p->cursor->x,
                    p->cursor->y)) {
        wlr_cursor_set_xcursor(p->cursor, p->cursor_mgr, "grabbing");
        return;
    }
    double sx, sy;
    struct wlr_surface *surface = surface_at(p, &sx, &sy);
    if (surface == NULL) {
        // Over the background: show our own cursor, focus nothing.
        wlr_cursor_set_xcursor(p->cursor, p->cursor_mgr, "default");
        wlr_seat_pointer_clear_focus(p->seat);
    } else {
        wlr_seat_pointer_notify_enter(p->seat, surface, sx, sy);
        wlr_seat_pointer_notify_motion(p->seat, time, sx, sy);
    }
}

void handle_cursor_motion(void *userdata, void *data) {
    struct oxide_pointer *p = userdata;
    struct wlr_pointer_motion_event *e = data;
    wlr_cursor_move(p->cursor, &e->pointer->base, e->delta_x, e->delta_y);
    process_motion(p, e->time_msec);
}

void handle_cursor_motion_absolute(void *userdata, void *data) {
    struct oxide_pointer *p = userdata;
    struct wlr_pointer_motion_absolute_event *e = data;
    wlr_cursor_warp_absolute(p->cursor, &e->pointer->base, e->x, e->y);
    process_motion(p, e->time_msec);
}

void handle_cursor_button(void *userdata, void *data) {
    struct oxide_pointer *p = userdata;
    struct wlr_pointer_button_event *e = data;
    if (e->state == WL_POINTER_BUTTON_STATE_PRESSED) {
        // Click-to-focus: give keyboard focus to the window under the cursor.
        double sx, sy;
        struct wlr_surface *surface = surface_at(p, &sx, &sy);
        struct wlr_keyboard *kb = wlr_seat_get_keyboard(p->seat);
        focus_surface(p, surface);
        struct wlr_surface *root =
                surface != NULL ? wlr_surface_get_root_surface(surface) : NULL;
        // Offer the press to Rust as a possible grab start (Mod+click on a
        // floating window). A consumed press never reaches the client.
        if (p->grab_button_callback != NULL) {
            uint32_t mods = kb != NULL ? wlr_keyboard_get_modifiers(kb) : 0;
            if (p->grab_button_callback(p->grab_userdata, root, e->button,
                    mods, true, p->cursor->x, p->cursor->y)) {
                return;
            }
        }
    } else {
        // A release ends an active grab and is swallowed with it — the
        // client never saw the press, so it must not see the release.
        if (p->grab_button_callback != NULL
                && p->grab_button_callback(p->grab_userdata, NULL, e->button,
                        0, false, p->cursor->x, p->cursor->y)) {
            return;
        }
    }
    wlr_seat_pointer_notify_button(p->seat, e->time_msec, e->button, e->state);
}

void handle_cursor_axis(void *userdata, void *data) {
    struct oxide_pointer *p = userdata;
    struct wlr_pointer_axis_event *e = data;
    wlr_seat_pointer_notify_axis(p->seat, e->time_msec, e->orientation, e->delta,
            e->delta_discrete, e->source, e->relative_direction);
}

void handle_cursor_frame(void *userdata, void *data) {
    (void)data;
    struct oxide_pointer *p = userdata;
    wlr_seat_pointer_notify_frame(p->seat);
}


static void pointer_add_touch(struct oxide_pointer *p,
        struct wlr_input_device *device) {
    struct oxide_touch_device *td = calloc(1, sizeof(*td));
    td->pointer = p;
    td->device = device;
    td->touch = wlr_touch_from_input_device(device);
    td->destroy_listener = signal_add(&device->events.destroy,
            handle_touch_device_destroy, td);

    wlr_cursor_attach_input_device(p->cursor, device);

    // Map the touchscreen to the sole output when there's exactly one (the
    // phone profile). This is what makes wlr_cursor_absolute_to_layout_coords
    // account for the output's transform — without an explicit mapping it
    // falls back to scaling raw touch fractions against the whole layout's
    // box, which isn't transform-aware. Multi-output desktop profiles are
    // left unmapped and unaffected.
    if (p->output_layout != NULL
            && wl_list_length(&p->output_layout->outputs) == 1) {
        struct wlr_output_layout_output *lo = wl_container_of(
                p->output_layout->outputs.next, lo, link);
        wlr_cursor_map_input_to_output(p->cursor, device, lo->output);
    }

    p->touch_device_count++;
    wlr_seat_set_capabilities(p->seat,
            p->seat->capabilities | WL_SEAT_CAPABILITY_TOUCH);
    wlr_log(WLR_INFO, "0xin: touch attached");
}

struct wlr_cursor *oxide_cursor_setup(struct wlr_output_layout *layout,
        struct wlr_scene *scene, struct wlr_seat *seat) {
    struct wlr_cursor *cursor = wlr_cursor_create();
    wlr_cursor_attach_output_layout(cursor, layout);

    struct wlr_xcursor_manager *cursor_mgr = wlr_xcursor_manager_create(NULL, 24);
    wlr_xcursor_manager_load(cursor_mgr, 1);

    struct oxide_pointer *p = calloc(1, sizeof(*p));
    p->cursor = cursor;
    p->cursor_mgr = cursor_mgr;
    p->scene = scene;
    p->seat = seat;
    wl_list_init(&p->touch_points);

    signal_add(&cursor->events.motion, handle_cursor_motion, p);
    signal_add(&cursor->events.motion_absolute, handle_cursor_motion_absolute, p);
    signal_add(&cursor->events.button, handle_cursor_button, p);
    signal_add(&cursor->events.axis, handle_cursor_axis, p);
    signal_add(&cursor->events.frame, handle_cursor_frame, p);
    signal_add(&cursor->events.touch_down, handle_touch_down, p);
    signal_add(&cursor->events.touch_motion, handle_touch_motion, p);
    signal_add(&cursor->events.touch_up, handle_touch_up, p);
    signal_add(&cursor->events.touch_cancel, handle_touch_cancel, p);
    signal_add(&cursor->events.touch_frame, handle_touch_frame, p);

    // Stash our context on the cursor so oxide_cursor_set_focus_callback can
    // find it later (the Rust Server, the callback's userdata, doesn't exist
    // yet when the cursor is created).
    cursor->data = p;

    return cursor;
}

// Register the Rust click-focus hook (see handle_cursor_button). Separate
// from oxide_cursor_setup because the Server pointer used as userdata is
// only constructed after the cursor.
void oxide_cursor_set_focus_callback(struct wlr_cursor *cursor,
        oxide_callback callback, void *userdata) {
    struct oxide_pointer *p = cursor->data;
    p->focus_callback = callback;
    p->focus_userdata = userdata;
}

// Register the Rust double-tap hook (same late-registration story as the
// focus callback above). Fires with the tapped root wlr_surface — reuses the
// generic oxide_callback shape, same as the focus callback, rather than a
// bespoke type for this one extra trigger.
void oxide_cursor_set_double_tap_callback(struct wlr_cursor *cursor,
        oxide_callback callback, void *userdata) {
    struct oxide_pointer *p = cursor->data;
    p->double_tap_callback = callback;
    p->double_tap_userdata = userdata;
}

// Register the Rust pointer-grab hooks (same late-registration story as the
// focus callback above).
void oxide_cursor_set_grab_callbacks(struct wlr_cursor *cursor,
        oxide_grab_button_callback button_callback,
        oxide_grab_motion_callback motion_callback, void *userdata) {
    struct oxide_pointer *p = cursor->data;
    p->grab_button_callback = button_callback;
    p->grab_motion_callback = motion_callback;
    p->grab_userdata = userdata;
}

void oxide_cursor_set_gestures(struct wlr_cursor *cursor,
        struct wlr_output_layout *layout, uint32_t enabled_mask,
        int keyboard_height, struct wl_event_loop *event_loop,
        oxide_gesture_callback callback, void *userdata) {
    struct oxide_pointer *p = cursor->data;
    p->output_layout = layout;
    p->gesture_mask = enabled_mask;
    p->configured_gesture_mask = enabled_mask;
    p->keyboard_height = keyboard_height;
    p->event_loop = event_loop;
    p->gesture_callback = callback;
    p->gesture_userdata = userdata;
}

void oxide_cursor_set_locked(struct wlr_cursor *cursor, bool locked) {
    struct oxide_pointer *p = cursor->data;
    p->gesture_mask = locked ? 0 : p->configured_gesture_mask;
    if (!locked) {
        return;
    }
    multi_reset(p);
    while (!wl_list_empty(&p->touch_points)) {
        struct oxide_touch_point *point =
                wl_container_of(p->touch_points.next, point, link);
        if (point->client != NULL) {
            touch_cancel_client(p, point->client);
        } else {
            if (point->gesture_kind == 5 && point->hold_timer != NULL) {
                oxide_event_source_remove(point->hold_timer);
            }
            wl_list_remove(&point->link);
            free(point);
        }
    }
}

void oxide_cursor_set_keyboard_visible(struct wlr_cursor *cursor,
        bool visible) {
    struct oxide_pointer *p = cursor->data;
    p->keyboard_visible = visible;
}

void oxide_cursor_set_keyboard_height(struct wlr_cursor *cursor, int height) {
    struct oxide_pointer *p = cursor->data;
    if (height > 0) {
        p->keyboard_height = height;
    }
}

void oxide_handle_new_input(struct wlr_seat *seat, struct wlr_cursor *cursor,
        struct wlr_input_device *device, oxide_key_callback key_callback,
        void *key_userdata) {
    switch (device->type) {
    case WLR_INPUT_DEVICE_KEYBOARD:
        seat_add_keyboard(seat, device, key_callback, key_userdata);
        break;
    case WLR_INPUT_DEVICE_POINTER:
        wlr_cursor_attach_input_device(cursor, device);
        wlr_log(WLR_INFO, "0xin: pointer attached");
        break;
    case WLR_INPUT_DEVICE_TOUCH:
        pointer_add_touch(cursor->data, device);
        break;
    default:
        break;
    }
}
