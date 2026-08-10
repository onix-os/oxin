//! 0xin — a dynamic tiling Wayland compositor on wlroots.
//!
//! `main()` brings up the wlroots backend, renderer, scene graph and every
//! protocol global, then wires their signals to the handlers in the other
//! modules and blocks in the event loop. Everything else — tiling policy,
//! per-protocol lifecycle handling — lives in its own module; this file is
//! just the setup and wiring.

// The bindgen output is C-shaped; silence Rust's naming lints for that module.
mod wlr {
    #![allow(
        non_upper_case_globals,
        non_camel_case_types,
        non_snake_case,
        dead_code
    )]
    include!(concat!(env!("OUT_DIR"), "/wlr_bindings.rs"));
}

mod config;
mod control;
mod decoration;
mod ffi;
mod input;
mod keybindings;
mod layer_shell;
mod layout;
mod output;
mod output_power;
mod session_lock;
mod state;
mod tiling;
mod toplevel;
mod wallpaper;

use config::Config;
use decoration::handle_new_decoration;
use ffi::*;
use input::{
    handle_click_focus, handle_double_tap, handle_gesture, handle_grab_button, handle_grab_motion,
    handle_new_input,
};
use keybindings::handle_text_input_visibility;
use layer_shell::handle_new_layer_surface;
use output::{handle_new_output, handle_session_active};
use state::{GrabMode, Server, Workspace, WORKSPACE_COUNT};
use std::env;
use std::ffi::CStr;
use std::os::raw::c_void;
use std::process::Command;
use std::ptr;
use toplevel::handle_new_toplevel;

fn main() {
    unsafe {
        oxide_log_init();

        // The display owns the event loop and (later) the client socket.
        let display = wlr::wl_display_create();
        let event_loop = wlr::wl_display_get_event_loop(display);

        // Quit gracefully on Ctrl-C / SIGTERM (via the loop's signalfd).
        oxide_setup_signals(event_loop, display);

        // Autocreate picks a backend from the environment: a nested Wayland
        // window when we're inside a session, or DRM/KMS on a bare TTY. On DRM it
        // also sets up a login session (libseat); we capture it for VT switching.
        // It stays NULL for the nested backend, which has no session.
        let mut session: *mut wlr::wlr_session = ptr::null_mut();
        let backend = wlr::wlr_backend_autocreate(event_loop, &mut session);
        assert!(!backend.is_null(), "failed to create wlr_backend");

        let renderer = wlr::wlr_renderer_autocreate(backend);
        assert!(!renderer.is_null(), "failed to create wlr_renderer");

        let allocator = wlr::wlr_allocator_autocreate(backend, renderer);
        assert!(!allocator.is_null(), "failed to create wlr_allocator");

        // Compositor-owned GLES2 program for corner-radius masking (wlroots'
        // scene/render-pass API has no such primitive — see
        // shim/gles2_corner.c). NULL on failure (e.g. a non-GLES2 renderer,
        // or a driver shader-compile quirk) — corner_radius then silently
        // has no effect rather than crashing the compositor.
        let corner_program = oxide_gles2_corner_program_create(renderer);
        if corner_program.is_null() {
            eprintln!("0xin: corner-radius GLES2 program unavailable — corner_radius will have no effect");
        }

        // Buffer-factory globals: wl_shm + linux-dmabuf. Clients need these to
        // hand us pixel buffers; without them no app can show anything.
        wlr::wlr_renderer_init_wl_display(renderer, display);

        // Core client-facing globals: surfaces/regions, subsurfaces, clipboard.
        wlr::wlr_compositor_create(display, 6, renderer);
        wlr::wlr_subcompositor_create(display);
        wlr::wlr_data_device_manager_create(display);
        // Fractional-scale clients use a viewport to submit integer-sized
        // buffers for a non-integer logical output scale (2.4 on the FP5).
        // The scene graph applies viewport source/destination state and sends
        // each visible surface its preferred fractional scale.
        wlr::wlr_viewporter_create(display);
        wlr::wlr_fractional_scale_manager_v1_create(display, 1);

        // Create the seat (wl_seat global). We wire input devices into it below.
        let seat = oxide_seat_create(display, c"seat0".as_ptr());
        // Manual on-screen keyboards (e.g. wvkbd) create a virtual keyboard
        // for this seat; the shim forwards all of its keys to seat focus.
        oxide_virtual_keyboard_setup(display, seat);

        // The scene graph holds everything that gets drawn; the output layout
        // arranges outputs in space. Attaching them lets the scene keep each
        // scene-output positioned to match its layout slot.
        let scene = wlr::wlr_scene_create();
        let output_layout = wlr::wlr_output_layout_create(display);
        let scene_layout = wlr::wlr_scene_attach_output_layout(scene, output_layout);

        // Ordered z-layers for the scene: each is a direct child of the scene
        // root, and creation order is paint order (later = on top). Layer-shell
        // surfaces (bars, panels, wallpaper) slot in around our own content.
        let tree_bg_fallback = oxide_scene_add_layer_tree(scene);
        let tree_layer_bg = oxide_scene_add_layer_tree(scene);
        let tree_layer_bottom = oxide_scene_add_layer_tree(scene);
        let tree_normal = oxide_scene_add_layer_tree(scene);
        // Floating windows paint over tiled ones but under bars (layer top).
        let tree_floating = oxide_scene_add_layer_tree(scene);
        let tree_layer_top = oxide_scene_add_layer_tree(scene);
        // Fullscreen windows paint over bars (layer top) but under overlay.
        let tree_fullscreen = oxide_scene_add_layer_tree(scene);
        let tree_layer_overlay = oxide_scene_add_layer_tree(scene);
        // Always last: compositor fallback + ext-session-lock surfaces must
        // cover applications and every shell layer while the session is locked.
        let tree_session_lock = oxide_scene_add_layer_tree(scene);

        // Cursor over the layout; the shim routes its events through scene
        // hit-testing to the seat. Pointer devices get attached in new_input.
        let cursor = oxide_cursor_setup(output_layout, scene, seat);

        // Load user config (modifier, gap, background, keybindings). Falls back
        // to built-in defaults; `OXIN_MOD=alt` overrides the modifier for
        // nested dev (a nesting host like Hyprland grabs Super-chords before us).
        let config = Config::load();
        let first_split_vertical = config.first_split_vertical;

        // `server` lives for the whole of main(), which blocks in wl_display_run
        // below, so the pointer we hand the shim stays valid for the run.
        let mut server = Server {
            display,
            session,
            scene,
            output_layout,
            scene_layout,
            seat,
            cursor,
            renderer,
            allocator,
            corner_program,
            tree_bg_fallback,
            tree_layer_bg,
            tree_layer_bottom,
            tree_normal,
            tree_floating,
            tree_layer_top,
            tree_fullscreen,
            tree_layer_overlay,
            tree_session_lock,
            layers: Vec::new(),
            lock_surfaces: Vec::new(),
            locked: false,
            active_lock: std::ptr::null_mut(),
            lock_new_surface_listener: std::ptr::null_mut(),
            lock_unlock_listener: std::ptr::null_mut(),
            lock_destroy_listener: std::ptr::null_mut(),
            workspaces: (0..WORKSPACE_COUNT)
                .map(|_| Workspace {
                    windows: Vec::new(),
                    focused: 0,
                    tree: None,
                    first_split_vertical,
                    solo: None,
                })
                .collect(),
            outputs: Vec::new(),
            config,
            event_loop,
            hold_source: std::ptr::null_mut(),
            held_keysym: 0,
            held_modifiers: 0,
            held_action: None,
            control_listener: None,
            control_path: None,
            keyboard_visible: false,
            grab: GrabMode::None,
            grab_tl: std::ptr::null_mut(),
            grab_cx: 0.0,
            grab_cy: 0.0,
            grab_x: 0,
            grab_y: 0,
            grab_w: 0,
            grab_h: 0,
        };
        let server_ptr = &mut server as *mut Server as *mut c_void;
        // Focused clients use the standard text-input-v3 protocol to request
        // whichever OSK the profile configured (wvkbd today, replaceable later).
        oxide_text_input_setup(display, seat, handle_text_input_visibility, server_ptr);
        oxide_backend_add_new_output(backend, handle_new_output, server_ptr);
        oxide_backend_add_new_input(backend, handle_new_input, server_ptr);
        // Keep Rust's focused-window bookkeeping in sync with click-to-focus.
        oxide_cursor_set_focus_callback(cursor, handle_click_focus, server_ptr);
        // Touch double-tap on a window: solo it (or whatever `double-tap =`
        // is configured to) — see src/input.rs::handle_double_tap.
        oxide_cursor_set_double_tap_callback(cursor, handle_double_tap, server_ptr);
        // Mod+drag move/resize of floating windows (pointer grabs).
        oxide_cursor_set_grab_callbacks(cursor, handle_grab_button, handle_grab_motion, server_ptr);
        oxide_cursor_set_gestures(
            cursor,
            output_layout,
            server.config.gesture_mask(),
            server.config.virtual_keyboard_height,
            event_loop,
            handle_gesture,
            server_ptr,
        );
        // Repaint outputs when we regain the VT (no-op when nested / no session).
        oxide_session_add_active(session, handle_session_active, server_ptr);

        // ext-session-lock-v1: a locker receives exclusive input and renders
        // above the compositor-owned opaque fallback on every output.
        session_lock::setup(display, server_ptr);

        // xdg-shell: the xdg_wm_base global apps bind to create windows. We hook
        // its new_toplevel signal so each app window enters our scene graph.
        let xdg_shell = wlr::wlr_xdg_shell_create(display, 6);
        oxide_xdg_shell_add_new_toplevel(xdg_shell, handle_new_toplevel, server_ptr);
        oxide_xdg_shell_setup_popups(xdg_shell);

        // xdg-decoration: force server-side mode on every toplevel so clients
        // skip drawing their own CSD title bar. We draw nothing in its place.
        let decoration_manager = wlr::wlr_xdg_decoration_manager_v1_create(display);
        oxide_xdg_decoration_manager_add_new_toplevel_decoration(
            decoration_manager,
            handle_new_decoration,
            server_ptr,
        );

        // wlr-layer-shell-unstable-v1: the global bars/panels/wallpaper (e.g.
        // quickshell, hyprpaper) bind to place themselves in a z-layer above
        // or below our app windows. Version 5 adds set_exclusive_edge; we
        // don't act on it (arrange_layers treats exclusive zones uniformly),
        // but wlroots handles that request at the wire level regardless, and
        // some clients (hyprpaper) refuse to bind below v5.
        let layer_shell = wlr::wlr_layer_shell_v1_create(display, 5);
        oxide_layer_shell_add_new_surface(layer_shell, handle_new_layer_surface, server_ptr);

        // wlr-output-power-management-unstable-v1: lets a client (e.g.
        // patin-lock) request a real DPMS on/off per output, distinct from
        // the opaque lock-fallback cover. wlroots handles the wire protocol;
        // we just apply the on/off it reports (src/output_power.rs).
        output_power::setup(display, server_ptr);

        // wlr-screencopy-unstable-v1: lets clients (grim, wf-recorder) capture
        // our own composited output. wlroots does all the work internally
        // once the global exists — no signals to hook on our side.
        wlr::wlr_screencopy_manager_v1_create(display);

        // xdg-output: without this, screenshot tools can't learn each
        // output's logical position/size (grim fails with a 0x0 capture) —
        // wlroots tracks it automatically from our existing output_layout.
        wlr::wlr_xdg_output_manager_v1_create(display, output_layout);

        // Open the Unix socket clients connect through (e.g. "wayland-2").
        let socket_ptr = wlr::wl_display_add_socket_auto(display);
        assert!(!socket_ptr.is_null(), "failed to open a Wayland socket");
        let socket = CStr::from_ptr(socket_ptr).to_str().unwrap().to_owned();

        assert!(wlr::wlr_backend_start(backend), "failed to start backend");
        eprintln!("0xin: socket ready — WAYLAND_DISPLAY={socket}");

        // Clients we spawn should talk to *us*, not the host compositor. (Our
        // own backend already connected to the host before this point.)
        env::set_var("WAYLAND_DISPLAY", &socket);

        if let Err(error) = control::setup(&mut server, event_loop, server_ptr) {
            eprintln!("0xin: control socket disabled: {error}");
        }

        // Declarative session startup. Commands run separately through the
        // same shell/client-environment path as configured key and gesture
        // actions, in the order they appear in 0xin.conf.
        for command in server.config.exec_once.clone() {
            println!("0xin: exec_once `{command}`");
            keybindings::spawn(&command);
        }

        // `cargo nested -- <cmd> [args…]` auto-spawns a test client against us.
        let mut args = env::args().skip(1);
        if let Some(program) = args.next() {
            let mut command = Command::new(&program);
            command.args(args);
            keybindings::reset_signals(&mut command);
            match command.spawn() {
                Ok(_) => println!("0xin: spawned client `{program}`"),
                Err(e) => eprintln!("0xin: failed to spawn `{program}`: {e}"),
            }
        }

        eprintln!("0xin: entering event loop (Ctrl-C to quit)");
        wlr::wl_display_run(display);

        // Disconnect clients cleanly (this fires our per-window destroy
        // handlers). We intentionally skip wl_display_destroy: tearing down
        // wlroots globals trips internal asserts about global listeners we
        // don't unregister, and the OS reclaims everything on process exit.
        wlr::wl_display_destroy_clients(display);
        control::cleanup(&server);
        eprintln!("0xin: shut down");
    }
}
