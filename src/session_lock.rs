//! Secure ext-session-lock-v1 lifecycle and scene/input isolation.

use crate::ffi::*;
use crate::keybindings::focus_index;
use crate::state::{LockSurface, Server};
use crate::tiling::active_workspace;
use crate::wlr;
use std::os::raw::c_void;
use std::ptr;

pub(crate) unsafe fn setup(display: *mut wlr::wl_display, userdata: *mut c_void) {
    let manager = oxide_session_lock_manager_create(display);
    assert!(!manager.is_null(), "failed to create session-lock manager");
    oxide_session_lock_manager_add_new_lock(manager, handle_new_lock, userdata);
}

pub(crate) unsafe fn ensure_output_fallback(server: &mut Server, index: usize) {
    let output = &mut server.outputs[index];
    if output.lock_fallback.is_null() {
        output.lock_fallback = oxide_scene_add_rect(
            server.tree_session_lock,
            output.x,
            output.y,
            output.w,
            output.h,
            0.0,
            0.0,
            0.0,
            1.0,
        );
    }
}

unsafe fn cover_all_outputs(server: &mut Server) {
    oxide_scene_tree_set_enabled(server.tree_session_lock, true);
    for index in 0..server.outputs.len() {
        ensure_output_fallback(server, index);
    }
    for output in &server.outputs {
        oxide_output_schedule_frame(output.wlr_output);
    }
}

unsafe fn clear_fallbacks(server: &mut Server) {
    for output in &mut server.outputs {
        if !output.lock_fallback.is_null() {
            oxide_scene_rect_destroy(output.lock_fallback);
            output.lock_fallback = ptr::null_mut();
        }
        oxide_output_schedule_frame(output.wlr_output);
    }
}

unsafe extern "C" fn handle_new_lock(userdata: *mut c_void, data: *mut c_void) {
    let server = &mut *(userdata as *mut Server);
    let lock = data;
    if !server.active_lock.is_null() {
        eprintln!("0xin: rejecting a second concurrent session lock");
        oxide_session_lock_reject(lock);
        return;
    }

    server.active_lock = lock;
    server.locked = true;
    cover_all_outputs(server);
    oxide_seat_clear_keyboard_focus(server.seat);
    oxide_cursor_set_locked(server.cursor, true);

    server.lock_new_surface_listener =
        oxide_session_lock_add_new_surface(lock, handle_new_surface, userdata);
    server.lock_unlock_listener = oxide_session_lock_add_unlock(lock, handle_unlock, userdata);
    server.lock_destroy_listener =
        oxide_session_lock_add_destroy(lock, handle_lock_destroy, userdata);

    // The opaque fallback already secures every current output, so it is safe
    // to acknowledge the lock before the client maps its prettier surfaces.
    oxide_session_lock_send_locked(lock);
    eprintln!("0xin: session locked");
}

unsafe extern "C" fn handle_new_surface(userdata: *mut c_void, data: *mut c_void) {
    let server = &mut *(userdata as *mut Server);
    let lock_surface = data;
    let output = oxide_session_lock_surface_output(lock_surface);
    let Some(output_state) = server.outputs.iter().find(|item| item.wlr_output == output) else {
        eprintln!("0xin: lock surface targeted an unknown output");
        return;
    };

    let scene_tree =
        oxide_scene_session_lock_surface_create(server.tree_session_lock, lock_surface);
    oxide_scene_tree_set_position(scene_tree, output_state.x, output_state.y);
    oxide_session_lock_surface_configure(
        lock_surface,
        output_state.w as u32,
        output_state.h as u32,
    );

    let tracked = Box::into_raw(Box::new(LockSurface {
        server: userdata as *mut Server,
        lock_surface,
        map_listener: ptr::null_mut(),
        destroy_listener: ptr::null_mut(),
    }));
    let surface_userdata = tracked as *mut c_void;
    (*tracked).map_listener =
        oxide_session_lock_surface_add_map(lock_surface, handle_surface_map, surface_userdata);
    (*tracked).destroy_listener = oxide_session_lock_surface_add_destroy(
        lock_surface,
        handle_surface_destroy,
        surface_userdata,
    );
    server.lock_surfaces.push(tracked);
}

unsafe extern "C" fn handle_surface_map(userdata: *mut c_void, _data: *mut c_void) {
    let surface = &*(userdata as *mut LockSurface);
    let server = &mut *surface.server;
    if server.locked && server.active_lock.is_null() == false {
        oxide_focus_session_lock_surface(server.seat, surface.lock_surface);
    }
}

unsafe extern "C" fn handle_surface_destroy(userdata: *mut c_void, _data: *mut c_void) {
    let surface = userdata as *mut LockSurface;
    oxide_listener_remove((*surface).map_listener);
    oxide_listener_remove((*surface).destroy_listener);
    let server = &mut *(*surface).server;
    server.lock_surfaces.retain(|&item| item != surface);
    // wlroots' scene-subsurface helper follows surface destruction itself.
    drop(Box::from_raw(surface));
}

unsafe extern "C" fn handle_unlock(userdata: *mut c_void, _data: *mut c_void) {
    let server = &mut *(userdata as *mut Server);
    server.locked = false;
    oxide_scene_tree_set_enabled(server.tree_session_lock, false);
    clear_fallbacks(server);
    oxide_cursor_set_locked(server.cursor, false);

    if !server.outputs.is_empty() {
        let workspace = active_workspace(server);
        let focused = server.workspaces[workspace].focused;
        focus_index(server, focused);
    }
    eprintln!("0xin: session unlocked");
}

unsafe extern "C" fn handle_lock_destroy(userdata: *mut c_void, _data: *mut c_void) {
    let server = &mut *(userdata as *mut Server);
    oxide_listener_remove(server.lock_new_surface_listener);
    oxide_listener_remove(server.lock_unlock_listener);
    oxide_listener_remove(server.lock_destroy_listener);
    server.lock_new_surface_listener = ptr::null_mut();
    server.lock_unlock_listener = ptr::null_mut();
    server.lock_destroy_listener = ptr::null_mut();
    server.active_lock = ptr::null_mut();

    if server.locked {
        // The client vanished without an unlock request. Never fail open:
        // retain the opaque covers and accept a replacement locker.
        cover_all_outputs(server);
        oxide_seat_clear_keyboard_focus(server.seat);
        eprintln!("0xin: lock client disappeared; retaining secure fallback");
    }
}
