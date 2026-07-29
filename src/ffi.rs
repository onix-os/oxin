//! Raw declarations for the functions implemented in `shim/oxide_shim.c`.
//!
//! Every other module reaches wlroots through these — nothing here has logic
//! of its own, it's the FFI boundary.

use crate::wlr;
use std::os::raw::{c_char, c_void};

/// Type of the callbacks our C shim invokes: (userdata, signal-data).
pub(crate) type ShimCallback = unsafe extern "C" fn(*mut c_void, *mut c_void);

/// Keybinding callback: (userdata, keysym, modifiers) -> was it consumed?
pub(crate) type KeyCallback = unsafe extern "C" fn(*mut c_void, u32, u32) -> bool;

/// Pointer-grab button callback: (userdata, clicked root wlr_surface — NULL
/// on release, button, modifiers, pressed, cursor x, cursor y) -> did a grab
/// start/end (consume the event)?
pub(crate) type GrabButtonCallback =
    unsafe extern "C" fn(*mut c_void, *mut c_void, u32, u32, bool, f64, f64) -> bool;

/// Pointer-grab motion callback: (userdata, cursor x, cursor y) -> is a grab
/// active (it handled the motion)?
pub(crate) type GrabMotionCallback = unsafe extern "C" fn(*mut c_void, f64, f64) -> bool;

/// Named compositor gesture trigger callback.
pub(crate) type GestureCallback = unsafe extern "C" fn(*mut c_void, u32);

/// Opaque handle to a `oxide_listener` living on the C heap.
#[repr(C)]
pub(crate) struct ShimListener {
    _opaque: [u8; 0],
}

// Functions implemented in shim/oxide_shim.c.
extern "C" {
    pub(crate) fn oxide_log_init();
    pub(crate) fn oxide_setup_signals(loop_: *mut wlr::wl_event_loop, display: *mut wlr::wl_display);
    pub(crate) fn oxide_event_loop_add_readable(
        loop_: *mut wlr::wl_event_loop,
        fd: i32,
        callback: ShimCallback,
        userdata: *mut c_void,
    ) -> *mut c_void;
    pub(crate) fn oxide_reset_child_signals();
    pub(crate) fn oxide_virtual_keyboard_setup(
        display: *mut wlr::wl_display,
        seat: *mut wlr::wlr_seat,
    );
    pub(crate) fn oxide_session_change_vt(session: *mut wlr::wlr_session, vt: u32);
    pub(crate) fn oxide_session_add_active(
        session: *mut wlr::wlr_session,
        callback: ShimCallback,
        userdata: *mut c_void,
    ) -> *mut ShimListener;
    pub(crate) fn oxide_session_is_active(session: *mut wlr::wlr_session) -> bool;
    pub(crate) fn oxide_backend_add_new_output(
        backend: *mut wlr::wlr_backend,
        callback: ShimCallback,
        userdata: *mut c_void,
    ) -> *mut ShimListener;
    pub(crate) fn oxide_output_add_frame(
        output: *mut wlr::wlr_output,
        callback: ShimCallback,
        userdata: *mut c_void,
    ) -> *mut ShimListener;
    pub(crate) fn oxide_output_enable(output: *mut wlr::wlr_output, scale: f32);
    pub(crate) fn oxide_output_name(output: *mut wlr::wlr_output) -> *const c_char;
    pub(crate) fn oxide_scene_add_layer_tree(scene: *mut wlr::wlr_scene) -> *mut wlr::wlr_scene_tree;
    pub(crate) fn oxide_scene_add_output_background(
        tree: *mut wlr::wlr_scene_tree,
        output: *mut wlr::wlr_output,
        x: i32,
        y: i32,
        r: f32,
        g: f32,
        b: f32,
    ) -> *mut c_void; // the background rect (opaque to Rust)
    pub(crate) fn oxide_scene_add_rect(
        tree: *mut wlr::wlr_scene_tree,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        r: f32,
        g: f32,
        b: f32,
        a: f32,
    ) -> *mut c_void;
    pub(crate) fn oxide_scene_rect_destroy(rect: *mut c_void);
    pub(crate) fn oxide_scene_rect_set_enabled(rect: *mut c_void, enabled: bool);
    pub(crate) fn oxide_scene_rect_set_position(rect: *mut c_void, x: i32, y: i32);
    pub(crate) fn oxide_scene_add_wallpaper(
        tree: *mut wlr::wlr_scene_tree,
        x: i32,
        y: i32,
        buffer_width: i32,
        buffer_height: i32,
        dest_width: i32,
        dest_height: i32,
        pixels: *const u8,
        stride: usize,
    ) -> *mut c_void;
    pub(crate) fn oxide_scene_wallpaper_destroy(wallpaper: *mut c_void);
    pub(crate) fn oxide_output_add_destroy(
        output: *mut wlr::wlr_output,
        callback: ShimCallback,
        userdata: *mut c_void,
    ) -> *mut ShimListener;
    pub(crate) fn oxide_output_layout_get_box(
        layout: *mut wlr::wlr_output_layout,
        output: *mut wlr::wlr_output,
        x: *mut i32,
        y: *mut i32,
        width: *mut i32,
        height: *mut i32,
    );
    pub(crate) fn oxide_output_at_cursor(
        cursor: *mut wlr::wlr_cursor,
        layout: *mut wlr::wlr_output_layout,
    ) -> *mut wlr::wlr_output;
    pub(crate) fn oxide_scene_output_render(scene_output: *mut wlr::wlr_scene_output);
    pub(crate) fn oxide_output_schedule_frame(output: *mut wlr::wlr_output);
    pub(crate) fn oxide_xdg_shell_add_new_toplevel(
        shell: *mut wlr::wlr_xdg_shell,
        callback: ShimCallback,
        userdata: *mut c_void,
    ) -> *mut ShimListener;
    pub(crate) fn oxide_xdg_shell_setup_popups(shell: *mut wlr::wlr_xdg_shell);
    pub(crate) fn oxide_scene_add_xdg_toplevel(
        tree: *mut wlr::wlr_scene_tree,
        toplevel: *mut wlr::wlr_xdg_toplevel,
    ) -> *mut wlr::wlr_scene_tree;
    pub(crate) fn oxide_xdg_add_commit(
        toplevel: *mut wlr::wlr_xdg_toplevel,
        callback: ShimCallback,
        userdata: *mut c_void,
    ) -> *mut ShimListener;
    pub(crate) fn oxide_xdg_initial_commit(toplevel: *mut wlr::wlr_xdg_toplevel) -> bool;
    pub(crate) fn oxide_xdg_toplevel_set_tiled_all(toplevel: *mut wlr::wlr_xdg_toplevel);
    pub(crate) fn oxide_xdg_toplevel_set_tiled_none(toplevel: *mut wlr::wlr_xdg_toplevel);
    // Float detection: dialog parent (NULL if none), app id (NULL if unset),
    // client-declared fixed size, and current geometry size (for centering).
    pub(crate) fn oxide_xdg_toplevel_parent(
        toplevel: *mut wlr::wlr_xdg_toplevel,
    ) -> *mut wlr::wlr_xdg_toplevel;
    pub(crate) fn oxide_xdg_toplevel_app_id(
        toplevel: *mut wlr::wlr_xdg_toplevel,
    ) -> *const std::os::raw::c_char;
    pub(crate) fn oxide_xdg_toplevel_fixed_size(toplevel: *mut wlr::wlr_xdg_toplevel) -> bool;
    pub(crate) fn oxide_xdg_toplevel_geometry(
        toplevel: *mut wlr::wlr_xdg_toplevel,
        width: *mut i32,
        height: *mut i32,
    );
    pub(crate) fn oxide_listener_remove(listener: *mut ShimListener);
    pub(crate) fn oxide_xdg_add_map(
        toplevel: *mut wlr::wlr_xdg_toplevel,
        callback: ShimCallback,
        userdata: *mut c_void,
    ) -> *mut ShimListener;
    pub(crate) fn oxide_xdg_add_unmap(
        toplevel: *mut wlr::wlr_xdg_toplevel,
        callback: ShimCallback,
        userdata: *mut c_void,
    ) -> *mut ShimListener;
    pub(crate) fn oxide_xdg_add_destroy(
        toplevel: *mut wlr::wlr_xdg_toplevel,
        callback: ShimCallback,
        userdata: *mut c_void,
    ) -> *mut ShimListener;
    pub(crate) fn oxide_xdg_add_request_fullscreen(
        toplevel: *mut wlr::wlr_xdg_toplevel,
        callback: ShimCallback,
        userdata: *mut c_void,
    ) -> *mut ShimListener;
    pub(crate) fn oxide_xdg_toplevel_requested_fullscreen(
        toplevel: *mut wlr::wlr_xdg_toplevel,
    ) -> bool;
    pub(crate) fn oxide_scene_tree_reparent(
        tree: *mut wlr::wlr_scene_tree,
        new_parent: *mut wlr::wlr_scene_tree,
    );
    pub(crate) fn oxide_scene_tree_set_position(tree: *mut wlr::wlr_scene_tree, x: i32, y: i32);
    pub(crate) fn oxide_scene_tree_set_enabled(tree: *mut wlr::wlr_scene_tree, enabled: bool);
    pub(crate) fn oxide_scene_tree_destroy(tree: *mut wlr::wlr_scene_tree);
    pub(crate) fn oxide_focus_toplevel(
        seat: *mut wlr::wlr_seat,
        toplevel: *mut wlr::wlr_xdg_toplevel,
    );
    pub(crate) fn oxide_seat_create(
        display: *mut wlr::wl_display,
        name: *const c_char,
    ) -> *mut wlr::wlr_seat;
    pub(crate) fn oxide_backend_add_new_input(
        backend: *mut wlr::wlr_backend,
        callback: ShimCallback,
        userdata: *mut c_void,
    ) -> *mut ShimListener;
    pub(crate) fn oxide_handle_new_input(
        seat: *mut wlr::wlr_seat,
        cursor: *mut wlr::wlr_cursor,
        device: *mut wlr::wlr_input_device,
        key_callback: KeyCallback,
        key_userdata: *mut c_void,
    );
    pub(crate) fn oxide_cursor_setup(
        layout: *mut wlr::wlr_output_layout,
        scene: *mut wlr::wlr_scene,
        seat: *mut wlr::wlr_seat,
    ) -> *mut wlr::wlr_cursor;
    // Click-focus hook: the callback's `data` is the clicked root wlr_surface
    // (opaque `*mut c_void` in Rust, matched by pointer identity against
    // oxide_xdg_toplevel_surface). Registered separately from cursor setup
    // because the Server userdata doesn't exist yet at that point.
    pub(crate) fn oxide_cursor_set_focus_callback(
        cursor: *mut wlr::wlr_cursor,
        callback: ShimCallback,
        userdata: *mut c_void,
    );
    // A toplevel's root wlr_surface, for matching clicks back to windows.
    pub(crate) fn oxide_cursor_set_grab_callbacks(
        cursor: *mut wlr::wlr_cursor,
        button_callback: GrabButtonCallback,
        motion_callback: GrabMotionCallback,
        userdata: *mut c_void,
    );
    pub(crate) fn oxide_cursor_set_gestures(
        cursor: *mut wlr::wlr_cursor,
        layout: *mut wlr::wlr_output_layout,
        enabled_mask: u32,
        keyboard_height: i32,
        callback: GestureCallback,
        userdata: *mut c_void,
    );
    pub(crate) fn oxide_cursor_set_keyboard_visible(cursor: *mut wlr::wlr_cursor, visible: bool);
    pub(crate) fn oxide_cursor_set_keyboard_height(cursor: *mut wlr::wlr_cursor, height: i32);
    pub(crate) fn oxide_xdg_toplevel_surface(
        toplevel: *mut wlr::wlr_xdg_toplevel,
    ) -> *mut c_void;

    // Layer-shell (bars, panels, wallpaper). Layer surfaces and the scene
    // helper wrapping them stay opaque `*mut c_void` in Rust, same as the
    // background rect above — we only ever pass them back into these helpers.
    pub(crate) fn oxide_layer_shell_add_new_surface(
        shell: *mut wlr::wlr_layer_shell_v1,
        callback: ShimCallback,
        userdata: *mut c_void,
    ) -> *mut ShimListener;
    pub(crate) fn oxide_layer_surface_output(ls: *mut c_void) -> *mut wlr::wlr_output;
    pub(crate) fn oxide_layer_surface_set_output(ls: *mut c_void, output: *mut wlr::wlr_output);
    pub(crate) fn oxide_layer_surface_layer(ls: *mut c_void) -> u32;
    pub(crate) fn oxide_scene_layer_surface_create(
        tree: *mut wlr::wlr_scene_tree,
        ls: *mut c_void,
    ) -> *mut c_void;
    pub(crate) fn oxide_scene_layer_surface_configure(
        scene_ls: *mut c_void,
        fx: i32,
        fy: i32,
        fw: i32,
        fh: i32,
        ux: *mut i32,
        uy: *mut i32,
        uw: *mut i32,
        uh: *mut i32,
    );
    pub(crate) fn oxide_layer_surface_add_commit(
        ls: *mut c_void,
        callback: ShimCallback,
        userdata: *mut c_void,
    ) -> *mut ShimListener;
    pub(crate) fn oxide_layer_surface_add_map(
        ls: *mut c_void,
        callback: ShimCallback,
        userdata: *mut c_void,
    ) -> *mut ShimListener;
    pub(crate) fn oxide_layer_surface_add_unmap(
        ls: *mut c_void,
        callback: ShimCallback,
        userdata: *mut c_void,
    ) -> *mut ShimListener;
    pub(crate) fn oxide_layer_surface_add_destroy(
        ls: *mut c_void,
        callback: ShimCallback,
        userdata: *mut c_void,
    ) -> *mut ShimListener;

    // xdg-decoration: force server-side mode so clients skip drawing their
    // own title bar. The decoration object stays opaque `*mut c_void`, same
    // treatment as the layer-shell surface above.
    pub(crate) fn oxide_xdg_decoration_manager_add_new_toplevel_decoration(
        manager: *mut wlr::wlr_xdg_decoration_manager_v1,
        callback: ShimCallback,
        userdata: *mut c_void,
    ) -> *mut ShimListener;
    pub(crate) fn oxide_xdg_toplevel_decoration_set_server_side(decoration: *mut c_void);
}
