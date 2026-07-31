#define WLR_USE_UNSTABLE
#include <stdlib.h>
#include <wlr/types/wlr_keyboard.h>
#include <wlr/types/wlr_scene.h>
#include <wlr/types/wlr_seat.h>
#include <wlr/types/wlr_xdg_shell.h>
#include <wlr/util/edges.h>

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

static void handle_popup_initial_commit(struct wl_listener *listener,
        void *data) {
    (void)data;
    struct oxide_xdg_popup_configure *pending =
            wl_container_of(listener, pending, commit);
    if (!pending->popup->base->initial_commit) {
        return;
    }
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

static void handle_new_popup(void *userdata, void *data) {
    (void)userdata;
    struct wlr_xdg_popup *popup = data;
    struct oxide_xdg_popup_configure *pending =
            calloc(1, sizeof(*pending));
    pending->popup = popup;
    pending->commit.notify = handle_popup_initial_commit;
    pending->destroy.notify = handle_popup_destroy_before_configure;
    wl_signal_add(&popup->base->surface->events.commit, &pending->commit);
    wl_signal_add(&popup->events.destroy, &pending->destroy);
}

void oxide_xdg_shell_setup_popups(struct wlr_xdg_shell *shell) {
    signal_add(&shell->events.new_popup, handle_new_popup, NULL);
}

struct wlr_scene_tree *oxide_scene_add_xdg_toplevel(struct wlr_scene_tree *tree,
        struct wlr_xdg_toplevel *toplevel) {
    // A scene node that tracks this surface (and its popups) and follows its
    // map/unmap state automatically.
    return wlr_scene_xdg_surface_create(tree, toplevel->base);
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
