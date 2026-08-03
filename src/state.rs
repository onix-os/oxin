//! Long-lived compositor state: the structs shared across every module.
//!
//! Smithay owns the Wayland plumbing (protocol globals, the surface tree, the
//! seat, the desktop `Space`); everything in here is 0xin's own policy state —
//! workspaces, the split trees, which output shows what, and the transient
//! bookkeeping for pointer grabs and hold bindings.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Instant;

use smithay::desktop::{PopupManager, Space, Window};
use smithay::input::{Seat, SeatState};
use smithay::output::Output;
use smithay::reexports::calloop::LoopHandle;
use smithay::reexports::wayland_server::backend::{ClientData, ClientId, DisconnectReason};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::DisplayHandle;
use smithay::utils::{Clock, Logical, Monotonic, Point, Rectangle};
use smithay::wayland::compositor::{CompositorClientState, CompositorState};
use smithay::wayland::dmabuf::DmabufState;
use smithay::wayland::fractional_scale::FractionalScaleManagerState;
use smithay::wayland::output::OutputManagerState;
use smithay::wayland::selection::data_device::DataDeviceState;
use smithay::wayland::selection::primary_selection::PrimarySelectionState;
use smithay::wayland::session_lock::{LockSurface, SessionLockManagerState, SessionLocker};
use smithay::wayland::shell::wlr_layer::WlrLayerShellState;
use smithay::wayland::shell::xdg::decoration::XdgDecorationState;
use smithay::wayland::shell::xdg::XdgShellState;
use smithay::wayland::shm::ShmState;
use smithay::wayland::viewporter::ViewporterState;
use smithay::wayland::virtual_keyboard::VirtualKeyboardManagerState;

use crate::backend::Backend;
use crate::config::{Action, Config};
use crate::gestures::Recognizer;
use crate::layout::Node;
use crate::wallpaper::Wallpaper;

/// Number of virtual workspaces.
pub const WORKSPACE_COUNT: usize = 9;

/// What an active pointer grab is doing to the grabbed floating window
/// (Mod+left-drag moves, Mod+right-drag resizes).
#[derive(Clone, Copy, PartialEq)]
pub enum GrabMode {
    None,
    Move,
    Resize,
}

/// One workspace: an independent list of windows, its focused index, and the
/// split tree its tiled (non-floating, non-fullscreen) windows are arranged
/// into. `tree`'s leaves correspond, in order, to `tiling::tiled_windows(self)`
/// — kept in sync by `tiling::tree_track`/`tree_untrack` every time a window
/// starts or stops tiling. `None` iff no window is currently tiled.
pub struct Workspace {
    pub windows: Vec<Window>,
    pub focused: usize,
    pub tree: Option<Node>,
    pub first_split_vertical: bool,
    /// The one window temporarily shown alone on this workspace (others
    /// hidden, not repositioned); `None` for normal tiled display. Never
    /// mutates `tree` — entering/exiting solo is purely a visibility and
    /// placement decision, so exiting restores the exact prior layout with
    /// no explicit restore step.
    pub solo: Option<Window>,
}

impl Workspace {
    pub fn new(first_split_vertical: bool) -> Self {
        Workspace {
            windows: Vec::new(),
            focused: 0,
            tree: None,
            first_split_vertical,
            solo: None,
        }
    }
}

/// One connected output (monitor): the Smithay output, its box in layout
/// coordinates, the workspace it displays, and its wallpaper.
///
/// `usable` is what is left after layer-shell surfaces reserve their exclusive
/// zones (e.g. a bar strip) — app windows tile within it, not within the full
/// box. Smithay's per-output layer map computes that zone for us; we mirror it
/// here because the tiling code is pure geometry and shouldn't have to reach
/// into the layer map.
pub struct OutputEntry {
    pub output: Output,
    pub geometry: Rectangle<i32, Logical>,
    pub usable: Rectangle<i32, Logical>,
    pub workspace: usize,
    pub wallpaper: Option<Wallpaper>,
    /// The session-lock client's surface for this output, once it has one.
    pub lock_surface: Option<LockSurface>,
}

/// A session lock in progress: the locker handle we must confirm or cancel,
/// plus whether we have already confirmed it.
pub struct LockState {
    pub locker: Option<SessionLocker>,
}

pub struct Oxin {
    pub dh: DisplayHandle,
    pub loop_handle: LoopHandle<'static, Oxin>,
    #[allow(dead_code)] // kept for presentation-time work
    pub clock: Clock<Monotonic>,
    pub start_time: Instant,
    pub running: Arc<AtomicBool>,
    pub socket_name: String,

    // Protocol globals, all owned by Smithay. Several are never read after
    // construction — they exist to *own* their global, which is unregistered
    // when the state is dropped, so they must stay alive for the whole run.
    pub compositor_state: CompositorState,
    pub xdg_shell_state: XdgShellState,
    #[allow(dead_code)] // owns its global
    pub xdg_decoration_state: XdgDecorationState,
    pub layer_shell_state: WlrLayerShellState,
    pub shm_state: ShmState,
    #[allow(dead_code)] // owns its global
    pub output_manager_state: OutputManagerState,
    pub seat_state: SeatState<Oxin>,
    pub data_device_state: DataDeviceState,
    pub primary_selection_state: PrimarySelectionState,
    #[allow(dead_code)] // owns its global
    pub viewporter_state: ViewporterState,
    #[allow(dead_code)] // owns its global
    pub fractional_scale_manager_state: FractionalScaleManagerState,
    pub session_lock_state: SessionLockManagerState,
    #[allow(dead_code)] // owns its global
    pub virtual_keyboard_state: VirtualKeyboardManagerState,
    pub dmabuf_state: DmabufState,
    #[allow(dead_code)] // owns its global
    pub output_power_state: crate::protocols::output_power::OutputPowerManagerState,
    /// Per-output DPMS state, by connector name. Absent means powered on.
    pub powered: std::collections::HashMap<String, bool>,
    #[allow(dead_code)] // owns its global
    pub screencopy_state: crate::protocols::screencopy::ScreencopyManagerState,

    pub seat: Seat<Oxin>,
    /// Every mapped window and output. Placement is ours (see `tiling`); the
    /// space is what turns that into damage tracking and render elements.
    pub space: Space<Window>,
    pub popups: PopupManager,
    /// Windows that exist but are not on a workspace yet: created by
    /// `new_toplevel`, waiting for their first configure to be acknowledged
    /// with an actual buffer. Mapping one earlier would put a sizeless,
    /// contentless window into the split tree.
    pub pending_windows: Vec<Window>,

    /// The backend actually driving pixels (nested winit window, or DRM/KMS).
    /// Taken out of the state for the duration of a render pass, so backend
    /// code can hold `&mut Oxin` while it draws — see `backend::with_backend`.
    pub backend: Option<Backend>,

    // --- 0xin policy state ---
    pub config: Config,
    pub workspaces: Vec<Workspace>,
    pub outputs: Vec<OutputEntry>,
    pub pointer_location: Point<f64, Logical>,

    /// Active pointer grab (Mod+drag on a floating window): what it does,
    /// which window, and the cursor position + window rect when it started —
    /// motion applies deltas against these, not against the previous event.
    pub grab: GrabMode,
    pub grab_window: Option<Window>,
    pub grab_cursor: Point<f64, Logical>,
    pub grab_rect: Rectangle<i32, Logical>,

    /// Hold bindings (`hold = MODS, KEY, MS, ACTION`): the chord being held,
    /// what it will fire, and the timer token that will fire it.
    pub held_keysym: u32,
    pub held_modifiers: u32,
    pub held_action: Option<Action>,
    pub hold_timer: Option<smithay::reexports::calloop::RegistrationToken>,

    /// Where 0xinctl's socket lives, so it can be removed on shutdown. The
    /// listener itself is owned by its calloop source.
    pub control_path: Option<PathBuf>,

    /// The pointer image. Behind a `RefCell` because building its buffer needs
    /// `&mut`, while element collection only has `&Oxin`.
    pub cursor: std::cell::RefCell<crate::cursor::Cursor>,

    pub keyboard_visible: bool,
    /// Touch gesture recognizer state (the phone profile's edge swipes).
    pub gestures: Recognizer,

    /// ext-session-lock-v1: a locker owns all input and paints above
    /// everything while this is set.
    pub locked: bool,
    pub lock: Option<LockState>,
}

impl Oxin {
    /// The window whose root surface is `surface`, if we track one.
    pub fn window_for_surface(&self, surface: &WlSurface) -> Option<Window> {
        self.workspaces
            .iter()
            .flat_map(|ws| ws.windows.iter())
            .find(|window| {
                window
                    .toplevel()
                    .map(|toplevel| toplevel.wl_surface() == surface)
                    .unwrap_or(false)
            })
            .cloned()
    }

    /// Which workspace currently holds `window`, if any.
    pub fn workspace_of(&self, window: &Window) -> Option<usize> {
        self.workspaces
            .iter()
            .position(|ws| ws.windows.contains(window))
    }

    /// The output entry for a Smithay output.
    pub fn output_entry(&self, output: &Output) -> Option<&OutputEntry> {
        self.outputs.iter().find(|entry| &entry.output == output)
    }
}

/// Per-client data. Smithay hands this back to us for every request, and the
/// compositor state inside it is where surface state for that client lives.
#[derive(Default)]
pub struct ClientState {
    pub compositor_state: CompositorClientState,
}

impl ClientData for ClientState {
    fn initialized(&self, _client_id: ClientId) {}
    fn disconnected(&self, _client_id: ClientId, _reason: DisconnectReason) {}
}
