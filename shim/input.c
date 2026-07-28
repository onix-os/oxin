#define WLR_USE_UNSTABLE
#include <stdlib.h>
#include <xkbcommon/xkbcommon.h>
#include <wlr/backend.h>
#include <wlr/types/wlr_compositor.h>
#include <wlr/types/wlr_cursor.h>
#include <wlr/types/wlr_input_device.h>
#include <wlr/types/wlr_keyboard.h>
#include <wlr/types/wlr_pointer.h>
#include <wlr/types/wlr_scene.h>
#include <wlr/types/wlr_seat.h>
#include <wlr/types/wlr_touch.h>
#include <wlr/types/wlr_xcursor_manager.h>
#include <wlr/util/log.h>

#include "oxide_shim_internal.h"

// --- seat & input ----------------------------------------------------------

struct wlr_seat *oxide_seat_create(struct wl_display *display, const char *name) {
    struct wlr_seat *seat = wlr_seat_create(display, name);
    // Advertise input capabilities so clients (e.g. foot) will start.
    wlr_seat_set_capabilities(seat,
            WL_SEAT_CAPABILITY_KEYBOARD | WL_SEAT_CAPABILITY_POINTER);
    return seat;
}

// Per-keyboard context so the key/modifier handlers can reach the seat and the
// Rust keybinding callback. We track our listeners so we can remove them when
// the device is destroyed (e.g. on VT switch, when logind pauses input) —
// otherwise wlroots asserts the keyboard's signal lists aren't empty.
struct oxide_keyboard {
    struct wlr_seat *seat;
    struct wlr_keyboard *keyboard;
    oxide_key_callback key_callback;
    void *key_userdata;
    struct oxide_listener *key_listener;
    struct oxide_listener *mod_listener;
    struct oxide_listener *destroy_listener;
};

static void handle_key(void *userdata, void *data) {
    struct oxide_keyboard *kb = userdata;
    struct wlr_keyboard_key_event *event = data;

    // Offer the press to Rust as a possible keybinding first. wlroots keycodes
    // are offset by 8 from xkb keycodes.
    bool handled = false;
    if (event->state == WL_KEYBOARD_KEY_STATE_PRESSED && kb->key_callback != NULL) {
        uint32_t keycode = event->keycode + 8;
        // Match bindings on the layout level-0 (unshifted) keysym, so e.g.
        // Mod+Shift+1 reads as '1' (+Shift modifier), not the shifted '!'.
        xkb_layout_index_t layout =
                xkb_state_key_get_layout(kb->keyboard->xkb_state, keycode);
        const xkb_keysym_t *syms;
        int nsyms = xkb_keymap_key_get_syms_by_level(kb->keyboard->keymap,
                keycode, layout, 0, &syms);
        uint32_t modifiers = wlr_keyboard_get_modifiers(kb->keyboard);
        for (int i = 0; i < nsyms; i++) {
            if (kb->key_callback(kb->key_userdata, syms[i], modifiers)) {
                handled = true;
            }
        }
    }

    // Unhandled keys go to the focused client.
    if (!handled) {
        wlr_seat_set_keyboard(kb->seat, kb->keyboard);
        wlr_seat_keyboard_notify_key(kb->seat, event->time_msec, event->keycode,
                event->state);
    }
}

static void handle_modifiers(void *userdata, void *data) {
    (void)data;
    struct oxide_keyboard *kb = userdata;
    wlr_seat_set_keyboard(kb->seat, kb->keyboard);
    wlr_seat_keyboard_notify_modifiers(kb->seat, &kb->keyboard->modifiers);
}

// The input device is going away (unplugged, or paused on a VT switch). Detach
// our listeners before wlroots tears the keyboard down, then free our context.
static void handle_keyboard_destroy(void *userdata, void *data) {
    (void)data;
    struct oxide_keyboard *kb = userdata;
    oxide_listener_remove(kb->key_listener);
    oxide_listener_remove(kb->mod_listener);
    oxide_listener_remove(kb->destroy_listener);
    free(kb);
    wlr_log(WLR_INFO, "0xide: keyboard removed");
}

static void seat_add_keyboard(struct wlr_seat *seat,
        struct wlr_input_device *device, oxide_key_callback key_callback,
        void *key_userdata) {
    struct wlr_keyboard *keyboard = wlr_keyboard_from_input_device(device);

    // Compile scancodes -> keysyms with the default (locale/us) layout.
    struct xkb_context *context = xkb_context_new(XKB_CONTEXT_NO_FLAGS);
    struct xkb_keymap *keymap =
            xkb_keymap_new_from_names(context, NULL, XKB_KEYMAP_COMPILE_NO_FLAGS);
    wlr_keyboard_set_keymap(keyboard, keymap);
    xkb_keymap_unref(keymap);
    xkb_context_unref(context);
    wlr_keyboard_set_repeat_info(keyboard, 25, 600);

    struct oxide_keyboard *kb = calloc(1, sizeof(*kb));
    kb->seat = seat;
    kb->keyboard = keyboard;
    kb->key_callback = key_callback;
    kb->key_userdata = key_userdata;
    kb->key_listener = signal_add(&keyboard->events.key, handle_key, kb);
    kb->mod_listener = signal_add(&keyboard->events.modifiers, handle_modifiers, kb);
    // Device-level destroy, so we clean up when the keyboard is removed.
    kb->destroy_listener = signal_add(&device->events.destroy, handle_keyboard_destroy, kb);

    wlr_seat_set_keyboard(seat, keyboard);
    wlr_log(WLR_INFO, "0xide: keyboard attached");
}

struct oxide_listener *oxide_backend_add_new_input(
        struct wlr_backend *backend, oxide_callback callback, void *userdata) {
    return signal_add(&backend->events.new_input, callback, userdata);
}

// --- pointer / cursor ------------------------------------------------------

// Bundles everything the cursor event handlers need.
struct oxide_pointer {
    struct wlr_cursor *cursor;
    struct wlr_xcursor_manager *cursor_mgr;
    struct wlr_scene *scene;
    struct wlr_seat *seat;
    // Rust click-focus hook: called with the clicked root wlr_surface so the
    // Rust side can keep its own focus bookkeeping in sync with the seat.
    oxide_callback focus_callback;
    void *focus_userdata;
    // Rust pointer-grab hooks (Mod+drag move/resize of floating windows).
    // The button hook decides whether a press starts / a release ends a grab;
    // the motion hook applies an active grab. Either returning true means
    // "this event is the grab's — don't forward it to any client".
    oxide_grab_button_callback grab_button_callback;
    oxide_grab_motion_callback grab_motion_callback;
    void *grab_userdata;
    // Active touch points keep the layout-to-surface offset established on
    // touch-down. Wayland keeps a touch sequence on that original surface,
    // even after the finger moves over another one.
    struct wl_list touch_points;
    size_t touch_device_count;
};

struct oxide_touch_point {
    int32_t touch_id;
    double offset_x, offset_y;
    struct wlr_seat_client *client;
    struct wlr_touch *touch;
    struct wl_list link;
};

struct oxide_touch_device {
    struct oxide_pointer *pointer;
    struct wlr_input_device *device;
    struct wlr_touch *touch;
    struct oxide_listener *destroy_listener;
};

// Find the surface at layout coordinates (and its surface-local coords), via
// the scene graph. Returns NULL over the bare background.
static struct wlr_surface *surface_at_coords(struct oxide_pointer *p,
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
// sync for pointer clicks and touch taps.
static void focus_surface(struct oxide_pointer *p, struct wlr_surface *surface) {
    struct wlr_keyboard *kb = wlr_seat_get_keyboard(p->seat);
    if (surface != NULL && kb != NULL) {
        wlr_seat_keyboard_notify_enter(p->seat, surface, kb->keycodes,
                kb->num_keycodes, &kb->modifiers);
    }
    struct wlr_surface *root =
            surface != NULL ? wlr_surface_get_root_surface(surface) : NULL;
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

static void handle_cursor_motion(void *userdata, void *data) {
    struct oxide_pointer *p = userdata;
    struct wlr_pointer_motion_event *e = data;
    wlr_cursor_move(p->cursor, &e->pointer->base, e->delta_x, e->delta_y);
    process_motion(p, e->time_msec);
}

static void handle_cursor_motion_absolute(void *userdata, void *data) {
    struct oxide_pointer *p = userdata;
    struct wlr_pointer_motion_absolute_event *e = data;
    wlr_cursor_warp_absolute(p->cursor, &e->pointer->base, e->x, e->y);
    process_motion(p, e->time_msec);
}

static void handle_cursor_button(void *userdata, void *data) {
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

static void handle_cursor_axis(void *userdata, void *data) {
    struct oxide_pointer *p = userdata;
    struct wlr_pointer_axis_event *e = data;
    wlr_seat_pointer_notify_axis(p->seat, e->time_msec, e->orientation, e->delta,
            e->delta_discrete, e->source, e->relative_direction);
}

static void handle_cursor_frame(void *userdata, void *data) {
    (void)data;
    struct oxide_pointer *p = userdata;
    wlr_seat_pointer_notify_frame(p->seat);
}

// --- touch -----------------------------------------------------------------

static struct oxide_touch_point *touch_point_find(
        struct oxide_pointer *p, int32_t touch_id) {
    struct oxide_touch_point *point;
    wl_list_for_each(point, &p->touch_points, link) {
        if (point->touch_id == touch_id) {
            return point;
        }
    }
    return NULL;
}

static void touch_cancel_client(struct oxide_pointer *p,
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

static void handle_touch_down(void *userdata, void *data) {
    struct oxide_pointer *p = userdata;
    struct wlr_touch_down_event *e = data;
    if (touch_point_find(p, e->touch_id) != NULL) {
        wlr_log(WLR_ERROR, "0xide: duplicate touch ID %d ignored",
                e->touch_id);
        return;
    }
    double lx, ly, sx, sy;
    wlr_cursor_absolute_to_layout_coords(p->cursor, &e->touch->base,
            e->x, e->y, &lx, &ly);
    struct wlr_surface *surface =
            surface_at_coords(p, lx, ly, &sx, &sy);
    if (surface == NULL) {
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
    wl_list_insert(&p->touch_points, &point->link);
}

static void handle_touch_motion(void *userdata, void *data) {
    struct oxide_pointer *p = userdata;
    struct wlr_touch_motion_event *e = data;
    struct oxide_touch_point *point = touch_point_find(p, e->touch_id);
    if (point == NULL) {
        return;
    }
    double lx, ly;
    wlr_cursor_absolute_to_layout_coords(p->cursor, &e->touch->base,
            e->x, e->y, &lx, &ly);
    wlr_seat_touch_notify_motion(p->seat, e->time_msec, e->touch_id,
            lx - point->offset_x, ly - point->offset_y);
}

static void handle_touch_up(void *userdata, void *data) {
    struct oxide_pointer *p = userdata;
    struct wlr_touch_up_event *e = data;
    struct oxide_touch_point *point = touch_point_find(p, e->touch_id);
    if (point == NULL) {
        return;
    }
    wlr_seat_touch_notify_up(p->seat, e->time_msec, e->touch_id);
    wl_list_remove(&point->link);
    free(point);
}

static void handle_touch_cancel(void *userdata, void *data) {
    struct oxide_pointer *p = userdata;
    struct wlr_touch_cancel_event *e = data;
    struct wlr_touch_point *seat_point =
            wlr_seat_touch_get_point(p->seat, e->touch_id);
    if (seat_point == NULL) {
        return;
    }
    // A Wayland cancel ends every point belonging to that seat client.
    touch_cancel_client(p, seat_point->client);
}

static void handle_touch_frame(void *userdata, void *data) {
    (void)data;
    struct oxide_pointer *p = userdata;
    wlr_seat_touch_notify_frame(p->seat);
}

static void handle_touch_device_destroy(void *userdata, void *data) {
    (void)data;
    struct oxide_touch_device *td = userdata;
    struct oxide_pointer *p = td->pointer;

    // Device destruction is allowed without a preceding cancel (notably while
    // a session is paused). Cancel each affected client once; canceling a
    // client removes all of its points, including any from another device.
    while (true) {
        struct oxide_touch_point *point;
        struct wlr_seat_client *client = NULL;
        wl_list_for_each(point, &p->touch_points, link) {
            if (point->touch == td->touch) {
                client = point->client;
                break;
            }
        }
        if (client == NULL) {
            break;
        }
        touch_cancel_client(p, client);
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
    wlr_log(WLR_INFO, "0xide: touch removed");
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
    p->touch_device_count++;
    wlr_seat_set_capabilities(p->seat,
            p->seat->capabilities | WL_SEAT_CAPABILITY_TOUCH);
    wlr_log(WLR_INFO, "0xide: touch attached");
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

void oxide_handle_new_input(struct wlr_seat *seat, struct wlr_cursor *cursor,
        struct wlr_input_device *device, oxide_key_callback key_callback,
        void *key_userdata) {
    switch (device->type) {
    case WLR_INPUT_DEVICE_KEYBOARD:
        seat_add_keyboard(seat, device, key_callback, key_userdata);
        break;
    case WLR_INPUT_DEVICE_POINTER:
        wlr_cursor_attach_input_device(cursor, device);
        wlr_log(WLR_INFO, "0xide: pointer attached");
        break;
    case WLR_INPUT_DEVICE_TOUCH:
        pointer_add_touch(cursor->data, device);
        break;
    default:
        break;
    }
}
