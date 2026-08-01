//! Protocol handlers.
//!
//! Smithay dispatches every Wayland request through these traits; each one is
//! the seam where a protocol event turns into 0xin policy (tiling, focus,
//! workspaces). Everything that is pure plumbing — surface trees, configure
//! serials, the wire itself — stays inside Smithay.

use smithay::backend::renderer::utils::{on_commit_buffer_handler, with_renderer_surface_state};
use smithay::desktop::{layer_map_for_output, LayerSurface, PopupKind, Window};
use smithay::input::{Seat, SeatHandler, SeatState};
use smithay::output::Output;
use smithay::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode as DecorationMode;
use smithay::reexports::wayland_server::protocol::wl_buffer::WlBuffer;
use smithay::reexports::wayland_server::protocol::wl_output::WlOutput;
use smithay::reexports::wayland_server::protocol::wl_seat::WlSeat;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::Client;
use smithay::utils::Serial;
use smithay::wayland::buffer::BufferHandler;
use smithay::wayland::compositor::{
    get_parent, is_sync_subsurface, with_states, CompositorClientState, CompositorHandler,
    CompositorState,
};
use smithay::wayland::dmabuf::{DmabufGlobal, DmabufHandler, DmabufState, ImportNotifier};
use smithay::wayland::selection::data_device::{
    ClientDndGrabHandler, DataDeviceHandler, DataDeviceState, ServerDndGrabHandler,
};
use smithay::wayland::selection::primary_selection::{
    PrimarySelectionHandler, PrimarySelectionState,
};
use smithay::wayland::selection::SelectionHandler;
use smithay::wayland::session_lock::{
    LockSurface, SessionLockHandler, SessionLockManagerState, SessionLocker,
};
use smithay::wayland::shell::wlr_layer::{
    Layer, LayerSurface as WlrLayerSurface, LayerSurfaceData, WlrLayerShellHandler,
    WlrLayerShellState,
};
use smithay::wayland::shell::xdg::decoration::XdgDecorationHandler;
use smithay::wayland::shell::xdg::{
    PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState,
};
use smithay::wayland::shm::{ShmHandler, ShmState};
use smithay::{
    delegate_compositor, delegate_data_device, delegate_dmabuf, delegate_fractional_scale,
    delegate_layer_shell, delegate_output, delegate_primary_selection, delegate_seat,
    delegate_session_lock, delegate_shm, delegate_viewporter, delegate_virtual_keyboard_manager,
    delegate_xdg_decoration, delegate_xdg_shell,
};

use crate::state::{ClientState, LockState, Oxin};
use crate::tiling::{active_output, arrange_layers, refresh};
use crate::toplevel::{initial_configure_size, map_window, set_fullscreen, unmap_window};

impl CompositorHandler for Oxin {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }

    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        &client.get_data::<ClientState>().unwrap().compositor_state
    }

    fn commit(&mut self, surface: &WlSurface) {
        on_commit_buffer_handler::<Self>(surface);

        if !is_sync_subsurface(surface) {
            let mut root = surface.clone();
            while let Some(parent) = get_parent(&root) {
                root = parent;
            }
            if let Some(window) = self.window_for_surface(&root) {
                window.on_commit();
            }
        }

        self.popups.commit(surface);
        toplevel_commit(self, surface);
        layer_commit(self, surface);
        lock_surface_commit(self, surface);
    }
}

/// Drive a toplevel through its first configure and its map/unmap edges.
fn toplevel_commit(state: &mut Oxin, surface: &WlSurface) {
    // A brand new window lives in `pending_windows` until it has both been
    // configured and attached a buffer — mapping it any earlier would put a
    // sizeless, contentless window into the split tree.
    if let Some(index) = state
        .pending_windows
        .iter()
        .position(|window| crate::toplevel::is_toplevel_surface(window, surface))
    {
        let window = state.pending_windows[index].clone();
        let Some(toplevel) = window.toplevel().cloned() else {
            return;
        };
        if !toplevel.is_initial_configure_sent() {
            let size = initial_configure_size(state, &window);
            toplevel.with_pending_state(|pending| {
                pending.size = (size.w > 0 && size.h > 0).then_some(size);
            });
            toplevel.send_configure();
            return;
        }
        if has_buffer(surface) {
            state.pending_windows.remove(index);
            map_window(state, &window);
        }
        return;
    }

    // A mapped window that drops its buffer is unmapped (hidden), and may map
    // again later — put it back on the pending list so the map path re-runs.
    if let Some(window) = state.window_for_surface(surface) {
        if !has_buffer(surface) {
            unmap_window(state, &window);
            state.pending_windows.push(window);
        }
    }
}

fn has_buffer(surface: &WlSurface) -> bool {
    with_renderer_surface_state(surface, |surface_state| surface_state.buffer().is_some())
        .unwrap_or(false)
}

/// Layer surfaces (bars, panels, wallpaper clients) need their first configure
/// too, and every commit can change an exclusive zone — which changes where
/// app windows may tile.
fn layer_commit(state: &mut Oxin, surface: &WlSurface) {
    let Some(output) = state
        .outputs
        .iter()
        .map(|entry| entry.output.clone())
        .find(|output| {
            layer_map_for_output(output)
                .layer_for_surface(surface, smithay::desktop::WindowSurfaceType::TOPLEVEL)
                .is_some()
        })
    else {
        return;
    };

    let initial_configure_sent = with_states(surface, |states| {
        states
            .data_map
            .get::<LayerSurfaceData>()
            .map(|data| data.lock().unwrap().initial_configure_sent)
            .unwrap_or(false)
    });

    {
        let mut map = layer_map_for_output(&output);
        map.arrange();
        if !initial_configure_sent {
            if let Some(layer) = map.layer_for_surface(surface, smithay::desktop::WindowSurfaceType::TOPLEVEL) {
                layer.layer_surface().send_configure();
            }
        }
    }

    arrange_layers(state, &output);
    refresh(state);
}

/// The lock client's per-output surface: answer its first configure with the
/// output size, so it can cover the screen.
fn lock_surface_commit(state: &mut Oxin, surface: &WlSurface) {
    let Some(entry) = state.outputs.iter().find(|entry| {
        entry
            .lock_surface
            .as_ref()
            .map(|lock| lock.wl_surface() == surface)
            .unwrap_or(false)
    }) else {
        return;
    };
    let size = entry.geometry.size;
    if let Some(lock) = &entry.lock_surface {
        lock.with_pending_state(|pending| {
            pending.size = Some((size.w as u32, size.h as u32).into());
        });
        lock.send_configure();
    }
}

impl BufferHandler for Oxin {
    fn buffer_destroyed(&mut self, _buffer: &WlBuffer) {}
}

impl ShmHandler for Oxin {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
    }
}

impl XdgShellHandler for Oxin {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        // Tracked, not mapped: the first configure (with the size this window
        // will actually get) goes out from the commit handler, and the window
        // joins a workspace once it has content.
        let window = Window::new_wayland_window(surface);
        self.pending_windows.push(window);
    }

    fn new_popup(&mut self, surface: PopupSurface, _positioner: PositionerState) {
        if let Err(error) = self.popups.track_popup(PopupKind::Xdg(surface)) {
            eprintln!("0xin: cannot track popup: {error}");
        }
    }

    fn grab(&mut self, _surface: PopupSurface, _seat: WlSeat, _serial: Serial) {}

    fn reposition_request(
        &mut self,
        surface: PopupSurface,
        positioner: PositionerState,
        token: u32,
    ) {
        surface.with_pending_state(|pending| {
            pending.geometry = positioner.get_geometry();
            pending.positioner = positioner;
        });
        surface.send_repositioned(token);
    }

    fn fullscreen_request(&mut self, surface: ToplevelSurface, _output: Option<WlOutput>) {
        if let Some(window) = self.window_for_surface(surface.wl_surface()) {
            set_fullscreen(self, &window, true);
        }
    }

    fn unfullscreen_request(&mut self, surface: ToplevelSurface) {
        if let Some(window) = self.window_for_surface(surface.wl_surface()) {
            set_fullscreen(self, &window, false);
        }
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        if let Some(window) = self.window_for_surface(surface.wl_surface()) {
            unmap_window(self, &window);
        }
        self.pending_windows
            .retain(|window| !crate::toplevel::is_toplevel_surface(window, surface.wl_surface()));
    }
}

impl XdgDecorationHandler for Oxin {
    fn new_decoration(&mut self, toplevel: ToplevelSurface) {
        // Server-side on every toplevel, so clients skip drawing their own CSD
        // title bar. We draw nothing in its place.
        toplevel.with_pending_state(|pending| {
            pending.decoration_mode = Some(DecorationMode::ServerSide);
        });
        toplevel.send_configure();
    }

    fn request_mode(&mut self, toplevel: ToplevelSurface, _mode: DecorationMode) {
        toplevel.with_pending_state(|pending| {
            pending.decoration_mode = Some(DecorationMode::ServerSide);
        });
        toplevel.send_pending_configure();
    }

    fn unset_mode(&mut self, toplevel: ToplevelSurface) {
        toplevel.with_pending_state(|pending| {
            pending.decoration_mode = Some(DecorationMode::ServerSide);
        });
        toplevel.send_pending_configure();
    }
}

impl WlrLayerShellHandler for Oxin {
    fn shell_state(&mut self) -> &mut WlrLayerShellState {
        &mut self.layer_shell_state
    }

    fn new_layer_surface(
        &mut self,
        surface: WlrLayerSurface,
        output: Option<WlOutput>,
        _layer: Layer,
        namespace: String,
    ) {
        // A layer surface that names no output goes to the active one — the
        // same rule the wlroots build used for bars that arrive before any
        // output exists.
        let output = output
            .as_ref()
            .and_then(Output::from_resource)
            .or_else(|| {
                self.outputs
                    .get(active_output(self))
                    .map(|entry| entry.output.clone())
            });
        let Some(output) = output else {
            eprintln!("0xin: layer surface `{namespace}` arrived before any output");
            return;
        };
        let layer = LayerSurface::new(surface, namespace);
        layer_map_for_output(&output).map_layer(&layer).ok();
        arrange_layers(self, &output);
        refresh(self);
    }

    fn layer_destroyed(&mut self, surface: WlrLayerSurface) {
        let outputs: Vec<Output> = self
            .outputs
            .iter()
            .map(|entry| entry.output.clone())
            .collect();
        for output in outputs {
            let removed = {
                let mut map = layer_map_for_output(&output);
                let layer = map
                    .layers()
                    .find(|layer| layer.layer_surface() == &surface)
                    .cloned();
                if let Some(layer) = layer {
                    map.unmap_layer(&layer);
                    true
                } else {
                    false
                }
            };
            if removed {
                arrange_layers(self, &output);
                refresh(self);
            }
        }
    }
}

impl SeatHandler for Oxin {
    type KeyboardFocus = WlSurface;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<Oxin> {
        &mut self.seat_state
    }

    fn focus_changed(&mut self, _seat: &Seat<Self>, _focused: Option<&WlSurface>) {}

    fn cursor_image(
        &mut self,
        _seat: &Seat<Self>,
        _image: smithay::input::pointer::CursorImageStatus,
    ) {
    }
}

impl SelectionHandler for Oxin {
    type SelectionUserData = ();
}

impl DataDeviceHandler for Oxin {
    fn data_device_state(&self) -> &DataDeviceState {
        &self.data_device_state
    }
}

impl ClientDndGrabHandler for Oxin {}
impl ServerDndGrabHandler for Oxin {}

impl PrimarySelectionHandler for Oxin {
    fn primary_selection_state(&self) -> &PrimarySelectionState {
        &self.primary_selection_state
    }
}

impl smithay::wayland::output::OutputHandler for Oxin {}

impl smithay::wayland::fractional_scale::FractionalScaleHandler for Oxin {}

impl DmabufHandler for Oxin {
    fn dmabuf_state(&mut self) -> &mut DmabufState {
        &mut self.dmabuf_state
    }

    fn dmabuf_imported(
        &mut self,
        _global: &DmabufGlobal,
        dmabuf: smithay::backend::allocator::dmabuf::Dmabuf,
        notifier: ImportNotifier,
    ) {
        let imported = self
            .backend
            .as_mut()
            .map(|backend| backend.import_dmabuf(&dmabuf))
            .unwrap_or(false);
        if imported {
            let _ = notifier.successful::<Oxin>();
        } else {
            notifier.failed();
        }
    }
}

impl SessionLockHandler for Oxin {
    fn lock_state(&mut self) -> &mut SessionLockManagerState {
        &mut self.session_lock_state
    }

    fn lock(&mut self, confirmation: SessionLocker) {
        // The compositor-owned opaque cover goes up immediately (see the
        // renderer): the session is visually locked even before the lock
        // client has mapped a surface, and stays locked if it crashes.
        self.locked = true;
        self.lock = Some(LockState {
            locker: Some(confirmation),
        });
        if let Some(lock) = self.lock.as_mut() {
            if let Some(locker) = lock.locker.take() {
                locker.lock();
            }
        }
        eprintln!("0xin: session locked");
    }

    fn unlock(&mut self) {
        self.locked = false;
        self.lock = None;
        for entry in self.outputs.iter_mut() {
            entry.lock_surface = None;
        }
        refresh(self);
        eprintln!("0xin: session unlocked");
    }

    fn new_surface(&mut self, surface: LockSurface, output: WlOutput) {
        let Some(output) = Output::from_resource(&output) else {
            return;
        };
        let size = self
            .output_entry(&output)
            .map(|entry| entry.geometry.size)
            .unwrap_or_default();
        surface.with_pending_state(|pending| {
            pending.size = Some((size.w.max(1) as u32, size.h.max(1) as u32).into());
        });
        surface.send_configure();
        if let Some(entry) = self
            .outputs
            .iter_mut()
            .find(|entry| entry.output == output)
        {
            entry.lock_surface = Some(surface);
        }
    }
}

/// Keyboard focus follows the window under the pointer on click; this keeps
/// `Workspace.focused` in step so close/movefocus/movewindow act on the
/// clicked window, not the last keyboard-focused one.
pub fn focus_clicked_window(state: &mut Oxin, surface: &WlSurface) {
    let mut root = surface.clone();
    while let Some(parent) = get_parent(&root) {
        root = parent;
    }
    let Some(window) = state.window_for_surface(&root) else {
        return;
    };
    let Some(workspace) = state.workspace_of(&window) else {
        return;
    };
    if let Some(index) = state.workspaces[workspace]
        .windows
        .iter()
        .position(|other| other == &window)
    {
        state.workspaces[workspace].focused = index;
    }
    crate::keybindings::focus_window(state, &window);
}

// Protocol dispatch: each macro wires the generated Wayland types to the
// handler impls above.
delegate_compositor!(Oxin);
delegate_shm!(Oxin);
delegate_seat!(Oxin);
delegate_data_device!(Oxin);
delegate_primary_selection!(Oxin);
delegate_output!(Oxin);
delegate_xdg_shell!(Oxin);
delegate_xdg_decoration!(Oxin);
delegate_layer_shell!(Oxin);
delegate_viewporter!(Oxin);
delegate_fractional_scale!(Oxin);
delegate_session_lock!(Oxin);
delegate_virtual_keyboard_manager!(Oxin);
delegate_dmabuf!(Oxin);
