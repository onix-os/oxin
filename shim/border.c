#define WLR_USE_UNSTABLE
#include <math.h>
#include <stdlib.h>
#include <wlr/types/wlr_buffer.h>
#include <wlr/types/wlr_output.h>
#include <wlr/types/wlr_scene.h>
#include <wlr/types/wlr_xdg_shell.h>

#include "oxide_shim_internal.h"

// --- window borders -----------------------------------------------------------
//
// A frame of `width` logical pixels just outside a toplevel's window box
// (oxide_xdg_toplevel_window_box), in one solid color.
//
// It is a scene tree of its own at the *bottom* of the toplevel's scene tree:
// moving, hiding or restacking the window carries the border along with no
// work of ours, and the window and its popups paint over it. Inside, four
// wlr_scene_rects run along the straight edges, and the four corners are one
// small CPU-drawn buffer shown mirrored into each corner. With rounded window
// corners that buffer is a quarter ring, concentric with the window's arc
// (outer radius = corner radius + width), so the band keeps one width all the
// way round; with square ones it is a solid square. No shader is involved, so
// unlike the corner mask this works on every renderer.
//
// In the toplevel tree's coordinates (window box (0,0)..(w,h), b = width,
// r = corner radius, R = r + b):
//
//   corners (R x R): TL (-b,-b)   TR (w-r,-b)   BL (-b,h-r)   BR (w-r,h-r)
//   edges:           top (r,-b, w-2r x b)      bottom (r,h, w-2r x b)
//                    left (-b,r, b x h-2r)     right (w,r, b x h-2r)
//
// The ring reaches ~1.5px inside the window's arc, under the window's own
// antialiased edge, so the two can't leave a hairline of background between
// them.
//
// wlroots destroys the tree along with the toplevel's tree (window destroy, or
// the scene rebuild after a VT switch, src/output.rs). A destroy listener
// forgets the nodes when that happens, and the next update builds new ones.

enum { EDGE_TOP, EDGE_BOTTOM, EDGE_LEFT, EDGE_RIGHT };
enum { CORNER_TL, CORNER_TR, CORNER_BL, CORNER_BR };

struct oxide_border {
    struct wlr_scene_tree *parent; // the toplevel tree it was built under
    struct wlr_scene_tree *tree;   // NULL until built, and once destroyed
    struct wlr_scene_rect *edges[4];
    struct wlr_scene_buffer *corners[4];
    struct wl_listener tree_destroy;
    // What the corner buffer shows; drawn_px == 0 means nothing yet.
    int drawn_px, drawn_width, drawn_radius;
    float drawn_color[3];
    // The window's output scale, as last seen.
    float scale;
};

static void border_forget_nodes(struct oxide_border *border) {
    border->parent = NULL;
    border->tree = NULL;
    for (int i = 0; i < 4; i++) {
        border->edges[i] = NULL;
        border->corners[i] = NULL;
    }
    border->drawn_px = 0;
}

static void handle_tree_destroy(struct wl_listener *listener, void *data) {
    (void)data;
    struct oxide_border *border = wl_container_of(listener, border, tree_destroy);
    wl_list_remove(&border->tree_destroy.link);
    border_forget_nodes(border);
}

static bool border_build(struct oxide_border *border,
        struct wlr_scene_tree *parent) {
    struct wlr_scene_tree *tree = wlr_scene_tree_create(parent);
    if (tree == NULL) {
        return false;
    }
    const float none[4] = {0};
    for (int i = 0; i < 4; i++) {
        border->edges[i] = wlr_scene_rect_create(tree, 0, 0, none);
        border->corners[i] = wlr_scene_buffer_create(tree, NULL);
        if (border->edges[i] == NULL || border->corners[i] == NULL) {
            wlr_scene_node_destroy(&tree->node);
            border_forget_nodes(border);
            return false;
        }
    }
    // One buffer, drawn for the top-left corner, mirrored into the others.
    // (A flip is its own inverse, so it doesn't matter which way round
    // wlroots applies these.)
    wlr_scene_buffer_set_transform(border->corners[CORNER_TR],
            WL_OUTPUT_TRANSFORM_FLIPPED);
    wlr_scene_buffer_set_transform(border->corners[CORNER_BL],
            WL_OUTPUT_TRANSFORM_FLIPPED_180);
    wlr_scene_buffer_set_transform(border->corners[CORNER_BR],
            WL_OUTPUT_TRANSFORM_180);
    wlr_scene_node_lower_to_bottom(&tree->node);

    border->parent = parent;
    border->tree = tree;
    border->drawn_px = 0;
    border->tree_destroy.notify = handle_tree_destroy;
    wl_signal_add(&tree->node.events.destroy, &border->tree_destroy);
    return true;
}

struct root_buffer_search {
    struct wlr_surface *surface;
    struct wlr_scene_buffer *found;
};

static void find_root_buffer(struct wlr_scene_buffer *buffer, int sx, int sy,
        void *data) {
    (void)sx;
    (void)sy;
    struct root_buffer_search *search = data;
    struct wlr_scene_surface *scene_surface =
            wlr_scene_surface_try_from_buffer(buffer);
    if (search->found == NULL && scene_surface != NULL
            && scene_surface->surface == search->surface) {
        search->found = buffer;
    }
}

// The scale of the output the window is mostly on — the one wlroots tells the
// client to render for — so the corners come out as sharp as the window.
// Unchanged when that isn't known (nothing committed, or not on an output).
static void update_scale(struct oxide_border *border,
        struct wlr_scene_tree *scene_tree, struct wlr_surface *root) {
    struct root_buffer_search search = { .surface = root };
    wlr_scene_node_for_each_buffer(&scene_tree->node, find_root_buffer, &search);
    if (search.found != NULL && search.found->primary_output != NULL) {
        border->scale = search.found->primary_output->output->scale;
    }
}

static double clamp01(double v) {
    return v < 0.0 ? 0.0 : (v > 1.0 ? 1.0 : v);
}

// The top-left corner, `px` pixels square standing for R = width + radius
// logical pixels: the band between radius and R around the corner's
// bottom-right point, antialiased, in premultiplied RGBA. radius 0 is a solid
// square.
static struct wlr_buffer *draw_corner(int px, int width, int radius,
        const float color[3]) {
    uint8_t *pixels;
    size_t stride = (size_t)px * 4;
    struct wlr_buffer *buffer = oxide_pixel_buffer_create(px, px, stride, &pixels);
    if (buffer == NULL) {
        return NULL;
    }
    double outer = px; // R, in this buffer's pixels
    double inner = radius * (double)px / (width + radius);
    for (int y = 0; y < px; y++) {
        uint8_t *row = pixels + (size_t)y * stride;
        for (int x = 0; x < px; x++) {
            double coverage = 1.0;
            if (radius > 0) {
                double dx = px - (x + 0.5);
                double dy = px - (y + 0.5);
                double d = sqrt(dx * dx + dy * dy);
                double out = clamp01(outer - d + 0.5);
                double in = clamp01(d - inner + 1.5);
                coverage = out < in ? out : in;
            }
            for (int c = 0; c < 3; c++) {
                row[x * 4 + c] = (uint8_t)lround(clamp01(color[c]) * coverage * 255.0);
            }
            row[x * 4 + 3] = (uint8_t)lround(coverage * 255.0);
        }
    }
    return buffer;
}

void oxide_toplevel_update_border(struct wlr_scene_tree *scene_tree,
        struct wlr_xdg_toplevel *xdg_toplevel, int width, int radius,
        float r, float g, float b, void **border_inout) {
    struct oxide_border *border = *border_inout;
    struct wlr_surface *root = xdg_toplevel->base->surface;
    int w, h;
    if (width <= 0 || !root->mapped
            || !oxide_xdg_toplevel_window_box(xdg_toplevel, &w, &h)) {
        // Off, fullscreen, or unmapped: an unmapped window's own surfaces
        // hide themselves, but nothing hides this tree for us.
        if (border != NULL && border->tree != NULL) {
            wlr_scene_node_set_enabled(&border->tree->node, false);
        }
        return;
    }

    if (border == NULL) {
        border = calloc(1, sizeof(*border));
        if (border == NULL) {
            return;
        }
        border->scale = 1.0f;
        *border_inout = border;
    }
    if (border->tree != NULL && border->parent != scene_tree) {
        // Built under a tree this window no longer uses (one replaced
        // without destroying it). Start over under the current one.
        wl_list_remove(&border->tree_destroy.link);
        wlr_scene_node_destroy(&border->tree->node);
        border_forget_nodes(border);
    }
    if (border->tree == NULL && !border_build(border, scene_tree)) {
        return;
    }

    // Same clamp as the corner mask's, so the arcs stay concentric on a
    // window smaller than two radii.
    if (radius > w / 2) {
        radius = w / 2;
    }
    if (radius > h / 2) {
        radius = h / 2;
    }
    if (radius < 0) {
        radius = 0;
    }
    int outer = radius + width;

    update_scale(border, scene_tree, root);
    int px = (int)ceil(outer * border->scale - 0.001);
    if (px < 1) {
        px = 1;
    }
    const float color[3] = {r, g, b};
    if (px != border->drawn_px || width != border->drawn_width
            || radius != border->drawn_radius || r != border->drawn_color[0]
            || g != border->drawn_color[1] || b != border->drawn_color[2]) {
        struct wlr_buffer *corner = draw_corner(px, width, radius, color);
        if (corner == NULL) {
            return;
        }
        for (int i = 0; i < 4; i++) {
            wlr_scene_buffer_set_buffer(border->corners[i], corner);
        }
        wlr_buffer_drop(corner); // the scene buffers hold their own locks
        border->drawn_px = px;
        border->drawn_width = width;
        border->drawn_radius = radius;
        border->drawn_color[0] = r;
        border->drawn_color[1] = g;
        border->drawn_color[2] = b;
    }

    // All of these return early when nothing changed.
    const float rgba[4] = {r, g, b, 1.0f};
    for (int i = 0; i < 4; i++) {
        wlr_scene_rect_set_color(border->edges[i], rgba);
        wlr_scene_buffer_set_dest_size(border->corners[i], outer, outer);
    }
    wlr_scene_rect_set_size(border->edges[EDGE_TOP], w - 2 * radius, width);
    wlr_scene_node_set_position(&border->edges[EDGE_TOP]->node, radius, -width);
    wlr_scene_rect_set_size(border->edges[EDGE_BOTTOM], w - 2 * radius, width);
    wlr_scene_node_set_position(&border->edges[EDGE_BOTTOM]->node, radius, h);
    wlr_scene_rect_set_size(border->edges[EDGE_LEFT], width, h - 2 * radius);
    wlr_scene_node_set_position(&border->edges[EDGE_LEFT]->node, -width, radius);
    wlr_scene_rect_set_size(border->edges[EDGE_RIGHT], width, h - 2 * radius);
    wlr_scene_node_set_position(&border->edges[EDGE_RIGHT]->node, w, radius);
    wlr_scene_node_set_position(&border->corners[CORNER_TL]->node, -width, -width);
    wlr_scene_node_set_position(&border->corners[CORNER_TR]->node, w - radius, -width);
    wlr_scene_node_set_position(&border->corners[CORNER_BL]->node, -width, h - radius);
    wlr_scene_node_set_position(&border->corners[CORNER_BR]->node, w - radius, h - radius);
    wlr_scene_node_set_enabled(&border->tree->node, true);
}

void oxide_border_destroy(void *border_ptr) {
    struct oxide_border *border = border_ptr;
    if (border == NULL) {
        return;
    }
    if (border->tree != NULL) {
        wl_list_remove(&border->tree_destroy.link);
        wlr_scene_node_destroy(&border->tree->node);
    }
    free(border);
}
