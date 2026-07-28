#define WLR_USE_UNSTABLE
#include <time.h>
#include <wlr/backend.h>
#include <wlr/types/wlr_cursor.h>
#include <wlr/types/wlr_output.h>
#include <wlr/types/wlr_output_layout.h>
#include <wlr/types/wlr_scene.h>

#include "oxide_shim_internal.h"

struct oxide_listener *oxide_backend_add_new_output(
        struct wlr_backend *backend, oxide_callback callback, void *userdata) {
    return signal_add(&backend->events.new_output, callback, userdata);
}

// Output destroy fires when a monitor is removed — including when logind
// disables the seat on a VT switch (the DRM backend tears the output down).
// Rust uses this to remove its frame listener (else wlr_output_finish asserts)
// and drop the output from its list. `data` is the wlr_output.
struct oxide_listener *oxide_output_add_destroy(
        struct wlr_output *output, oxide_callback callback, void *userdata) {
    return signal_add(&output->events.destroy, callback, userdata);
}

struct oxide_listener *oxide_output_add_frame(
        struct wlr_output *output, oxide_callback callback, void *userdata) {
    return signal_add(&output->events.frame, callback, userdata);
}

void oxide_output_enable(struct wlr_output *output, float scale) {
    struct wlr_output_state state;
    wlr_output_state_init(&state);
    wlr_output_state_set_enabled(&state, true);

    // Windowed backends (nested Wayland/X11) expose no modes; only real
    // displays do. Pick the preferred one when present.
    struct wlr_output_mode *mode = wlr_output_preferred_mode(output);
    if (mode != NULL) {
        wlr_output_state_set_mode(&state, mode);
    }

    wlr_output_state_set_scale(&state, scale);

    wlr_output_commit_state(output, &state);
    wlr_output_state_finish(&state);
}

// The connector name (e.g. "eDP-1", "HDMI-A-1") — read once by Rust and
// converted to an owned String immediately, so there's no pointer lifetime to
// worry about on the Rust side.
const char *oxide_output_name(struct wlr_output *output) {
    return output->name;
}

// Create an ordered child tree directly under the scene root. Creation order
// is paint order (later = on top) — this is how we get correct layer-shell
// z-ordering without touching wlroots' internals.
struct wlr_scene_tree *oxide_scene_add_layer_tree(struct wlr_scene *scene) {
    return wlr_scene_tree_create(&scene->tree);
}

struct wlr_scene_rect *oxide_scene_add_output_background(struct wlr_scene_tree *tree,
        struct wlr_output *output, int x, int y, float r, float g, float b) {
    const float color[4] = {r, g, b, 1.0f};
    struct wlr_scene_rect *rect =
            wlr_scene_rect_create(tree, output->width, output->height, color);
    // Scene nodes share one coordinate space; place this output's background at
    // the output's position in the layout so multiple monitors don't overlap.
    wlr_scene_node_set_position(&rect->node, x, y);
    return rect;
}

struct wlr_scene_rect *oxide_scene_add_rect(struct wlr_scene_tree *tree,
        int x, int y, int width, int height, float r, float g, float b,
        float a) {
    const float color[4] = {r, g, b, a};
    struct wlr_scene_rect *rect = wlr_scene_rect_create(tree, width, height, color);
    wlr_scene_node_set_position(&rect->node, x, y);
    return rect;
}

// Remove a background rectangle (when its output is destroyed).
void oxide_scene_rect_destroy(struct wlr_scene_rect *rect) {
    wlr_scene_node_destroy(&rect->node);
}

// Enable/disable a background rect. Toggling it off then on damages the whole
// output region (the rect spans the output), which forces a full re-present —
// used on VT resume so idle windows, which produce no damage of their own, get
// flipped back to the screen instead of staying black.
void oxide_scene_rect_set_enabled(struct wlr_scene_rect *rect, bool enabled) {
    wlr_scene_node_set_enabled(&rect->node, enabled);
}

void oxide_scene_rect_set_position(struct wlr_scene_rect *rect, int x, int y) {
    wlr_scene_node_set_position(&rect->node, x, y);
}

// Which output is under the cursor right now (NULL if none). Lets Rust target
// the monitor the mouse is on for new windows / workspace switches.
struct wlr_output *oxide_output_at_cursor(struct wlr_cursor *cursor,
        struct wlr_output_layout *layout) {
    return wlr_output_layout_output_at(layout, cursor->x, cursor->y);
}

// Read an output's box (position + size) in layout coordinates, so Rust can tile
// windows within the correct monitor. (Touches wlr_box internals.)
void oxide_output_layout_get_box(struct wlr_output_layout *layout,
        struct wlr_output *output, int *x, int *y, int *width, int *height) {
    struct wlr_box box;
    wlr_output_layout_get_box(layout, output, &box);
    *x = box.x;
    *y = box.y;
    *width = box.width;
    *height = box.height;
}

// Ask the output to emit a `frame` event when it's ready to draw. Used to kick
// the first paint of a (re)created output without rendering before it's ready —
// the frame handler then does a full repaint, so resumed windows reappear.
void oxide_output_schedule_frame(struct wlr_output *output) {
    wlr_output_schedule_frame(output);
}

void oxide_scene_output_render(struct wlr_scene_output *scene_output) {
    // The scene does the damage-tracked render pass internally, then we tell
    // clients their frame was shown so they can produce the next one.
    wlr_scene_output_commit(scene_output, NULL);
    struct timespec now;
    clock_gettime(CLOCK_MONOTONIC, &now);
    wlr_scene_output_send_frame_done(scene_output, &now);
}

void oxide_output_get_size(struct wlr_output *output, int *width, int *height) {
    *width = output->width;
    *height = output->height;
}
