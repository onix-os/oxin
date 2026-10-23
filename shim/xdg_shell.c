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

// Per-popup state for the popup's whole life: the one-shot initial configure,
// and a fresh unconstrain whenever the client repositions it (xdg_popup v3
// `reposition`, which a menu uses to move without being torn down).
struct oxide_xdg_popup {
    struct wlr_xdg_popup *popup;
    struct wl_listener commit;     // removed once the initial commit is handled
    struct wl_listener reposition;
    struct wl_listener destroy;
};

// Defined below, next to the popup scene wiring it belongs with.
static void popup_unconstrain(struct wlr_xdg_popup *popup);

static void handle_popup_initial_commit(struct wl_listener *listener,
        void *data) {
    (void)data;
    struct oxide_xdg_popup *state = wl_container_of(listener, state, commit);
    if (!state->popup->base->initial_commit) {
        return;
    }
    // Unconstraining schedules the configure itself; the explicit call is the
    // fallback for when there was no layout or no root surface to measure
    // against, since a popup that is never configured never maps at all.
    popup_unconstrain(state->popup);
    wlr_xdg_surface_schedule_configure(state->popup->base);
    // Re-initialised so the destroy handler can remove it unconditionally.
    wl_list_remove(&state->commit.link);
    wl_list_init(&state->commit.link);
}

static void handle_popup_reposition(struct wl_listener *listener, void *data) {
    (void)data;
    struct oxide_xdg_popup *state =
            wl_container_of(listener, state, reposition);
    // wlroots has already recomputed the geometry from the new positioner;
    // its constraint adjustments still only happen when we ask for them.
    if (state->popup->base->initialized) {
        popup_unconstrain(state->popup);
    }
}

static void handle_popup_destroy(struct wl_listener *listener, void *data) {
    (void)data;
    struct oxide_xdg_popup *state = wl_container_of(listener, state, destroy);
    wl_list_remove(&state->commit.link);
    wl_list_remove(&state->reposition.link);
    wl_list_remove(&state->destroy.link);
    free(state);
}

// The output layout, kept so a popup can be unconstrained against the output it
// actually appears on. Set once at startup by oxide_xdg_shell_setup_popups.
static struct wlr_output_layout *popup_output_layout = NULL;

// Where a popup chain's root surface sits: the layout coordinates of its
// surface-local origin, and the output the popup should be kept on. The root is
// an xdg toplevel for an application's menu and a layer surface for a bar's; a
// submenu's immediate parent is another popup, so the chain is walked to the top.
static bool popup_root_anchor(struct wlr_xdg_popup *popup, int *lx, int *ly,
        struct wlr_output **output) {
    struct wlr_surface *surface = popup->parent;
    while (surface != NULL) {
        struct wlr_xdg_surface *xdg =
                wlr_xdg_surface_try_from_wlr_surface(surface);
        if (xdg == NULL) {
            // Not an xdg surface at all: a layer surface, or nothing we place.
            return oxide_layer_popup_anchor(surface, lx, ly, output);
        }
        if (xdg->role == WLR_XDG_SURFACE_ROLE_TOPLEVEL) {
            struct wlr_scene_tree *tree = xdg->data;
            if (tree == NULL || popup_output_layout == NULL
                    || !wlr_scene_node_coords(&tree->node, lx, ly)) {
                return false;
            }
            *output = wlr_output_layout_output_at(popup_output_layout, *lx, *ly);
            // The toplevel's tree sits at its *window geometry* origin
            // (wlr_scene_xdg_surface_create offsets the surface inside it), so
            // the surface origin is that minus the geometry offset — nonzero
            // for a client that draws its own shadow.
            *lx -= xdg->geometry.x;
            *ly -= xdg->geometry.y;
            return *output != NULL;
        }
        if (xdg->role != WLR_XDG_SURFACE_ROLE_POPUP || xdg->popup == NULL) {
            return false;
        }
        surface = xdg->popup->parent;
    }
    return false;
}

// Keeps a popup on screen by letting wlroots apply the client's own positioner
// rules (flip/slide/resize) against the output box. Without this a menu anchored
// near an edge simply overflows it and is clipped — the positioner's constraint
// adjustments are only applied when the compositor asks for them.
//
// **Never from the new_popup handler.** It ends in
// wlr_xdg_surface_schedule_configure, which asserts `surface->initialized` —
// and that is not true until the surface's first commit. Calling this from the
// new_popup handler aborts the compositor before the menu can appear.
static void popup_unconstrain(struct wlr_xdg_popup *popup) {
    int lx, ly;
    struct wlr_output *output;
    if (!popup_root_anchor(popup, &lx, &ly, &output)) {
        return;
    }
    struct wlr_box box;
    wlr_output_layout_get_box(popup_output_layout, output, &box);
    // wlr_xdg_popup_unconstrain_from_box wants the box in the root surface's
    // local coordinates (it walks the popup chain up itself), and (lx, ly) is
    // where that surface's origin sits in the layout.
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
    //
    // **The parent can be NULL.** A layer surface's popup is created with no
    // parent and only gets one from zwlr_layer_surface_v1.get_popup, after this
    // signal; wlroots still emits it, and wlr_xdg_surface_try_from_wlr_surface
    // does not accept NULL. That popup's node is made in shim/layer_shell.c,
    // from the layer surface's own new_popup, once the parent is known.
    struct wlr_xdg_surface *parent = popup->parent != NULL
            ? wlr_xdg_surface_try_from_wlr_surface(popup->parent)
            : NULL;
    struct wlr_scene_tree *parent_tree = parent != NULL ? parent->data : NULL;
    if (parent_tree != NULL) {
        popup->base->data =
                wlr_scene_xdg_surface_create(parent_tree, popup->base);
    } else if (popup->parent != NULL) {
        // Parented to something with no scene tree of ours: left alone rather
        // than guessed at — better an unplaced menu than one attached to the
        // wrong part of the scene.
        wlr_log(WLR_DEBUG, "0xin: xdg popup with no parent scene tree; not adding to the scene");
    }

    struct oxide_xdg_popup *state = calloc(1, sizeof(*state));
    state->popup = popup;
    state->commit.notify = handle_popup_initial_commit;
    state->reposition.notify = handle_popup_reposition;
    state->destroy.notify = handle_popup_destroy;
    wl_signal_add(&popup->base->surface->events.commit, &state->commit);
    wl_signal_add(&popup->events.reposition, &state->reposition);
    wl_signal_add(&popup->events.destroy, &state->destroy);
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
