#define WLR_USE_UNSTABLE
#include <stdlib.h>
#include <wlr/types/wlr_keyboard.h>
#include <wlr/types/wlr_output_layout.h>
#include <wlr/types/wlr_scene.h>
#include <wlr/types/wlr_seat.h>
#include <wlr/types/wlr_xdg_shell.h>
#include <wlr/util/edges.h>
#include <wlr/util/log.h>

#include "oxide_shim_internal.h"

struct oxide_listener *oxide_xdg_shell_add_new_toplevel(
        struct wlr_xdg_shell *shell, oxide_callback callback, void *userdata) {
    return signal_add(&shell->events.new_toplevel, callback, userdata);
}

struct oxide_xdg_popup_configure {
    struct wlr_xdg_popup *popup;
    struct wl_listener commit;
    struct wl_listener destroy;
};

static void popup_configure_finish(struct oxide_xdg_popup_configure *pending) {
    wl_list_remove(&pending->commit.link);
    wl_list_remove(&pending->destroy.link);
    free(pending);
}

// Defined below, next to the popup scene wiring it belongs with.
static void popup_unconstrain(struct wlr_xdg_popup *popup);

static void handle_popup_initial_commit(struct wl_listener *listener,
        void *data) {
    (void)data;
    struct oxide_xdg_popup_configure *pending =
            wl_container_of(listener, pending, commit);
    if (!pending->popup->base->initial_commit) {
        return;
    }
    // Unconstraining schedules the configure itself; the explicit call is the
    // fallback for when there was no layout or no root toplevel to measure
    // against, since a popup that is never configured never maps at all.
    popup_unconstrain(pending->popup);
    wlr_xdg_surface_schedule_configure(pending->popup->base);
    popup_configure_finish(pending);
}

static void handle_popup_destroy_before_configure(struct wl_listener *listener,
        void *data) {
    (void)data;
    struct oxide_xdg_popup_configure *pending =
            wl_container_of(listener, pending, destroy);
    popup_configure_finish(pending);
}

// The output layout, kept so a popup can be unconstrained against the output it
// actually appears on. Set once at startup by oxide_xdg_shell_setup_popups.
static struct wlr_output_layout *popup_output_layout = NULL;

// Walks a popup chain up to the toplevel it ultimately belongs to and returns
// that toplevel's scene tree, whose layout coordinates anchor the unconstrain
// box below. A submenu's immediate parent is another popup, so this cannot just
// look at popup->parent.
static struct wlr_scene_tree *popup_root_toplevel_tree(
        struct wlr_xdg_popup *popup) {
    struct wlr_surface *surface = popup->parent;
    while (surface != NULL) {
        struct wlr_xdg_surface *xdg =
                wlr_xdg_surface_try_from_wlr_surface(surface);
        if (xdg == NULL) {
            return NULL;
        }
        if (xdg->role == WLR_XDG_SURFACE_ROLE_TOPLEVEL) {
            return xdg->data;
        }
        if (xdg->role != WLR_XDG_SURFACE_ROLE_POPUP || xdg->popup == NULL) {
            return NULL;
        }
        surface = xdg->popup->parent;
    }
    return NULL;
}

// Keeps a popup on screen by letting wlroots apply the client's own positioner
// rules (flip/slide/resize) against the output box. Without this a menu anchored
// near an edge simply overflows it and is clipped — the positioner's constraint
// adjustments are only applied when the compositor asks for them.
//
// **Only ever from the initial commit.** It ends in
// wlr_xdg_surface_schedule_configure, which asserts `surface->initialized` —
// and that is not true until the surface's first commit. Calling this from the
// new_popup handler aborts the compositor before the menu can appear.
static void popup_unconstrain(struct wlr_xdg_popup *popup) {
    if (popup_output_layout == NULL) {
        return;
    }
    // The box has to be in the *root toplevel's* surface coordinates —
    // wlr_xdg_popup_get_toplevel_coords walks the popup chain up itself — so
    // the immediate parent is not good enough for a submenu.
    struct wlr_scene_tree *root_tree = popup_root_toplevel_tree(popup);
    if (root_tree == NULL) {
        return;
    }
    int lx, ly;
    if (!wlr_scene_node_coords(&root_tree->node, &lx, &ly)) {
        return;
    }
    struct wlr_output *output =
            wlr_output_layout_output_at(popup_output_layout, lx, ly);
    if (output == NULL) {
        return;
    }
    struct wlr_box box;
    wlr_output_layout_get_box(popup_output_layout, output, &box);
    // The box must be in the popup's root toplevel surface coordinates, and
    // (lx, ly) is where that toplevel sits in the layout.
    box.x -= lx;
    box.y -= ly;
    wlr_xdg_popup_unconstrain_from_box(popup, &box);
}

static void handle_new_popup(void *userdata, void *data) {
    (void)userdata;
    struct wlr_xdg_popup *popup = data;

    // **A popup needs its own scene node.** `wlr_scene_xdg_surface_create` on
    // the toplevel covers that surface "and all of its sub-surfaces" — and a
    // popup is not a sub-surface, it is a separate xdg_surface. Without this
    // the popup maps perfectly happily, the client believes its menu is open,
    // and nothing is ever drawn. Every menu in every application is invisible.
    //
    // The parent is a toplevel for a first-level menu and another popup for a
    // submenu, so the scene tree is carried on `xdg_surface->data` — set here
    // and in oxide_scene_add_xdg_toplevel — rather than looked up by type.
    struct wlr_xdg_surface *parent =
            wlr_xdg_surface_try_from_wlr_surface(popup->parent);
    struct wlr_scene_tree *parent_tree = parent != NULL ? parent->data : NULL;
    if (parent_tree != NULL) {
        popup->base->data =
                wlr_scene_xdg_surface_create(parent_tree, popup->base);
    } else {
        // A popup parented to something with no scene tree (a layer surface
        // whose helper owns its own nodes, say) is left alone rather than
        // guessed at: better an unplaced menu than one attached to the wrong
        // part of the scene.
        wlr_log(WLR_DEBUG, "0xin: xdg popup with no parent scene tree; not adding to the scene");
    }

    struct oxide_xdg_popup_configure *pending =
            calloc(1, sizeof(*pending));
    pending->popup = popup;
    pending->commit.notify = handle_popup_initial_commit;
    pending->destroy.notify = handle_popup_destroy_before_configure;
    wl_signal_add(&popup->base->surface->events.commit, &pending->commit);
    wl_signal_add(&popup->events.destroy, &pending->destroy);
}

void oxide_xdg_shell_setup_popups(struct wlr_xdg_shell *shell,
        struct wlr_output_layout *layout) {
    popup_output_layout = layout;
    signal_add(&shell->events.new_popup, handle_new_popup, NULL);
}

struct wlr_scene_tree *oxide_scene_add_xdg_toplevel(struct wlr_scene_tree *tree,
        struct wlr_xdg_toplevel *toplevel) {
    // A scene node that tracks this surface and its *sub-surfaces*, following
    // its map/unmap state automatically. Popups are deliberately not included —
    // they are separate xdg_surfaces and get their own node in handle_new_popup,
    // which finds this tree through the `data` pointer set below.
    struct wlr_scene_tree *surface_tree =
            wlr_scene_xdg_surface_create(tree, toplevel->base);
    toplevel->base->data = surface_tree;
    return surface_tree;
}

// Commit listener, routed to Rust. Fires on every commit; Rust filters for
// the initial one (oxide_xdg_initial_commit) and answers it with a configure
// carrying the window's predicted tile size — so the client's very first
// frame is already the right size instead of its own preferred (often huge)
// one. Returned so Rust can remove it on destroy with the others.
struct oxide_listener *oxide_xdg_add_commit(struct wlr_xdg_toplevel *toplevel,
        oxide_callback callback, void *userdata) {
    return signal_add(&toplevel->base->surface->events.commit, callback, userdata);
}

// True only for the client's very first commit — the one the compositor must
// answer with a configure (or the client never maps).
bool oxide_xdg_initial_commit(struct wlr_xdg_toplevel *toplevel) {
    return toplevel->base->initial_commit;
}

// Mark the window tiled on all four edges. Without a tiled state the
// configure is "floating" semantics and clients (Firefox, GTK apps) may
// prefer their own remembered size over the one we send; with it, the
// configure size is binding. Kept in C so the WLR_EDGE_* enum stays native.
void oxide_xdg_toplevel_set_tiled_all(struct wlr_xdg_toplevel *toplevel) {
    wlr_xdg_toplevel_set_tiled(toplevel, WLR_EDGE_TOP | WLR_EDGE_BOTTOM
            | WLR_EDGE_LEFT | WLR_EDGE_RIGHT);
}

// Clear the tiled states again (edge mask 0) — the tiled -> floating toggle.
// The next configure goes back to "floating" semantics: our size is a hint
// and the client is free to use its own natural size.
void oxide_xdg_toplevel_set_tiled_none(struct wlr_xdg_toplevel *toplevel) {
    wlr_xdg_toplevel_set_tiled(toplevel, 0);
}

// The parent toplevel set via xdg_toplevel.set_parent (NULL if none). A
// non-NULL parent marks a dialog/utility window — the main float signal.
struct wlr_xdg_toplevel *oxide_xdg_toplevel_parent(
        struct wlr_xdg_toplevel *toplevel) {
    return toplevel->parent;
}

// The client's app id (e.g. "kitty", "firefox"); NULL if it never set one.
// Matched against the config's `float = <app_id>` rules.
const char *oxide_xdg_toplevel_app_id(struct wlr_xdg_toplevel *toplevel) {
    return toplevel->app_id;
}

// The client's window title; NULL if it never set one. Offered to config
// window rules, which often want to match a document or page rather than the
// application that opened it.
const char *oxide_xdg_toplevel_title(struct wlr_xdg_toplevel *toplevel) {
    return toplevel->title;
}

// True when the client committed equal, nonzero min and max sizes on both
// axes — a window that declares it cannot be resized, so tiling it would
// only stretch or letterbox it.
bool oxide_xdg_toplevel_fixed_size(struct wlr_xdg_toplevel *toplevel) {
    struct wlr_xdg_toplevel_state *s = &toplevel->current;
    return s->min_width > 0 && s->min_width == s->max_width
            && s->min_height > 0 && s->min_height == s->max_height;
}

// The window's current effective geometry (the part of the surface that is
// actually the window, excluding client-side shadows), for centering a
// floating window at its natural size on map.
void oxide_xdg_toplevel_geometry(struct wlr_xdg_toplevel *toplevel,
        int *width, int *height) {
    *width = toplevel->base->geometry.width;
    *height = toplevel->base->geometry.height;
}

struct oxide_listener *oxide_xdg_add_map(struct wlr_xdg_toplevel *toplevel,
        oxide_callback callback, void *userdata) {
    return signal_add(&toplevel->base->surface->events.map, callback, userdata);
}

struct oxide_listener *oxide_xdg_add_unmap(struct wlr_xdg_toplevel *toplevel,
        oxide_callback callback, void *userdata) {
    return signal_add(&toplevel->base->surface->events.unmap, callback, userdata);
}

struct oxide_listener *oxide_xdg_add_destroy(struct wlr_xdg_toplevel *toplevel,
        oxide_callback callback, void *userdata) {
    return signal_add(&toplevel->events.destroy, callback, userdata);
}

void oxide_scene_tree_set_position(struct wlr_scene_tree *tree, int x, int y) {
    wlr_scene_node_set_position(&tree->node, x, y);
}

// Crop a window's scene subtree to width x height (surface-local
// coordinates), so a client that ignores its requested tile size can't
// visually spill into a neighboring tile. A width/height of 0 disables
// clipping (used for floating windows, which size themselves freely).
void oxide_scene_tree_set_clip(struct wlr_scene_tree *tree, int width, int height) {
    struct wlr_box clip = {0, 0, width, height};
    wlr_scene_subsurface_tree_set_clip(&tree->node, &clip);
}

// Destroy a window's scene tree (used to rebuild it from scratch on VT resume,
// where the original node stops presenting its surface after the outputs are
// torn down and recreated).
void oxide_scene_tree_destroy(struct wlr_scene_tree *tree) {
    wlr_scene_node_destroy(&tree->node);
}

void oxide_scene_tree_set_enabled(struct wlr_scene_tree *tree, bool enabled) {
    wlr_scene_node_set_enabled(&tree->node, enabled);
}

static void set_buffer_opacity(struct wlr_scene_buffer *buffer,
        int sx, int sy, void *userdata) {
    (void)sx;
    (void)sy;
    float opacity = *(float *)userdata;
    wlr_scene_buffer_set_opacity(buffer, opacity);
}

void oxide_scene_tree_set_opacity(struct wlr_scene_tree *tree, float opacity) {
    wlr_scene_node_for_each_buffer(&tree->node, set_buffer_opacity, &opacity);
}

// The toplevel's root wlr_surface — what scene hit-testing resolves clicks to
// (via wlr_surface_get_root_surface), so Rust can match a clicked surface
// back to the Toplevel it tracks.
struct wlr_surface *oxide_xdg_toplevel_surface(struct wlr_xdg_toplevel *toplevel) {
    return toplevel->base->surface;
}

// Fires when the client asks to enter OR leave fullscreen (F11 in a browser,
// mpv --fs). The protocol requires the compositor to answer every state
// request with a configure — Rust does that via wlr_xdg_toplevel_set_fullscreen.
struct oxide_listener *oxide_xdg_add_request_fullscreen(
        struct wlr_xdg_toplevel *toplevel, oxide_callback callback,
        void *userdata) {
    return signal_add(&toplevel->events.request_fullscreen, callback, userdata);
}

// What the client currently wants (checked on the request signal and on map).
bool oxide_xdg_toplevel_requested_fullscreen(struct wlr_xdg_toplevel *toplevel) {
    return toplevel->requested.fullscreen;
}

// Move a window's scene tree to another layer tree (normal <-> fullscreen).
void oxide_scene_tree_reparent(struct wlr_scene_tree *tree,
        struct wlr_scene_tree *new_parent) {
    wlr_scene_node_reparent(&tree->node, new_parent);
}

void oxide_focus_toplevel(struct wlr_seat *seat,
        struct wlr_xdg_toplevel *toplevel) {
    struct wlr_surface *surface = toplevel->base->surface;
    struct wlr_keyboard *kb = wlr_seat_get_keyboard(seat);
    if (kb != NULL) {
        wlr_seat_keyboard_notify_enter(seat, surface, kb->keycodes,
                kb->num_keycodes, &kb->modifiers);
    } else {
        wlr_seat_keyboard_notify_enter(seat, surface, NULL, 0, NULL);
    }
}
