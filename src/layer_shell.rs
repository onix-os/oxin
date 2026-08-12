//! Layer-shell surfaces: bars, panels, wallpaper (e.g. quickshell).

use crate::ffi::*;
use crate::state::*;
use crate::tiling::{active_output, arrange_layers, refresh};
use std::os::raw::c_void;
use std::ptr;

/// Called by the shim when a client creates a layer-shell surface (a bar,
/// panel, or wallpaper — e.g. quickshell). Assigns an output if the client
/// didn't request one, places it in the scene tree matching its layer, and
/// registers its lifecycle listeners.
pub(crate) unsafe extern "C" fn handle_new_layer_surface(userdata: *mut c_void, data: *mut c_void) {
    let server = &mut *(userdata as *mut Server);
    let ls = data;

    let mut wlr_output = oxide_layer_surface_output(ls);
    if wlr_output.is_null() {
        if server.outputs.is_empty() {
            // No output to assign yet (e.g. right after a VT-switch tore one
            // down). Track it with a null output — handle_new_output attaches
            // it once an output actually appears, instead of it hanging
            // forever waiting for a configure that never comes.
            eprintln!("0xin: layer surface arrived with no output yet, deferring");
        } else {
            wlr_output = server.outputs[active_output(server)].wlr_output;
            oxide_layer_surface_set_output(ls, wlr_output);
        }
    }

    let tree = match oxide_layer_surface_layer(ls) {
        0 => server.tree_layer_bg,
        1 => server.tree_layer_bottom,
        3 => server.tree_layer_overlay,
        _ => server.tree_layer_top, // 2 (top), and any unknown value
    };
    let scene_ls = oxide_scene_layer_surface_create(tree, ls);

    let lsurf = Box::into_raw(Box::new(LayerSurface {
        server: userdata as *mut Server,
        wlr_layer_surface: ls,
        scene_ls,
        wlr_output,
        commit_listener: ptr::null_mut(),
        map_listener: ptr::null_mut(),
        unmap_listener: ptr::null_mut(),
        destroy_listener: ptr::null_mut(),
    }));
    let ud = lsurf as *mut c_void;
    (*lsurf).commit_listener = oxide_layer_surface_add_commit(ls, handle_layer_commit, ud);
    (*lsurf).map_listener = oxide_layer_surface_add_map(ls, handle_layer_map, ud);
    (*lsurf).unmap_listener = oxide_layer_surface_add_unmap(ls, handle_layer_unmap, ud);
    (*lsurf).destroy_listener = oxide_layer_surface_add_destroy(ls, handle_layer_destroy, ud);

    server.layers.push(lsurf);
}

/// Re-arrange the layer surface's output whenever one of its lifecycle events
/// fires. The initial commit's configure is what lets the client map.
unsafe fn rearrange_layer_output(l: &LayerSurface) {
    let server = &mut *l.server;
    if let Some(idx) = server
        .outputs
        .iter()
        .position(|o| o.wlr_output == l.wlr_output)
    {
        arrange_layers(server, idx);
    }
    refresh(server);
}

unsafe extern "C" fn handle_layer_commit(userdata: *mut c_void, _data: *mut c_void) {
    rearrange_layer_output(&*(userdata as *mut LayerSurface));
}

unsafe extern "C" fn handle_layer_map(userdata: *mut c_void, _data: *mut c_void) {
    rearrange_layer_output(&*(userdata as *mut LayerSurface));
}

// Deliberately not rearranging here (unlike commit/map): at the moment unmap
// fires, initialized/configured are still true — wlroots hasn't finished
// tearing the surface down yet, that happens a beat later once the generic
// commit signal fires. A configure sent from here reaches the client, gets
// legitimately ack'd, but by the time the ack arrives wlroots has already
// invalidated it as part of finishing the unmap — a fatal "wrong configure
// serial" protocol error that kills the client's connection outright.
// Confirmed via a live traced repro (initialized/configured were true at
// unmap, false three events later at the next commit). A surface that's
// disappearing doesn't need a fresh configure anyway — the next real state
// change (a future commit/map on reshow) already triggers one.
unsafe extern "C" fn handle_layer_unmap(_userdata: *mut c_void, _data: *mut c_void) {}

/// The layer surface was destroyed: unregister its listeners, drop it from
/// tracking, and re-arrange the output it was on.
unsafe extern "C" fn handle_layer_destroy(userdata: *mut c_void, _data: *mut c_void) {
    let l = userdata as *mut LayerSurface;
    oxide_listener_remove((*l).commit_listener);
    oxide_listener_remove((*l).map_listener);
    oxide_listener_remove((*l).unmap_listener);
    oxide_listener_remove((*l).destroy_listener);

    let server = &mut *(*l).server;
    let wlr_output = (*l).wlr_output;
    server.layers.retain(|&x| x != l);
    if let Some(idx) = server
        .outputs
        .iter()
        .position(|o| o.wlr_output == wlr_output)
    {
        arrange_layers(server, idx);
    }
    refresh(server);
    drop(Box::from_raw(l));
}
