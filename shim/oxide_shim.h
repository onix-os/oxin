#ifndef OXIN_SHIM_H
#define OXIN_SHIM_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

// Opaque to Rust; full definitions live in the wlroots headers / shim .c.
struct wl_display;
struct wl_event_loop;
struct wlr_backend;
struct wlr_output;
struct wlr_scene;
struct wlr_scene_output;
struct wlr_scene_tree;
struct wlr_scene_rect;
struct wlr_xdg_shell;
struct wlr_xdg_toplevel;
struct wlr_layer_shell_v1;
struct wlr_layer_surface_v1;
struct wlr_scene_layer_surface_v1;
struct wlr_xdg_decoration_manager_v1;
struct wlr_seat;
struct wlr_input_device;
struct wlr_cursor;
struct wlr_output_layout;
struct wlr_session;
struct oxide_listener;

// Generic event callback handed to Rust: (userdata, signal-data).
typedef void (*oxide_callback)(void *userdata, void *data);

// Key callback: returns true if the keysym was consumed as a binding (and so
// must NOT be forwarded to the focused client).
typedef bool (*oxide_key_callback)(void *userdata, uint32_t keysym,
        uint32_t modifiers, bool pressed);

// Pointer-grab callbacks (Mod+drag move/resize of floating windows).
// Button: called on press (with the clicked root surface, held modifiers and
// cursor position) and on release (root_surface NULL, modifiers 0); returning
// true consumes the event — a grab started or ended, don't forward it to the
// client. Motion: called with the cursor's layout position; returning true
// means a grab is active and handled the motion.
struct wlr_surface;
typedef bool (*oxide_grab_button_callback)(void *userdata,
        struct wlr_surface *root_surface, uint32_t button, uint32_t modifiers,
        bool pressed, double cx, double cy);
typedef bool (*oxide_grab_motion_callback)(void *userdata, double cx,
        double cy);
typedef void (*oxide_gesture_callback)(void *userdata, uint32_t trigger);

// --- toolchain / logging ---------------------------------------------------
const char *oxide_wlroots_version(void);
void oxide_log_init(void);

// Resolve a key name (case-insensitive, e.g. "Return"/"q"/"1") to an xkb keysym
// for the config parser. Returns 0 for an unknown name.
uint32_t oxide_keysym_from_name(const char *name);

// Terminate the display loop on SIGINT/SIGTERM (graceful shutdown).
void oxide_setup_signals(struct wl_event_loop *loop, struct wl_display *display);
// Call `callback(userdata, NULL)` whenever fd becomes readable.
void *oxide_event_loop_add_readable(struct wl_event_loop *loop, int fd,
        oxide_callback callback, void *userdata);
void *oxide_event_loop_add_timer(struct wl_event_loop *loop, int delay_ms,
        oxide_callback callback, void *userdata);
void oxide_event_source_remove(void *source);
// Reset the compositor's signal state (ignored SIGCHLD, blocked INT/TERM) in
// a forked child before exec, so clients start with clean signals. Must be
// called from every spawn path's pre_exec.
void oxide_reset_child_signals(void);

// Publish virtual-keyboard-unstable-v1 and attach each client-created virtual
// keyboard to `seat`. Virtual keys bypass compositor keybindings.
void oxide_virtual_keyboard_setup(struct wl_display *display,
        struct wlr_seat *seat);

// Switch to virtual terminal `vt` (1-based); no-op if `session` is NULL.
void oxide_session_change_vt(struct wlr_session *session, unsigned vt);
// Subscribe to session active changes (VT switch away/back). NULL if no session.
struct oxide_listener *oxide_session_add_active(struct wlr_session *session,
        oxide_callback callback, void *userdata);
// True while the session owns the VT (false while switched away).
bool oxide_session_is_active(struct wlr_session *session);

// --- listener glue ---------------------------------------------------------
struct oxide_listener *oxide_backend_add_new_output(
        struct wlr_backend *backend, oxide_callback callback, void *userdata);
struct oxide_listener *oxide_output_add_frame(
        struct wlr_output *output, oxide_callback callback, void *userdata);
// Fires when an output is destroyed (unplug, or VT-switch seat disable).
struct oxide_listener *oxide_output_add_destroy(
        struct wlr_output *output, oxide_callback callback, void *userdata);

// --- output / scene helpers ------------------------------------------------
// Enable the output (owns the wlr_output_state init/commit/finish dance),
// applying `scale` (1.0 if the config doesn't set one).
void oxide_output_enable(struct wlr_output *output, float scale);
// The connector name (e.g. "eDP-1", "HDMI-A-1"), for matching against a
// `monitor = NAME, ...` config entry.
const char *oxide_output_name(struct wlr_output *output);
// Create an ordered child tree directly under the scene root. Rust calls this
// once per z-layer at startup (bg_fallback, layer_bg, layer_bottom, normal,
// layer_top, layer_overlay); creation order is paint order (later = on top).
struct wlr_scene_tree *oxide_scene_add_layer_tree(struct wlr_scene *scene);
// Add a solid-color background rectangle, sized to `output`, positioned at the
// output's (x, y) in the layout so multiple outputs don't overlap at the
// origin, under `tree` (the bg_fallback layer). Returns the rect so Rust can
// destroy it when the output goes away.
struct wlr_scene_rect *oxide_scene_add_output_background(struct wlr_scene_tree *tree,
        struct wlr_output *output, int x, int y, float r, float g, float b);
struct wlr_scene_rect *oxide_scene_add_rect(struct wlr_scene_tree *tree,
        int x, int y, int width, int height, float r, float g, float b,
        float a);
// Destroy a background rect created above.
void oxide_scene_rect_destroy(struct wlr_scene_rect *rect);
// Enable/disable a background rect (toggle to force a full-output repaint).
void oxide_scene_rect_set_enabled(struct wlr_scene_rect *rect, bool enabled);
void oxide_scene_rect_set_position(struct wlr_scene_rect *rect, int x, int y);
// Copy an RGBA8888 pixel image into an owned wlroots buffer and add it to the
// scene. The buffer may be a different size from its logical destination.
void *oxide_scene_add_wallpaper(struct wlr_scene_tree *tree, int x, int y,
        int buffer_width, int buffer_height, int dest_width, int dest_height,
        const uint8_t *pixels, size_t stride);
void oxide_scene_wallpaper_destroy(void *wallpaper);
// Read an output's layout box (position + pixel size) for per-output tiling.
void oxide_output_layout_get_box(struct wlr_output_layout *layout,
        struct wlr_output *output, int *x, int *y, int *width, int *height);
// The output currently under the cursor (NULL if none).
struct wlr_output *oxide_output_at_cursor(struct wlr_cursor *cursor,
        struct wlr_output_layout *layout);
// Render + present one frame for this scene output (owns the timespec/clock).
void oxide_scene_output_render(struct wlr_scene_output *scene_output);
// Request a `frame` event once the output is ready (kicks the first paint).
void oxide_output_schedule_frame(struct wlr_output *output);

// --- xdg-shell (app windows) ----------------------------------------------
struct oxide_listener *oxide_xdg_shell_add_new_toplevel(
        struct wlr_xdg_shell *shell, oxide_callback callback, void *userdata);
// Configure XDG popups after their initial commit so they can map.
void oxide_xdg_shell_setup_popups(struct wlr_xdg_shell *shell);
// Add a toplevel to the scene graph, under `tree` (the normal/app-window
// layer). Returns the scene tree node so Rust can position it. (Use
// oxide_xdg_add_commit for the initial-configure listener.)
struct wlr_scene_tree *oxide_scene_add_xdg_toplevel(struct wlr_scene_tree *tree,
        struct wlr_xdg_toplevel *toplevel);

// Commit listener (fires every commit; Rust filters for the initial one and
// answers it with a sized configure). Returns the listener so Rust can
// remove it on destroy with the others.
struct oxide_listener *oxide_xdg_add_commit(struct wlr_xdg_toplevel *toplevel,
        oxide_callback callback, void *userdata);
// True only on the client's very first commit (must be answered with a
// configure or the client never maps).
bool oxide_xdg_initial_commit(struct wlr_xdg_toplevel *toplevel);
// Mark a window tiled on all edges, making our configure sizes binding
// rather than a "floating" hint clients may override with a saved size.
void oxide_xdg_toplevel_set_tiled_all(struct wlr_xdg_toplevel *toplevel);
// Clear the tiled states again (the tiled -> floating toggle): configures go
// back to floating semantics and the client may keep its natural size.
void oxide_xdg_toplevel_set_tiled_none(struct wlr_xdg_toplevel *toplevel);

// Float-detection accessors: a dialog's parent toplevel (NULL if none), the
// client's app id (NULL if never set), whether the client committed a fixed
// (min == max, nonzero) size, and the current effective geometry size.
struct wlr_xdg_toplevel *oxide_xdg_toplevel_parent(
        struct wlr_xdg_toplevel *toplevel);
const char *oxide_xdg_toplevel_app_id(struct wlr_xdg_toplevel *toplevel);
bool oxide_xdg_toplevel_fixed_size(struct wlr_xdg_toplevel *toplevel);
void oxide_xdg_toplevel_geometry(struct wlr_xdg_toplevel *toplevel,
        int *width, int *height);

// Unsubscribe and free a listener returned by one of the add helpers.
void oxide_listener_remove(struct oxide_listener *listener);

// Lifecycle listeners on a toplevel (Rust drives window tracking & layout).
struct oxide_listener *oxide_xdg_add_map(struct wlr_xdg_toplevel *toplevel,
        oxide_callback callback, void *userdata);
struct oxide_listener *oxide_xdg_add_unmap(struct wlr_xdg_toplevel *toplevel,
        oxide_callback callback, void *userdata);
struct oxide_listener *oxide_xdg_add_destroy(struct wlr_xdg_toplevel *toplevel,
        oxide_callback callback, void *userdata);
// The client asked to enter/leave fullscreen (must be answered with a
// configure — Rust calls wlr_xdg_toplevel_set_fullscreen either way).
struct oxide_listener *oxide_xdg_add_request_fullscreen(
        struct wlr_xdg_toplevel *toplevel, oxide_callback callback,
        void *userdata);
// The fullscreen state the client currently wants.
bool oxide_xdg_toplevel_requested_fullscreen(struct wlr_xdg_toplevel *toplevel);

// Layout helpers: move a window's scene node; give a window keyboard focus;
// read an output's pixel size. (All touch wlroots struct internals.)
void oxide_scene_tree_set_position(struct wlr_scene_tree *tree, int x, int y);
void oxide_scene_tree_set_enabled(struct wlr_scene_tree *tree, bool enabled);
// Apply one opacity value to every buffer currently under a toplevel tree.
void oxide_scene_tree_set_opacity(struct wlr_scene_tree *tree, float opacity);
// Move a window's scene tree between layer trees (normal <-> fullscreen).
void oxide_scene_tree_reparent(struct wlr_scene_tree *tree,
        struct wlr_scene_tree *new_parent);
// Destroy a window's scene tree (to rebuild it on VT resume).
void oxide_scene_tree_destroy(struct wlr_scene_tree *tree);
void oxide_focus_toplevel(struct wlr_seat *seat,
        struct wlr_xdg_toplevel *toplevel);
// The toplevel's root wlr_surface (for matching a clicked surface to a window).
struct wlr_surface *oxide_xdg_toplevel_surface(struct wlr_xdg_toplevel *toplevel);
void oxide_output_get_size(struct wlr_output *output, int *width, int *height);

// --- layer-shell (bars, panels, wallpaper) ----------------------------------
//
// Mirrors the xdg-shell wiring above: wlroots implements the protocol and a
// scene helper that does anchor/margin math and exclusive-zone bookkeeping;
// we expose that plus lifecycle listeners. Rust owns which tree (z-layer) and
// output each surface lands on.

// The zwlr_layer_shell_v1 global is created directly from Rust via bindgen's
// wlr_layer_shell_v1_create binding (same pattern as wlr_xdg_shell_create).
struct oxide_listener *oxide_layer_shell_add_new_surface(
        struct wlr_layer_shell_v1 *shell, oxide_callback callback, void *userdata);

// NULL if the client didn't request a specific output; Rust must assign one
// before returning from the new_surface handler.
struct wlr_output *oxide_layer_surface_output(struct wlr_layer_surface_v1 *ls);
void oxide_layer_surface_set_output(struct wlr_layer_surface_v1 *ls,
        struct wlr_output *output);
// Requested z-layer: 0=background, 1=bottom, 2=top, 3=overlay
// (zwlr_layer_shell_v1_layer).
uint32_t oxide_layer_surface_layer(struct wlr_layer_surface_v1 *ls);

// Add the layer surface (and its sub-surfaces/popups) to the scene, under the
// tree matching its layer.
struct wlr_scene_layer_surface_v1 *oxide_scene_layer_surface_create(
        struct wlr_scene_tree *tree, struct wlr_layer_surface_v1 *ls);
// Position/size the surface per its anchors+margins within the output box
// (fx,fy,fw,fh), and shrink the usable box (ux,uy,uw,uh, in/out) by its
// exclusive zone. Also sends the layer_surface.configure event to the client.
void oxide_scene_layer_surface_configure(struct wlr_scene_layer_surface_v1 *scene_ls,
        int fx, int fy, int fw, int fh, int *ux, int *uy, int *uw, int *uh);

// Lifecycle listeners on a layer surface (Rust drives arrange + tiling).
struct oxide_listener *oxide_layer_surface_add_commit(struct wlr_layer_surface_v1 *ls,
        oxide_callback callback, void *userdata);
struct oxide_listener *oxide_layer_surface_add_map(struct wlr_layer_surface_v1 *ls,
        oxide_callback callback, void *userdata);
struct oxide_listener *oxide_layer_surface_add_unmap(struct wlr_layer_surface_v1 *ls,
        oxide_callback callback, void *userdata);
struct oxide_listener *oxide_layer_surface_add_destroy(struct wlr_layer_surface_v1 *ls,
        oxide_callback callback, void *userdata);

// --- xdg-decoration (server-side window decoration negotiation) ------------
//
// The manager global is created directly from Rust via bindgen's
// wlr_xdg_decoration_manager_v1_create binding (same pattern as
// wlr_xdg_shell_create) — no shim wrapper needed for a plain creation call.

struct oxide_listener *oxide_xdg_decoration_manager_add_new_toplevel_decoration(
        struct wlr_xdg_decoration_manager_v1 *manager, oxide_callback callback,
        void *userdata);
// Force server-side mode: the client stops drawing its own title bar/border.
// We draw nothing in its place (bare windows), so this is a one-shot,
// fire-and-forget call — no per-decoration state to track or clean up.
void oxide_xdg_toplevel_decoration_set_server_side(void *decoration);

// --- seat & input ----------------------------------------------------------
// Create the wl_seat global; returns the seat so Rust can wire input/focus.
struct wlr_seat *oxide_seat_create(struct wl_display *display, const char *name);
// Subscribe to the backend's new_input signal (data = wlr_input_device).
struct oxide_listener *oxide_backend_add_new_input(
        struct wlr_backend *backend, oxide_callback callback, void *userdata);
// Wire a new input device: keyboards get an xkb keymap + event forwarding
// (key presses are offered to `key_callback` first), pointers get attached to
// the cursor. Other device types are ignored.
void oxide_handle_new_input(struct wlr_seat *seat, struct wlr_cursor *cursor,
        struct wlr_input_device *device, oxide_key_callback key_callback,
        void *key_userdata);

// Create a cursor over the output layout, route its events through scene
// hit-testing to the seat, and show a default cursor image. Returns the cursor
// so Rust can attach pointer devices to it.
struct wlr_cursor *oxide_cursor_setup(struct wlr_output_layout *layout,
        struct wlr_scene *scene, struct wlr_seat *seat);

// Register a hook called on every click with the clicked root wlr_surface,
// so Rust's focus bookkeeping (Workspace.focused) follows mouse focus.
// Separate from oxide_cursor_setup: its userdata (the Server) is only
// constructed after the cursor exists.
void oxide_cursor_set_focus_callback(struct wlr_cursor *cursor,
        oxide_callback callback, void *userdata);

// Register the pointer-grab hooks (see the callback typedefs above). Same
// late-registration story as the focus callback: the Server userdata is only
// constructed after the cursor.
void oxide_cursor_set_grab_callbacks(struct wlr_cursor *cursor,
        oxide_grab_button_callback button_callback,
        oxide_grab_motion_callback motion_callback, void *userdata);
// Enable only the named gesture triggers selected by `enabled_mask` and emit
// their stable numeric IDs through one callback. Rust maps IDs to policy.
void oxide_cursor_set_gestures(struct wlr_cursor *cursor,
        struct wlr_output_layout *layout, uint32_t enabled_mask,
        int keyboard_height, oxide_gesture_callback callback, void *userdata);
void oxide_cursor_set_keyboard_visible(struct wlr_cursor *cursor, bool visible);
void oxide_cursor_set_keyboard_height(struct wlr_cursor *cursor, int height);

#endif // OXIN_SHIM_H
