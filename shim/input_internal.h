#ifndef OXIN_INPUT_INTERNAL_H
#define OXIN_INPUT_INTERNAL_H

// Shared across the input*.c / pointer.c / touch*.c files that together
// implement seat, cursor, and touch-gesture handling — split out of one
// large input.c into these cooperating translation units. Never seen by
// bindgen (wrapper.h doesn't include any shim header — see build.rs).

#include "oxide_shim_internal.h"

// Forward declarations, so this header doesn't need to pull in every
// wlroots header its struct/prototype pointer parameters mention — each
// .c file already includes whichever of these it dereferences.
struct wlr_surface;
struct wlr_seat_client;
struct wlr_touch_down_event;

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

// --- pointer.c ---------------------------------------------------------

// Find the surface at layout coordinates (and its surface-local coords), via
// the scene graph. Returns NULL over the bare background. Also used by
// touch code (release_hold, handle_touch_down, multi_two_finger_tap_check).
struct wlr_surface *surface_at_coords(struct oxide_pointer *p,
        double lx, double ly, double *sx, double *sy);

// Keep both Wayland keyboard focus and Rust's focused-window bookkeeping in
// sync for pointer clicks and touch taps. Also used by touch code
// (release_hold, handle_touch_down).
void focus_surface(struct oxide_pointer *p, struct wlr_surface *surface);

// The cursor/touch signal handlers themselves — non-static so
// oxide_cursor_setup (pointer.c) can wire them up via signal_add.
void handle_cursor_motion(void *userdata, void *data);
void handle_cursor_motion_absolute(void *userdata, void *data);
void handle_cursor_button(void *userdata, void *data);
void handle_cursor_axis(void *userdata, void *data);
void handle_cursor_frame(void *userdata, void *data);

// --- keyboard_seat.c -----------------------------------------------------

// Attaches key/modifier/destroy listeners for one keyboard device. Called
// both from handle_new_virtual_keyboard (keyboard_seat.c itself) and from
// oxide_handle_new_input (pointer.c) for real hardware keyboards.
void seat_add_keyboard(struct wlr_seat *seat,
        struct wlr_input_device *device, oxide_key_callback key_callback,
        void *key_userdata);

// --- touch_gestures.c ------------------------------------------------------

// Find a tracked touch point by its wlr touch id, or NULL.
struct oxide_touch_point *touch_point_find(
        struct oxide_pointer *p, int32_t touch_id);

bool keyboard_gesture_hit(struct oxide_pointer *p, double lx, double ly);

// -1 for the left edge zone, +1 for the right, 0 for neither.
int workspace_gesture_edge(struct oxide_pointer *p, double lx, double ly);

bool top_gesture_hit(struct oxide_pointer *p, double lx, double ly);

// True for a touch-down landing on the visible keyboard while the
// swipe-down-to-hide gesture is configured — see gesture_kind 5's doc
// comment on oxide_touch_point above.
bool keyboard_hide_candidate(struct oxide_pointer *p, double lx, double ly);

// Forwards the touch-down a kind-5 (or a demoted pending-pinch) point held
// back, then catches the client up to the current position if it already
// moved. Transitions the point to an ordinary gesture_kind == 0 client
// touch. See its definition in touch_gestures.c for the full contract.
void release_hold(struct oxide_pointer *p, struct oxide_touch_point *point);

// Fires once OXIDE_KEYBOARD_HOLD_MS elapses without the swipe-down gesture
// committing or the touch lifting. Non-static: armed as an event-loop timer
// callback from handle_touch_down (touch_events.c).
void handle_keyboard_hold_timeout(void *userdata, void *data);

// Cancels every touch point belonging to `client` (a Wayland touch cancel
// ends the whole sequence for that client, not just one finger).
void touch_cancel_client(struct oxide_pointer *p,
        struct wlr_seat_client *client);

// --- touch_multi.c -----------------------------------------------------

bool multi_gestures_enabled(struct oxide_pointer *p);
int multi_index(struct oxide_pointer *p, int32_t touch_id);
void multi_reset(struct oxide_pointer *p);
void multi_add(struct oxide_pointer *p, int32_t touch_id, double lx, double ly);

// Starts the undecided two-finger window — see its definition in
// touch_multi.c for the full contract.
void multi_pending_begin(struct oxide_pointer *p,
        struct oxide_touch_point *first, struct wlr_touch_down_event *e,
        double second_lx, double second_ly);

// Commits a pending pair to a compositor gesture (cancels the first
// finger's client sequence, drops the held second, hands off to
// multi_active tracking). Also used directly by handle_touch_down when a
// third finger arrives before the pair resolves.
void multi_promote_to_active(struct oxide_pointer *p);

// Ends the pending window without ever deciding on a gesture, without
// disturbing whichever finger is still a live client touch.
void multi_pending_abandon(struct oxide_pointer *p);

// Re-checks a pending pair's shape after motion: promotes to a swipe,
// releases to a pinch, or keeps waiting. See touch_multi.c.
void multi_pending_motion(struct oxide_pointer *p);

// Classifies an already-promoted (multi_active) two/three-finger touch's
// motion as a directional swipe.
void multi_motion(struct oxide_pointer *p, int32_t touch_id, double lx, double ly);

// Checked when a tracked two-finger touch fully releases, to recognize a
// two-finger double-tap retroactively (no in-progress signal to promote on,
// unlike the motion-based gestures above).
void multi_two_finger_tap_check(struct oxide_pointer *p, uint32_t time_msec);

// --- touch_events.c ------------------------------------------------------

// The six wlroots touch signal handlers — non-static so oxide_cursor_setup
// (pointer.c) and pointer_add_touch (pointer.c) can wire them up.
void handle_touch_down(void *userdata, void *data);
void handle_touch_motion(void *userdata, void *data);
void handle_touch_up(void *userdata, void *data);
void handle_touch_cancel(void *userdata, void *data);
void handle_touch_frame(void *userdata, void *data);
void handle_touch_device_destroy(void *userdata, void *data);

#endif // OXIN_INPUT_INTERNAL_H
