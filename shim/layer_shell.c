#define WLR_USE_UNSTABLE
#include <wlr/types/wlr_layer_shell_v1.h>
#include <wlr/types/wlr_scene.h>

#include "oxide_shim_internal.h"

// --- layer-shell (bars, panels, wallpaper) ----------------------------------
//
// The global itself is created directly from Rust via the bindgen binding for
// wlr_layer_shell_v1_create (same pattern as wlr_xdg_shell_create) — no shim
// wrapper needed for a plain creation call.

struct oxide_listener *oxide_layer_shell_add_new_surface(
        struct wlr_layer_shell_v1 *shell, oxide_callback callback, void *userdata) {
    return signal_add(&shell->events.new_surface, callback, userdata);
}

// The output may be NULL if the client didn't request a specific one; Rust
// must assign one before returning from the new_surface handler.
struct wlr_output *oxide_layer_surface_output(struct wlr_layer_surface_v1 *ls) {
    return ls->output;
}

void oxide_layer_surface_set_output(struct wlr_layer_surface_v1 *ls,
        struct wlr_output *output) {
    ls->output = output;
}

// Requested z-layer (0=background..3=overlay, zwlr_layer_shell_v1_layer). Set
// directly by the get_layer_surface request, so it's already valid when
// new_surface fires.
uint32_t oxide_layer_surface_layer(struct wlr_layer_surface_v1 *ls) {
    return ls->pending.layer;
}

bool oxide_layer_surface_initialized(struct wlr_layer_surface_v1 *ls) {
    return ls->initialized;
}

// Whether this surface's most recent commit actually needs a rearrange:
// either its very first commit (which must get its first configure to map
// at all), or one that set a layout-relevant property (size, anchor,
// exclusive zone, margin, layer, or exclusive edge). A commit that's just a
// content redraw (a new buffer, nothing else) sets none of these bits, and
// re-arranging for it anyway sends every layer surface on the output a
// fresh, entirely redundant configure — a burst of these racing a client's
// ack against an already-superseded serial produces a fatal "wrong
// configure serial" protocol error. Skipping the rearrange for redraw-only
// commits avoids the storm at its source.
bool oxide_layer_surface_commit_needs_rearrange(struct wlr_layer_surface_v1 *ls) {
    const uint32_t layout_relevant = WLR_LAYER_SURFACE_V1_STATE_DESIRED_SIZE
        | WLR_LAYER_SURFACE_V1_STATE_ANCHOR
        | WLR_LAYER_SURFACE_V1_STATE_EXCLUSIVE_ZONE
        | WLR_LAYER_SURFACE_V1_STATE_MARGIN
        | WLR_LAYER_SURFACE_V1_STATE_LAYER
        | WLR_LAYER_SURFACE_V1_STATE_EXCLUSIVE_EDGE;
    return ls->initial_commit || (ls->current.committed & layout_relevant) != 0;
}

struct wlr_scene_layer_surface_v1 *oxide_scene_layer_surface_create(
        struct wlr_scene_tree *tree, struct wlr_layer_surface_v1 *ls) {
    return wlr_scene_layer_surface_v1_create(tree, ls);
}

// Position/size `scene_ls` per its anchors+margins within the output box
// (fx,fy,fw,fh), and shrink the usable box (ux,uy,uw,uh, in/out) by its
// exclusive zone. wlr_scene_layer_surface_v1_configure also sends the
// layer_surface.configure event that lets the client map.
void oxide_scene_layer_surface_configure(struct wlr_scene_layer_surface_v1 *scene_ls,
        int fx, int fy, int fw, int fh, int *ux, int *uy, int *uw, int *uh) {
    struct wlr_box full = {.x = fx, .y = fy, .width = fw, .height = fh};
    struct wlr_box usable = {.x = *ux, .y = *uy, .width = *uw, .height = *uh};
    wlr_scene_layer_surface_v1_configure(scene_ls, &full, &usable);
    *ux = usable.x;
    *uy = usable.y;
    *uw = usable.width;
    *uh = usable.height;
}

struct oxide_listener *oxide_layer_surface_add_commit(struct wlr_layer_surface_v1 *ls,
        oxide_callback callback, void *userdata) {
    return signal_add(&ls->surface->events.commit, callback, userdata);
}

struct oxide_listener *oxide_layer_surface_add_map(struct wlr_layer_surface_v1 *ls,
        oxide_callback callback, void *userdata) {
    return signal_add(&ls->surface->events.map, callback, userdata);
}

struct oxide_listener *oxide_layer_surface_add_unmap(struct wlr_layer_surface_v1 *ls,
        oxide_callback callback, void *userdata) {
    return signal_add(&ls->surface->events.unmap, callback, userdata);
}

struct oxide_listener *oxide_layer_surface_add_destroy(struct wlr_layer_surface_v1 *ls,
        oxide_callback callback, void *userdata) {
    return signal_add(&ls->events.destroy, callback, userdata);
}
