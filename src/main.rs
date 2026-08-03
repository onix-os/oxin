//! 0xin — a dynamic tiling Wayland compositor built on Smithay.
//!
//! `main()` builds the Wayland display, brings up a backend (nested winit
//! window, or DRM/KMS on a bare TTY), creates every protocol global and blocks
//! in the calloop event loop. Everything else — tiling policy, per-protocol
//! lifecycle handling — lives in its own module; this file is just setup and
//! wiring.

mod backend;
mod config;
mod control;
mod corners;
mod cursor;
mod gestures;
mod handlers;
mod input;
mod keybindings;
mod layout;
mod output;
mod protocols;
mod render;
mod state;
mod tiling;
mod toplevel;
mod wallpaper;
mod window;

use std::env;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use smithay::input::keyboard::XkbConfig;
use smithay::input::SeatState;
use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::{EventLoop, Interest, Mode as CalloopMode, PostAction};
use smithay::reexports::wayland_server::Display;
use smithay::utils::Clock;
use smithay::wayland::compositor::CompositorState;
use smithay::wayland::dmabuf::DmabufState;
use smithay::wayland::fractional_scale::FractionalScaleManagerState;
use smithay::wayland::output::OutputManagerState;
use smithay::wayland::selection::data_device::DataDeviceState;
use smithay::wayland::selection::primary_selection::PrimarySelectionState;
use smithay::wayland::session_lock::SessionLockManagerState;
use smithay::wayland::shell::wlr_layer::WlrLayerShellState;
use smithay::wayland::shell::xdg::decoration::XdgDecorationState;
use smithay::wayland::shell::xdg::XdgShellState;
use smithay::wayland::shm::ShmState;
use smithay::wayland::socket::ListeningSocketSource;
use smithay::wayland::viewporter::ViewporterState;
use smithay::wayland::virtual_keyboard::VirtualKeyboardManagerState;

use crate::config::Config;
use crate::gestures::Recognizer;
use crate::state::{ClientState, Oxin, Workspace, WORKSPACE_COUNT};

fn main() {
    if let Ok(filter) = tracing_subscriber::EnvFilter::try_from_default_env() {
        tracing_subscriber::fmt().with_env_filter(filter).init();
    } else {
        tracing_subscriber::fmt().init();
    }

    if let Err(error) = run() {
        eprintln!("0xin: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut event_loop: EventLoop<Oxin> =
        EventLoop::try_new().map_err(|error| format!("cannot create the event loop: {error}"))?;
    let display: Display<Oxin> =
        Display::new().map_err(|error| format!("cannot create the Wayland display: {error}"))?;
    let dh = display.handle();
    let loop_handle = event_loop.handle();

    // Load user config (modifier, gap, background, keybindings). Falls back to
    // built-in defaults; `OXIN_MOD=alt` overrides the modifier for nested dev
    // (a nesting host like Hyprland grabs Super-chords before us).
    let config = Config::load();
    let first_split_vertical = config.first_split_vertical;
    let gesture_mask = config.gesture_mask();
    let keyboard_height = config.virtual_keyboard_height;
    let handle_visible = config.has_keyboard_handle();

    let mut seat_state = SeatState::new();
    let mut seat = seat_state.new_wl_seat(&dh, "seat0");

    let mut state = Oxin {
        dh: dh.clone(),
        loop_handle: loop_handle.clone(),
        clock: Clock::new(),
        start_time: Instant::now(),
        running: Arc::new(AtomicBool::new(true)),
        socket_name: String::new(),

        compositor_state: CompositorState::new::<Oxin>(&dh),
        xdg_shell_state: XdgShellState::new::<Oxin>(&dh),
        xdg_decoration_state: XdgDecorationState::new::<Oxin>(&dh),
        layer_shell_state: WlrLayerShellState::new::<Oxin>(&dh),
        shm_state: ShmState::new::<Oxin>(&dh, Vec::new()),
        // xdg-output as well as wl_output: without it screenshot tools can't
        // learn each output's logical position/size (grim fails with a 0x0
        // capture).
        output_manager_state: OutputManagerState::new_with_xdg_output::<Oxin>(&dh),
        seat_state,
        data_device_state: DataDeviceState::new::<Oxin>(&dh),
        primary_selection_state: PrimarySelectionState::new::<Oxin>(&dh),
        // Fractional-scale clients use a viewport to submit integer-sized
        // buffers for a non-integer logical output scale (2.4 on the FP5).
        viewporter_state: ViewporterState::new::<Oxin>(&dh),
        fractional_scale_manager_state: FractionalScaleManagerState::new::<Oxin>(&dh),
        session_lock_state: SessionLockManagerState::new::<Oxin, _>(&dh, |_| true),
        // Manual on-screen keyboards (e.g. wvkbd) create a virtual keyboard
        // for this seat; its keys reach seat focus like any other keyboard.
        virtual_keyboard_state: VirtualKeyboardManagerState::new::<Oxin, _>(&dh, |_| true),
        dmabuf_state: DmabufState::new(),
        // wlr-output-power-management: Smithay has no such protocol, so the
        // wire handling is ours (src/protocols/output_power.rs).
        output_power_state: crate::protocols::output_power::OutputPowerManagerState::new(&dh),
        powered: std::collections::HashMap::new(),
        // wlr-screencopy: what grim and wf-recorder capture through.
        screencopy_state: crate::protocols::screencopy::ScreencopyManagerState::new(&dh),

        seat: seat.clone(),
        space: smithay::desktop::Space::default(),
        popups: smithay::desktop::PopupManager::default(),
        pending_windows: Vec::new(),
        backend: None,

        config,
        workspaces: (0..WORKSPACE_COUNT)
            .map(|_| Workspace::new(first_split_vertical))
            .collect(),
        outputs: Vec::new(),
        pointer_location: (0.0, 0.0).into(),

        grab: state::GrabMode::None,
        grab_window: None,
        grab_cursor: (0.0, 0.0).into(),
        grab_rect: Default::default(),

        held_keysym: 0,
        held_modifiers: 0,
        held_action: None,
        hold_timer: None,

        control_path: None,

        cursor: std::cell::RefCell::new(crate::cursor::Cursor::load()),
        keyboard_visible: false,
        gestures: Recognizer::new(gesture_mask, keyboard_height, handle_visible),

        locked: false,
        lock: None,
    };

    handlers::advertise_layer_shell_v5(&mut state);

    seat.add_keyboard(XkbConfig::default(), 600, 25)
        .map_err(|error| format!("cannot create the keyboard: {error}"))?;
    seat.add_pointer();
    seat.add_touch();

    // Pick a backend the way `wlr_backend_autocreate` did: a nested window
    // when we are inside a session, DRM/KMS on a bare TTY.
    let nested = env::var_os("WAYLAND_DISPLAY").is_some() || env::var_os("DISPLAY").is_some();
    if nested {
        backend::winit::init(&mut state)?;
    } else {
        backend::udev::init(&mut state)?;
    }

    // Open the Unix socket clients connect through (e.g. "wayland-2").
    let socket = ListeningSocketSource::new_auto()
        .map_err(|error| format!("cannot open a Wayland socket: {error}"))?;
    let socket_name = socket.socket_name().to_string_lossy().into_owned();
    loop_handle
        .insert_source(socket, |stream, _, state: &mut Oxin| {
            if let Err(error) = state
                .dh
                .insert_client(stream, Arc::new(ClientState::default()))
            {
                eprintln!("0xin: cannot accept client: {error}");
            }
        })
        .map_err(|error| format!("cannot register the Wayland socket: {error}"))?;

    loop_handle
        .insert_source(
            Generic::new(display, Interest::READ, CalloopMode::Level),
            |_, display, state: &mut Oxin| {
                // SAFETY: the display is never dropped while the source lives.
                unsafe {
                    display
                        .get_mut()
                        .dispatch_clients(state)
                        .map_err(|error| std::io::Error::other(error))?;
                }
                Ok(PostAction::Continue)
            },
        )
        .map_err(|error| format!("cannot register the display source: {error}"))?;

    state.socket_name = socket_name.clone();
    eprintln!("0xin: socket ready — WAYLAND_DISPLAY={socket_name}");

    // Clients we spawn should talk to *us*, not the host compositor. (Our own
    // backend already connected to the host before this point.)
    env::set_var("WAYLAND_DISPLAY", &socket_name);

    if let Err(error) = control::setup(&mut state) {
        eprintln!("0xin: control socket disabled: {error}");
    }

    // Quit gracefully on Ctrl-C / SIGTERM.
    install_signal_handlers();

    // Declarative session startup. Commands run separately through the same
    // shell/client-environment path as configured key and gesture actions, in
    // the order they appear in 0xin.conf.
    for command in state.config.exec_once.clone() {
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
            Err(error) => eprintln!("0xin: failed to spawn `{program}`: {error}"),
        }
    }

    eprintln!("0xin: entering event loop (Ctrl-C to quit)");
    let running = state.running.clone();
    while running.load(Ordering::SeqCst) && !terminated() {
        if event_loop
            .dispatch(Some(Duration::from_millis(16)), &mut state)
            .is_err()
        {
            break;
        }
        backend::render_pending(&mut state);
        state.space.refresh();
        state.popups.cleanup();
        if let Err(error) = state.dh.flush_clients() {
            eprintln!("0xin: cannot flush clients: {error}");
            break;
        }
    }

    control::cleanup(&state);
    eprintln!("0xin: shut down");
    Ok(())
}

static TERMINATED: AtomicBool = AtomicBool::new(false);

fn terminated() -> bool {
    TERMINATED.load(Ordering::SeqCst)
}

extern "C" fn handle_signal(_signal: libc::c_int) {
    TERMINATED.store(true, Ordering::SeqCst);
}

fn install_signal_handlers() {
    unsafe {
        libc::signal(libc::SIGINT, handle_signal as *const () as libc::sighandler_t);
        libc::signal(libc::SIGTERM, handle_signal as *const () as libc::sighandler_t);
        // Clients are reaped by the kernel; we never wait on them.
        libc::signal(libc::SIGCHLD, libc::SIG_IGN);
        libc::signal(libc::SIGPIPE, libc::SIG_IGN);
    }
}
