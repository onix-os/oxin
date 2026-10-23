#define WLR_USE_UNSTABLE
#include <stdlib.h>
#include <wlr/types/wlr_layer_shell_v1.h>
#include <wlr/types/wlr_scene.h>
#include <wlr/types/wlr_xdg_shell.h>

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

// --- layer-surface popups (a bar's tray menu, a panel's dropdown) -----------
//
// The scene helper above covers the layer surface and its sub-surfaces only. A
// popup is a separate xdg_surface, so without this it maps, the client thinks
// its menu is open, and nothing is drawn. It cannot be placed from xdg-shell's
// new_popup either: a layer surface's popup has no parent at that point and is
// only given one by get_popup, which then raises the layer surface's own
// new_popup — handled here.
//
// Per layer surface, reachable from `wlr_layer_surface_v1.data` (which
// wlroots leaves to the compositor) for as long as the surface lives.
struct oxide_layer_popups {
    // Held directly rather than read through scene_ls: the scene helper frees
    // itself on the same destroy signal, and its listener runs first.
    struct wlr_layer_surface_v1 *ls;
    struct wlr_scene_layer_surface_v1 *scene_ls;
    // NULL: popups go straight under the layer surface's own tree and move with
    // it for free. Otherwise each popup gets a holder node in this tree, kept at
    // the layer surface's position — so a menu from a bottom-layer bar paints
    // over the windows it drops across instead of underneath them.
    struct wlr_scene_tree *popup_tree;
    struct wl_listener new_popup;
    struct wl_listener destroy;
};

// A lifted popup's holder. Owned by the popup rather than the layer surface,
// since wlroots does not promise which of the two is destroyed first.
struct oxide_layer_popup_holder {
    struct wlr_scene_tree *holder;
    struct wl_listener destroy;
};

// Moves every lifted popup's holder to where the layer surface now is. A
// first-level popup's scene tree is a child of its holder (its submenus nest
// inside that tree and follow it), so the holder is that tree's parent.
static void layer_popups_sync(struct oxide_layer_popups *lp) {
    if (lp->popup_tree == NULL) {
        return;
    }
    int lx, ly;
    if (!wlr_scene_node_coords(&lp->scene_ls->tree->node, &lx, &ly)) {
        return;
    }
    struct wlr_xdg_popup *popup;
    wl_list_for_each(popup, &lp->ls->popups, link) {
        struct wlr_scene_tree *tree = popup->base->data;
        if (tree != NULL && tree->node.parent != NULL) {
            wlr_scene_node_set_position(&tree->node.parent->node, lx, ly);
        }
    }
}

static void handle_layer_popup_holder_destroy(struct wl_listener *listener,
        void *data) {
    (void)data;
    struct oxide_layer_popup_holder *h = wl_container_of(listener, h, destroy);
    wl_list_remove(&h->destroy.link);
    // Takes the popup's scene tree with it, if wlroots hasn't already.
    wlr_scene_node_destroy(&h->holder->node);
    free(h);
}

static void handle_layer_new_popup(struct wl_listener *listener, void *data) {
    struct oxide_layer_popups *lp = wl_container_of(listener, lp, new_popup);
    struct wlr_xdg_popup *popup = data;

    struct wlr_scene_tree *parent = lp->scene_ls->tree;
    if (lp->popup_tree != NULL) {
        struct oxide_layer_popup_holder *h = calloc(1, sizeof(*h));
        h->holder = wlr_scene_tree_create(lp->popup_tree);
        h->destroy.notify = handle_layer_popup_holder_destroy;
        wl_signal_add(&popup->events.destroy, &h->destroy);
        parent = h->holder;
    }
    // Carried on `data` like every other popup's tree, so its submenus find
    // their parent through the same path in shim/xdg_shell.c.
    popup->base->data = wlr_scene_xdg_surface_create(parent, popup->base);
    layer_popups_sync(lp);
}

static void handle_layer_popups_destroy(struct wl_listener *listener,
        void *data) {
    (void)data;
    struct oxide_layer_popups *lp = wl_container_of(listener, lp, destroy);
    lp->ls->data = NULL;
    wl_list_remove(&lp->new_popup.link);
    wl_list_remove(&lp->destroy.link);
    free(lp);
}

void oxide_layer_surface_setup_popups(struct wlr_scene_layer_surface_v1 *scene_ls,
        struct wlr_scene_tree *popup_tree) {
    struct wlr_layer_surface_v1 *ls = scene_ls->layer_surface;
    struct oxide_layer_popups *lp = calloc(1, sizeof(*lp));
    lp->ls = ls;
    lp->scene_ls = scene_ls;
    lp->popup_tree = popup_tree;
    lp->new_popup.notify = handle_layer_new_popup;
    lp->destroy.notify = handle_layer_popups_destroy;
    wl_signal_add(&ls->events.new_popup, &lp->new_popup);
    wl_signal_add(&ls->events.destroy, &lp->destroy);
    ls->data = lp;
}

bool oxide_layer_popup_anchor(struct wlr_surface *surface, int *lx, int *ly,
        struct wlr_output **output) {
    struct wlr_layer_surface_v1 *ls =
            wlr_layer_surface_v1_try_from_wlr_surface(surface);
    if (ls == NULL || ls->data == NULL || ls->output == NULL) {
        return false;
    }
    struct oxide_layer_popups *lp = ls->data;
    // A layer surface has no window geometry: its tree's origin, which
    // wlr_scene_layer_surface_v1_configure places, is the surface origin.
    if (!wlr_scene_node_coords(&lp->scene_ls->tree->node, lx, ly)) {
        return false;
    }
    *output = ls->output;
    return true;
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
    // The surface may just have moved; lifted popups don't follow on their own.
    if (scene_ls->layer_surface->data != NULL) {
        layer_popups_sync(scene_ls->layer_surface->data);
    }
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
