//! Long-lived compositor state: the structs shared across every module.

use crate::config::Action;
use crate::config::Config;
use crate::ffi::ShimListener;
use crate::layout::Node;
use crate::lua::link::LuaLink;
use crate::wlr;
use std::collections::{BTreeMap, HashMap};
use std::ops::{Index, IndexMut};
use std::os::fd::OwnedFd;
use std::os::raw::c_void;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;

/// Highest workspace number a config may name (1-based). Workspaces are made
/// on demand, so this is only a bound on the number, not on how many exist —
/// the same 32-bit limit Hyprland puts on workspace IDs.
pub(crate) const MAX_WORKSPACE: usize = i32::MAX as usize;

/// What an active pointer grab is doing to the grabbed floating window
/// (Mod+left-drag moves, Mod+right-drag resizes).
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum GrabMode {
    None,
    Move,
    Resize,
}

/// How many frames to force-repaint after an output comes up or the VT resumes.
pub(crate) const REPAINT_FRAMES: u32 = 3;

/// Long-lived compositor state. We hand a pointer to this to the shim as the
/// `userdata` for the new_output callback, so the handler can reach the scene
/// and layout it needs to wire up each output.
pub(crate) struct Server {
    pub(crate) display: *mut wlr::wl_display,
    /// The login session (DRM/libseat); NULL when running nested. Used for VT
    /// switching.
    pub(crate) session: *mut wlr::wlr_session,
    pub(crate) scene: *mut wlr::wlr_scene,
    pub(crate) output_layout: *mut wlr::wlr_output_layout,
    pub(crate) scene_layout: *mut wlr::wlr_scene_output_layout,
    pub(crate) seat: *mut wlr::wlr_seat,
    pub(crate) cursor: *mut wlr::wlr_cursor,
    pub(crate) renderer: *mut wlr::wlr_renderer,
    pub(crate) allocator: *mut wlr::wlr_allocator,
    /// Compositor-owned GLES2 program used to mask window corners (opaque —
    /// see shim/gles2_corner.c); NULL if compilation failed at startup, in
    /// which case corner-radius masking is silently unavailable.
    pub(crate) corner_program: *mut c_void,
    /// Ordered scene trees for z-layering. Creation order is paint order
    /// (later = on top): the config background, then layer-shell background,
    /// bottom, app windows (normal), layer-shell top, then overlay.
    pub(crate) tree_bg_fallback: *mut wlr::wlr_scene_tree,
    pub(crate) tree_layer_bg: *mut wlr::wlr_scene_tree,
    pub(crate) tree_layer_bottom: *mut wlr::wlr_scene_tree,
    pub(crate) tree_normal: *mut wlr::wlr_scene_tree,
    /// Floating windows: above tiled windows but below layer-shell top (bars
    /// stay visible over them). Windows are reparented in and out as they
    /// float/tile.
    pub(crate) tree_floating: *mut wlr::wlr_scene_tree,
    pub(crate) tree_layer_top: *mut wlr::wlr_scene_tree,
    /// Fullscreen windows: above layer-shell top (covers bars) but below
    /// overlay (lock screens stay on top). Windows are reparented in and
    /// out of here as they enter/leave fullscreen.
    pub(crate) tree_fullscreen: *mut wlr::wlr_scene_tree,
    pub(crate) tree_layer_overlay: *mut wlr::wlr_scene_tree,
    pub(crate) tree_session_lock: *mut wlr::wlr_scene_tree,
    /// Layer-shell surfaces (bars, panels, wallpaper) from any output.
    pub(crate) layers: Vec<*mut LayerSurface>,
    pub(crate) lock_surfaces: Vec<*mut LockSurface>,
    pub(crate) locked: bool,
    pub(crate) active_lock: *mut c_void,
    pub(crate) lock_new_surface_listener: *mut ShimListener,
    pub(crate) lock_unlock_listener: *mut ShimListener,
    pub(crate) lock_destroy_listener: *mut ShimListener,
    /// The workspaces that exist (see `Workspaces`); each belongs to one
    /// output and is shown on at most that one.
    pub(crate) workspaces: Workspaces,
    /// The workspace each disconnected output was showing, by connector name,
    /// so plugging the same monitor back in shows it again. An entry goes
    /// when its workspace stops existing (`workspaces::prune`).
    pub(crate) remembered_workspace: HashMap<String, usize>,
    /// Connected outputs (monitors), each displaying one workspace.
    pub(crate) outputs: Vec<Output>,
    /// User configuration: modifier, gap, background, keybindings.
    pub(crate) config: Config,
    /// Pointers to the Lua VM and its registry, both `main` locals. Null when
    /// no Lua config is in use; see `lua::link` for why they live outside
    /// `Server` rather than as fields.
    pub(crate) lua: LuaLink,
    pub(crate) event_loop: *mut wlr::wl_event_loop,
    /// The backend, for `0xinctl headless-output` (adding and removing
    /// outputs on the headless backend, to test monitor hotplug).
    pub(crate) backend: *mut wlr::wlr_backend,
    pub(crate) hold_source: *mut c_void,
    pub(crate) held_keysym: u32,
    pub(crate) held_modifiers: u32,
    pub(crate) held_action: Option<Action>,
    /// Nonblocking local IPC endpoint used by 0xinctl.
    pub(crate) control_listener: Option<UnixListener>,
    pub(crate) control_path: Option<PathBuf>,
    /// Push-event endpoint (`0xinctl events`): one line per state change to
    /// every connected subscriber, as opposed to `control_listener`'s
    /// one-shot request/response.
    pub(crate) event_listener: Option<UnixListener>,
    pub(crate) event_path: Option<PathBuf>,
    pub(crate) event_subscribers: Vec<UnixStream>,
    /// inotify fd watching the config's directories for live reload (Stage
    /// 31); `None` if the watcher could not be set up.
    pub(crate) watch_fd: Option<OwnedFd>,
    /// Pending debounce timer for a reload the watcher saw coming, or null.
    pub(crate) watch_timer: *mut c_void,
    pub(crate) keyboard_visible: bool,
    /// Active pointer grab (Mod+drag on a floating window): what it does,
    /// which window, and the cursor position + window rect when it started —
    /// motion applies deltas against these, not against the previous event.
    pub(crate) grab: GrabMode,
    pub(crate) grab_tl: *mut Toplevel,
    pub(crate) grab_cx: f64,
    pub(crate) grab_cy: f64,
    pub(crate) grab_x: i32,
    pub(crate) grab_y: i32,
    pub(crate) grab_w: i32,
    pub(crate) grab_h: i32,
}

/// One workspace: an independent list of windows, its focused index, and the
/// split tree its tiled (non-floating, non-fullscreen) windows are arranged
/// into. `tree`'s leaves correspond, in order, to `tiling::tiled_windows(self)`
/// — kept in sync by `tiling::tree_track`/`tree_untrack` every time a window
/// starts or stops tiling. `None` iff no window is currently tiled.
pub(crate) struct Workspace {
    /// Connector name of the output this workspace belongs to (Hyprland's
    /// model: a workspace lives on one monitor, shown or not). Names, not
    /// output indices, because an output can go away and come back.
    pub(crate) monitor: String,
    /// The output this workspace was on when that output was unplugged; it
    /// moves back there when the same connector reappears.
    pub(crate) return_to: Option<String>,
    pub(crate) windows: Vec<*mut Toplevel>,
    pub(crate) focused: usize,
    pub(crate) tree: Option<Node>,
    pub(crate) first_split_vertical: bool,
    /// The one window temporarily shown alone on this workspace (others
    /// hidden, not repositioned); `None` for normal tiled display. Never
    /// mutates `tree` — entering/exiting solo is purely a visibility and
    /// placement decision, so exiting restores the exact prior layout with
    /// no explicit restore step.
    pub(crate) solo: Option<*mut Toplevel>,
}

impl Workspace {
    pub(crate) fn new(monitor: &str, first_split_vertical: bool) -> Self {
        Workspace {
            monitor: monitor.to_owned(),
            return_to: None,
            windows: Vec::new(),
            focused: 0,
            tree: None,
            first_split_vertical,
            solo: None,
        }
    }
}

/// Every workspace that exists, keyed by number: 0-based here, 1-based
/// wherever a user sees it. As in Hyprland, a workspace is made the first
/// time something asks for it and dropped once it is empty and off screen
/// (`workspaces::prune`), so the numbers in use can have gaps. Indexing by a
/// number that does not exist is a bug and panics.
#[derive(Default)]
pub(crate) struct Workspaces(BTreeMap<usize, Workspace>);

impl Workspaces {
    pub(crate) fn contains(&self, id: usize) -> bool {
        self.0.contains_key(&id)
    }

    pub(crate) fn get(&self, id: usize) -> Option<&Workspace> {
        self.0.get(&id)
    }

    pub(crate) fn insert(&mut self, id: usize, workspace: Workspace) {
        self.0.insert(id, workspace);
    }

    /// Keep only the workspaces `keep` says yes to, by number.
    pub(crate) fn retain(&mut self, mut keep: impl FnMut(usize, &Workspace) -> bool) {
        self.0.retain(|&id, ws| keep(id, ws));
    }

    /// Numbers in ascending order.
    pub(crate) fn ids(&self) -> impl Iterator<Item = usize> + '_ {
        self.0.keys().copied()
    }

    /// `(number, workspace)` in ascending order.
    pub(crate) fn iter(&self) -> impl Iterator<Item = (usize, &Workspace)> {
        self.0.iter().map(|(&id, ws)| (id, ws))
    }

    pub(crate) fn values(&self) -> impl Iterator<Item = &Workspace> {
        self.0.values()
    }

    pub(crate) fn values_mut(&mut self) -> impl Iterator<Item = &mut Workspace> {
        self.0.values_mut()
    }
}

impl Index<usize> for Workspaces {
    type Output = Workspace;

    fn index(&self, id: usize) -> &Workspace {
        self.0
            .get(&id)
            .unwrap_or_else(|| panic!("workspace {} does not exist", id + 1))
    }
}

impl IndexMut<usize> for Workspaces {
    fn index_mut(&mut self, id: usize) -> &mut Workspace {
        self.0
            .get_mut(&id)
            .unwrap_or_else(|| panic!("workspace {} does not exist", id + 1))
    }
}

/// One connected output (monitor): its box in layout coordinates, the workspace
/// it currently displays, and the resources to release when it's destroyed.
pub(crate) struct Output {
    pub(crate) wlr_output: *mut wlr::wlr_output,
    /// Connector name ("eDP-1", "HDMI-A-1"): what a workspace records as its
    /// output, and how the same monitor is recognised when it comes back.
    pub(crate) name: String,
    pub(crate) x: i32,
    pub(crate) y: i32,
    pub(crate) w: i32,
    pub(crate) h: i32,
    /// Current WL_OUTPUT_TRANSFORM_* value (0 = normal). Set at creation and
    /// by `output::rotate`.
    pub(crate) transform: u32,
    /// Usable area left after layer-shell surfaces reserve their exclusive
    /// zones (e.g. a bar strip). Starts equal to the full box; recomputed by
    /// `arrange_layers`. App windows tile within this, not the full box.
    pub(crate) ux: i32,
    pub(crate) uy: i32,
    pub(crate) uw: i32,
    pub(crate) uh: i32,
    /// The workspace on screen here; it always belongs to this output.
    pub(crate) workspace: usize,
    /// Listeners + background node + frame context to tear down on destroy.
    pub(crate) frame_listener: *mut ShimListener,
    pub(crate) destroy_listener: *mut ShimListener,
    pub(crate) background: *mut c_void,
    /// Image scene-buffer above the solid fallback, or NULL.
    pub(crate) wallpaper: *mut c_void,
    /// Bottom gesture-handle rectangle, or NULL when the profile disables it.
    pub(crate) gesture_handle: *mut c_void,
    /// Opaque compositor-owned cover shown immediately while locked, including
    /// when the lock client has not mapped yet or has crashed.
    pub(crate) lock_fallback: *mut c_void,
    pub(crate) frame_ctx: *mut FrameCtx,
    /// Frames remaining to force a full repaint (after creation/VT resume). A
    /// `frame` event only fires once the output is actually presenting, so doing
    /// the full-output damage here lands *after* the async resume modeset.
    pub(crate) repaint_frames: u32,
}

/// Userdata for an output's `frame` callback: enough to render this output and
/// find its `Output` entry (so handle_frame can honor `repaint_frames`).
pub(crate) struct FrameCtx {
    pub(crate) server: *mut Server,
    pub(crate) scene_output: *mut wlr::wlr_scene_output,
    pub(crate) wlr_output: *mut wlr::wlr_output,
}

/// One application window we track. Heap-allocated; a raw pointer to it is the
/// `userdata` for that window's map/unmap/destroy listeners.
pub(crate) struct Toplevel {
    pub(crate) server: *mut Server,
    pub(crate) xdg_toplevel: *mut wlr::wlr_xdg_toplevel,
    pub(crate) scene_tree: *mut wlr::wlr_scene_tree,
    /// This window's rect as of the last `tiling::refresh()` pass. Not
    /// authoritative (the scene node / xdg_toplevel size are) — just a cache
    /// for directional focus/move to compare windows against each other.
    pub(crate) x: i32,
    pub(crate) y: i32,
    pub(crate) w: i32,
    pub(crate) h: i32,
    /// Whether this window is fullscreen (covers its output's full box,
    /// scene tree parented under `Server.tree_fullscreen`).
    pub(crate) fullscreen: bool,
    /// Whether this window floats (keeps its own size, centered on map,
    /// scene tree parented under `Server.tree_floating`, skipped by the
    /// spiral). Fullscreen wins while both are set.
    pub(crate) floating: bool,
    // Listeners we registered; removed+freed on destroy so wlroots doesn't
    // assert on a non-empty destroy list.
    pub(crate) commit_listener: *mut ShimListener,
    pub(crate) map_listener: *mut ShimListener,
    pub(crate) unmap_listener: *mut ShimListener,
    pub(crate) destroy_listener: *mut ShimListener,
    pub(crate) fullscreen_listener: *mut ShimListener,
    /// Per-window GPU buffer chain for corner-radius masking (opaque —
    /// `struct wlr_swapchain*`, see shim/gles2_corner.c); NULL until the
    /// first masked commit. Recreated when the surface's buffer size
    /// changes, so `corner_swapchain_w`/`_h` track what size it currently
    /// matches.
    pub(crate) corner_swapchain: *mut c_void,
    pub(crate) corner_swapchain_w: i32,
    pub(crate) corner_swapchain_h: i32,
}

/// One layer-shell surface (bar, panel, wallpaper — e.g. quickshell). Heap-
/// allocated; a raw pointer to it is the `userdata` for its commit/map/unmap/
/// destroy listeners. Modeled on `Toplevel`.
pub(crate) struct LayerSurface {
    pub(crate) server: *mut Server,
    pub(crate) wlr_layer_surface: *mut c_void,
    /// The `wlr_scene_layer_surface_v1` scene helper (opaque to Rust).
    pub(crate) scene_ls: *mut c_void,
    pub(crate) wlr_output: *mut wlr::wlr_output,
    pub(crate) commit_listener: *mut ShimListener,
    pub(crate) map_listener: *mut ShimListener,
    pub(crate) unmap_listener: *mut ShimListener,
    pub(crate) destroy_listener: *mut ShimListener,
}

pub(crate) struct LockSurface {
    pub(crate) server: *mut Server,
    pub(crate) lock_surface: *mut c_void,
    pub(crate) map_listener: *mut ShimListener,
    pub(crate) destroy_listener: *mut ShimListener,
}
