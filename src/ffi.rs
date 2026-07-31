//! Raw declarations for the functions implemented in `shim/oxide_shim.c`.
//!
//! Every other module reaches wlroots through these — nothing here has logic
//! of its own, it's the FFI boundary.

use crate::wlr;
use std::os::raw::{c_char, c_void};

/// Type of the callbacks our C shim invokes: (userdata, signal-data).
pub(crate) type ShimCallback = unsafe extern "C" fn(*mut c_void, *mut c_void);

/// Keybinding callback: (userdata, keysym, modifiers, pressed) -> consumed?
pub(crate) type KeyCallback = unsafe extern "C" fn(*mut c_void, u32, u32, bool) -> bool;

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
    pub(crate) fn oxide_setup_signals(
        loop_: *mut wlr::wl_event_loop,
        display: *mut wlr::wl_display,
    );
    pub(crate) fn oxide_event_loop_add_readable(
        loop_: *mut wlr::wl_event_loop,
        fd: i32,
        callback: ShimCallback,
        userdata: *mut c_void,
    ) -> *mut c_void;
    pub(crate) fn oxide_event_loop_add_timer(
        loop_: *mut wlr::wl_event_loop,
        delay_ms: i32,
        callback: ShimCallback,
        userdata: *mut c_void,
    ) -> *mut c_void;
    pub(crate) fn oxide_event_source_remove(source: *mut c_void);
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
    pub(crate) fn oxide_scene_add_layer_tree(
        scene: *mut wlr::wlr_scene,
    ) -> *mut wlr::wlr_scene_tree;
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
    pub(crate) fn oxide_scene_tree_set_clip(tree: *mut wlr::wlr_scene_tree, width: i32, height: i32);
    pub(crate) fn oxide_scene_tree_set_enabled(tree: *mut wlr::wlr_scene_tree, enabled: bool);
    pub(crate) fn oxide_scene_tree_set_opacity(tree: *mut wlr::wlr_scene_tree, opacity: f32);
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
    // Double-tap hook: the callback's `data` is the tapped root wlr_surface
    // (same opaque `*mut c_void` shape as the focus callback above).
    pub(crate) fn oxide_cursor_set_double_tap_callback(
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
    pub(crate) fn oxide_cursor_set_locked(cursor: *mut wlr::wlr_cursor, locked: bool);
    pub(crate) fn oxide_session_lock_manager_create(display: *mut wlr::wl_display) -> *mut c_void;
    pub(crate) fn oxide_session_lock_manager_add_new_lock(
        manager: *mut c_void,
        callback: ShimCallback,
        userdata: *mut c_void,
    ) -> *mut ShimListener;
    pub(crate) fn oxide_session_lock_add_new_surface(
        lock: *mut c_void,
        callback: ShimCallback,
        userdata: *mut c_void,
    ) -> *mut ShimListener;
    pub(crate) fn oxide_session_lock_add_unlock(
        lock: *mut c_void,
        callback: ShimCallback,
        userdata: *mut c_void,
    ) -> *mut ShimListener;
    pub(crate) fn oxide_session_lock_add_destroy(
        lock: *mut c_void,
        callback: ShimCallback,
        userdata: *mut c_void,
    ) -> *mut ShimListener;
    pub(crate) fn oxide_session_lock_send_locked(lock: *mut c_void);
    pub(crate) fn oxide_session_lock_reject(lock: *mut c_void);
    pub(crate) fn oxide_session_lock_surface_output(surface: *mut c_void) -> *mut wlr::wlr_output;
    pub(crate) fn oxide_scene_session_lock_surface_create(
        parent: *mut wlr::wlr_scene_tree,
        surface: *mut c_void,
    ) -> *mut wlr::wlr_scene_tree;
    pub(crate) fn oxide_session_lock_surface_configure(
        surface: *mut c_void,
        width: u32,
        height: u32,
    );
    pub(crate) fn oxide_focus_session_lock_surface(seat: *mut wlr::wlr_seat, surface: *mut c_void);
    pub(crate) fn oxide_seat_clear_keyboard_focus(seat: *mut wlr::wlr_seat);
    pub(crate) fn oxide_session_lock_surface_add_map(
        surface: *mut c_void,
        callback: ShimCallback,
        userdata: *mut c_void,
    ) -> *mut ShimListener;
    pub(crate) fn oxide_session_lock_surface_add_destroy(
        surface: *mut c_void,
        callback: ShimCallback,
        userdata: *mut c_void,
    ) -> *mut ShimListener;
    pub(crate) fn oxide_xdg_toplevel_surface(toplevel: *mut wlr::wlr_xdg_toplevel) -> *mut c_void;

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

    // wlr-output-power-management-unstable-v1: wlroots owns the wire
    // protocol; we only react to `set_mode` and apply it via
    // oxide_output_set_powered.
    pub(crate) fn oxide_output_power_manager_add_set_mode(
        manager: *mut wlr::wlr_output_power_manager_v1,
        callback: ShimCallback,
        userdata: *mut c_void,
    ) -> *mut ShimListener;
    // `event` is the set_mode signal's data (opaque — never reached through
    // an allowlisted function signature, so bindgen never sees its type).
    pub(crate) fn oxide_output_power_set_mode_event_output(
        event: *mut c_void,
    ) -> *mut wlr::wlr_output;
    pub(crate) fn oxide_output_power_set_mode_event_is_on(event: *mut c_void) -> bool;
    pub(crate) fn oxide_output_set_powered(output: *mut wlr::wlr_output, enabled: bool);

    // Rounded-corner GLES2 masking: a compositor-owned shader program,
    // compiled once at startup, living for the process's lifetime (no
    // destroy — see oxide_shim.h). NULL on failure — treat as "unavailable".
    pub(crate) fn oxide_gles2_corner_program_create(
        renderer: *mut wlr::wlr_renderer,
    ) -> *mut c_void;
    // Best-effort: returns false and leaves the previous buffer in place on
    // any failure. `swapchain_inout`/`_w_inout`/`_h_inout` are the caller's
    // per-Toplevel corner_swapchain/_w/_h fields, updated in place.
    pub(crate) fn oxide_toplevel_apply_corner_radius(
        renderer: *mut wlr::wlr_renderer,
        allocator: *mut wlr::wlr_allocator,
        corner_program: *mut c_void,
        scene_tree: *mut wlr::wlr_scene_tree,
        root_surface: *mut c_void,
        radius: i32,
        dst_w: i32,
        dst_h: i32,
        swapchain_inout: *mut *mut c_void,
        swapchain_w_inout: *mut i32,
        swapchain_h_inout: *mut i32,
    ) -> bool;
    pub(crate) fn oxide_swapchain_destroy(swapchain: *mut c_void);
}
