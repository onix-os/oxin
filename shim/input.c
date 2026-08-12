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

#include "oxide_shim_internal.h"

// Double-tap thresholds: max travel for a touch to still count as a tap, and
// the max time/distance gap between two taps for them to count as a pair.
#define OXIDE_TAP_DRAG_PX 24
#define OXIDE_DOUBLE_TAP_MS 400
#define OXIDE_DOUBLE_TAP_PX 100
// How long a touch landing on the visible keyboard is held before being
// forwarded as a real keypress, giving a swipe-down-to-hide gesture a
// chance to claim it first instead — see keyboard_hide_candidate. Long
// enough for an unhurried swipe (the keyboard itself is only ~125 logical
// px tall, so a fast flick isn't required) without making ordinary taps
// feel delayed.
#define OXIDE_KEYBOARD_HOLD_MS 220

// --- seat & input ----------------------------------------------------------

// A client sets the seat's clipboard/primary selection by asking for it
// (request_set_*_selection); nothing actually holds the selection until the
// compositor confirms it back with wlr_seat_set_*_selection(). Without these,
// every copy is silently accepted and then goes nowhere — paste has nothing
// to receive.
static void handle_request_set_selection(void *userdata, void *data) {
    struct wlr_seat *seat = userdata;
    struct wlr_seat_request_set_selection_event *event = data;
    wlr_seat_set_selection(seat, event->source, event->serial);
}

static void handle_request_set_primary_selection(void *userdata, void *data) {
    struct wlr_seat *seat = userdata;
    struct wlr_seat_request_set_primary_selection_event *event = data;
    wlr_seat_set_primary_selection(seat, event->source, event->serial);
}

struct wlr_seat *oxide_seat_create(struct wl_display *display, const char *name) {
    struct wlr_seat *seat = wlr_seat_create(display, name);
    // Advertise input capabilities so clients (e.g. foot) will start.
    wlr_seat_set_capabilities(seat,
            WL_SEAT_CAPABILITY_KEYBOARD | WL_SEAT_CAPABILITY_POINTER);
    signal_add(&seat->events.request_set_selection,
            handle_request_set_selection, seat);
    signal_add(&seat->events.request_set_primary_selection,
            handle_request_set_primary_selection, seat);
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

    // Offer the event to Rust as a possible keybinding first. Releases matter
    // for long-press cancellation. wlroots keycodes
    // are offset by 8 from xkb keycodes.
    bool handled = false;
    if (kb->key_callback != NULL) {
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
            bool pressed = event->state == WL_KEYBOARD_KEY_STATE_PRESSED;
            if (kb->key_callback(kb->key_userdata, syms[i], modifiers, pressed)) {
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
    if (wlr_seat_get_keyboard(kb->seat) == kb->keyboard) {
        wlr_seat_set_keyboard(kb->seat, NULL);
    }
    oxide_listener_remove(kb->key_listener);
    oxide_listener_remove(kb->mod_listener);
    oxide_listener_remove(kb->destroy_listener);
    free(kb);
    wlr_log(WLR_INFO, "0xin: keyboard removed");
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
    wlr_log(WLR_INFO, "0xin: keyboard attached");
}

static void handle_new_virtual_keyboard(void *userdata, void *data) {
    struct wlr_seat *seat = userdata;
    struct wlr_virtual_keyboard_v1 *virtual_keyboard = data;
    // Virtual keys are client input, never compositor keybindings. Passing no
    // key callback makes seat_add_keyboard forward every key to seat focus.
    seat_add_keyboard(seat, &virtual_keyboard->keyboard.base, NULL, NULL);
    wlr_log(WLR_INFO, "0xin: virtual keyboard attached");
}

void oxide_virtual_keyboard_setup(struct wl_display *display,
        struct wlr_seat *seat) {
    struct wlr_virtual_keyboard_manager_v1 *manager =
            wlr_virtual_keyboard_manager_v1_create(display);
    signal_add(&manager->events.new_virtual_keyboard,
            handle_new_virtual_keyboard, seat);
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
    // Needed to arm the keyboard-hide-swipe hold timer — see
    // keyboard_hide_candidate/handle_keyboard_hold_timeout.
    struct wl_event_loop *event_loop;
    struct wlr_output_layout *output_layout;
    uint32_t gesture_mask;
    uint32_t configured_gesture_mask;
    bool keyboard_visible;
    int keyboard_height;
    oxide_gesture_callback gesture_callback;
    void *gesture_userdata;
    // Touchscreen multi-finger gesture, promoted from ordinary client touch
    // when a second configured finger arrives.
    bool multi_active;
    bool multi_fired;
    int multi_count;
    int multi_active_count;
    int32_t multi_ids[3];
    bool multi_down[3];
    double multi_start_x[3], multi_start_y[3];
    double multi_x[3], multi_y[3];
    // Undecided two-finger state: the first finger is still a live,
    // undisturbed client touch (pending_first, gesture_kind 0 or 4) while the
    // second is held back (pending_second, gesture_kind 6) until motion shows
    // whether this is a swipe (promote to multi_active, same as before), a
    // pinch (release pending_second to the client — see multi_pending_motion),
    // or a tap (falls through to the existing double-tap check unchanged).
    // Mutually exclusive with multi_active.
    bool multi_pending;
    struct oxide_touch_point *pending_first;
    struct oxide_touch_point *pending_second;
    // Double-tap: a compositor-owned window-identity gesture, requiring two
    // fingers (promoted through the multi_* tracking above, same as a
    // swipe) tapped together twice in roughly the same spot. See
    // multi_two_finger_tap_check and oxide_cursor_set_double_tap_callback.
    struct wlr_surface *last_tap_surface;
    double last_tap_lx, last_tap_ly;
    uint32_t last_tap_time_msec;
    oxide_callback double_tap_callback;
    void *double_tap_userdata;
};

struct oxide_touch_point {
    int32_t touch_id;
    double offset_x, offset_y;
    struct wlr_seat_client *client;
    struct wlr_touch *touch;
    // 0 = client touch, 1 = keyboard handle, 2 = workspace edge,
    // 3 = top-edge gesture, 4 = bare-background multi-finger candidate,
    // 5 = keyboard-hide-swipe candidate (held, not yet forwarded to any
    // client — see keyboard_hide_candidate), 6 = pending second finger of an
    // undecided two-finger gesture (held, not yet forwarded — see
    // multi_pending_begin/multi_pending_motion).
    int gesture_kind;
    bool gesture_fired;
    bool to_top_candidate;
    // Same idea, sideways: eligible to become a to-left/to-right edge
    // gesture (browser-style back/forward) if it travels far enough and
    // reaches close to a physical left or right edge. See to_top_candidate's
    // handling in handle_touch_motion for the shared pattern.
    bool to_edge_candidate;
    // Signed running step count for kind 2 and kind 3's stepped gestures
    // (volume, workspace, back/forward, brightness) — see step_toward. Can
    // go negative: reversing direction mid-touch fires the paired trigger to
    // walk this back down, rather than a one-way commit that can only ever
    // advance.
    int gesture_steps;
    // -1 for the left edge and +1 for the right edge.
    int gesture_edge;
    // Kind 2 only: 0 until the touch commits to vertical (volume/workspace)
    // rather than horizontal (back/forward) — see the fabs(dy) >= 30 check
    // below. Once nonzero, stays that way for the rest of the touch; the
    // stepping itself is bidirectional (see gesture_steps), only this
    // vertical-vs-horizontal decision is a one-way commit.
    int gesture_vlock;
    double start_lx, start_ly;
    // Position last seen in handle_touch_motion, and the time it was seen
    // at (needed to give a real timestamp to the catch-up motion event a
    // kind-5 point sends once released — see release_hold).
    double last_lx, last_ly;
    uint32_t last_time_msec;
    // kind 5 only: the owning pointer (needed inside the hold timer's
    // callback, which only receives this point as userdata), the pending
    // timer itself (NULL once fired/cancelled), and the touch's true
    // down time (used to give the delayed notify_down a real timestamp).
    struct oxide_pointer *owner;
    void *hold_timer;
    uint32_t hold_time_msec;
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
// sync for pointer clicks and touch taps. Ordinary layer surfaces (notably an
// on-screen keyboard) must not take focus from the app they are typing into.
static void focus_surface(struct oxide_pointer *p, struct wlr_surface *surface) {
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

static bool keyboard_gesture_hit(struct oxide_pointer *p, double lx, double ly) {
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

static int workspace_gesture_edge(struct oxide_pointer *p,
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

static bool top_gesture_hit(struct oxide_pointer *p, double lx, double ly) {
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

// True for a touch-down landing on the visible keyboard while the
// swipe-down-to-hide gesture is configured — see gesture_kind 5's doc
// comment above and OXIDE_KEYBOARD_HOLD_MS.
static bool keyboard_hide_candidate(struct oxide_pointer *p, double lx,
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
static void release_hold(struct oxide_pointer *p,
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
static void handle_keyboard_hold_timeout(void *userdata, void *data) {
    struct oxide_touch_point *point = userdata;
    oxide_event_source_remove(data);
    point->hold_timer = NULL;
    release_hold(point->owner, point);
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

static bool multi_gestures_enabled(struct oxide_pointer *p) {
    // Bits 8-15: two/three-finger swipes. Bit 16: double-tap, now a
    // two-finger gesture too — a second finger must promote to a compositor
    // gesture for it to have a chance of recognizing a tap, same as swipes.
    return (p->gesture_mask & 0x1ff00u) != 0;
}

static int multi_index(struct oxide_pointer *p, int32_t touch_id) {
    for (int i = 0; i < p->multi_count; i++) {
        if (p->multi_down[i] && p->multi_ids[i] == touch_id) {
            return i;
        }
    }
    return -1;
}

static void multi_reset(struct oxide_pointer *p) {
    p->multi_active = false;
    p->multi_fired = false;
    p->multi_count = 0;
    p->multi_active_count = 0;
}

static void multi_add(struct oxide_pointer *p, int32_t touch_id,
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
static void multi_pending_begin(struct oxide_pointer *p,
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
static void multi_promote_to_active(struct oxide_pointer *p) {
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
static void multi_pending_abandon(struct oxide_pointer *p) {
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
static void multi_pending_motion(struct oxide_pointer *p) {
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

static void multi_motion(struct oxide_pointer *p, int32_t touch_id,
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
static void multi_two_finger_tap_check(struct oxide_pointer *p,
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

static void handle_touch_down(void *userdata, void *data) {
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

static void handle_touch_motion(void *userdata, void *data) {
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

static void handle_touch_up(void *userdata, void *data) {
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

static void handle_touch_cancel(void *userdata, void *data) {
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

static void handle_touch_frame(void *userdata, void *data) {
    (void)data;
    struct oxide_pointer *p = userdata;
    wlr_seat_touch_notify_frame(p->seat);
}

static void handle_touch_device_destroy(void *userdata, void *data) {
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
