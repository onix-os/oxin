//! Config data types: the vocabulary `0xin.conf` lines parse into.

/// Modifier bits (mirror the WLR_MODIFIER_* enum).
pub const MOD_SHIFT: u32 = 1 << 0; // Shift
pub const MOD_CTRL: u32 = 1 << 2; // Control
pub const MOD_ALT: u32 = 1 << 3; // Alt
pub const MOD_LOGO: u32 = 1 << 6; // Super / Logo

/// The modifier bits we consider when matching binds. Excludes Caps Lock (1<<1)
/// and Num Lock (Mod2, 1<<4) so they never break a binding.
pub const MOD_MASK: u32 = MOD_SHIFT | MOD_CTRL | MOD_ALT | MOD_LOGO;

/// A screen-relative direction, for directional focus/move (`Mod+hjkl`).
#[derive(Clone, Copy)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

/// What a keybinding does when triggered.
#[derive(Clone)]
pub enum Action {
    Spawn(String),
    Close,
    Quit,
    FocusNext,
    FocusPrev,
    /// Focus whichever window is spatially adjacent in this direction.
    MoveFocus(Direction),
    /// Swap the focused window's tiling position with its spatial neighbor.
    MoveWindow(Direction),
    /// Resize the focused tiled window along its nearest matching-axis split
    /// (vertical for Left/Right, horizontal for Up/Down): Right/Down grow it
    /// or shrink it depending on which side of that split it's on — the
    /// opposite direction always undoes it. No-op if the focused window is
    /// floating/fullscreen (it isn't in the split tree at all).
    ResizeWindow(Direction),
    /// Toggle the focused window fullscreen (full output box, above bars).
    Fullscreen,
    /// Toggle the focused tiled window as the sole visible window on its
    /// workspace: others hide, it fills the usable area, and the split tree
    /// is untouched — toggling off restores the exact prior layout.
    ToggleSolo,
    /// Toggle the focused window between tiled and floating.
    ToggleFloating,
    /// Switch to workspace (0-based index).
    Workspace(usize),
    /// Move the focused window to a workspace (0-based index).
    MoveToWorkspace(usize),
    MoveToWorkspaceNext,
    MoveToWorkspacePrevious,
    WorkspaceNext,
    WorkspacePrevious,
    KeyboardShow,
    KeyboardHide,
    KeyboardToggle,
}

/// One key combination mapped to an action.
#[derive(Clone)]
pub struct Bind {
    pub mods: u32,
    pub keysym: u32,
    pub action: Action,
}

#[derive(Clone)]
pub struct HoldBind {
    pub mods: u32,
    pub keysym: u32,
    pub duration_ms: i32,
    pub action: Action,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum GestureTrigger {
    BottomUp = 0,
    BottomDown = 1,
    EdgeLeftIn = 2,
    EdgeRightIn = 3,
    TopRight = 4,
    TopLeft = 5,
    TopDown = 6,
    ToTop = 7,
    TwoUp = 8,
    TwoDown = 9,
    TwoLeft = 10,
    TwoRight = 11,
    ThreeUp = 12,
    ThreeDown = 13,
    ThreeLeft = 14,
    ThreeRight = 15,
    DoubleTap = 16,
    EdgeLeftUp = 17,
    EdgeLeftDown = 18,
    /// A single finger starting anywhere ordinary (not already claimed by
    /// another edge zone) that travels far enough sideways to reach close to
    /// a physical edge — browser-style back/forward, distinct from
    /// EdgeLeftIn/EdgeRightIn which only fire for touches starting at the
    /// edge. See to_edge_candidate in shim/input.c.
    ToLeft = 19,
    ToRight = 20,
    /// Right-edge counterpart to EdgeLeftUp/EdgeLeftDown — same 28px-strip,
    /// stepped vertical swipe, just on the other side and for a different
    /// purpose (workspace switching rather than volume).
    EdgeRightUp = 21,
    EdgeRightDown = 22,
}

#[derive(Clone)]
pub struct GestureBind {
    pub trigger: GestureTrigger,
    pub action: Action,
}

/// An explicit position + scale for one named output (connector name, e.g.
/// `HDMI-A-1`). An output with no matching entry keeps the default
/// `wlr_output_layout_add_auto` placement — this is opt-in per monitor.
#[derive(Clone)]
pub struct MonitorConfig {
    pub name: String,
    pub x: i32,
    pub y: i32,
    pub scale: f32,
}

/// Parsed compositor configuration.
pub struct Config {
    /// The primary modifier (`$mod` / `MOD` in binds); Super by default.
    pub modifier: u32,
    /// Gap between/around tiled windows, in pixels.
    pub gap: i32,
    /// Axis of the first dwindle split. Desktop defaults to vertical
    /// (left/right); portrait profiles can choose horizontal (top/bottom).
    pub first_split_vertical: bool,
    /// Background color of empty workspace area (r, g, b in 0..1).
    pub background: (f32, f32, f32),
    /// Optional PNG/JPEG wallpaper path. The solid color remains the fallback.
    pub wallpaper: Option<String>,
    /// Opacity applied to application toplevel buffers (1.0 = fully opaque).
    pub window_opacity: f32,
    /// Corner radius applied to tiled/floating application windows, in
    /// logical pixels (0 = disabled, the default — no masking cost).
    pub corner_radius: i32,
    pub binds: Vec<Bind>,
    pub hold_binds: Vec<HoldBind>,
    /// Shell commands launched once, in declaration order, after the Wayland
    /// socket is ready on each compositor start.
    pub exec_once: Vec<String>,
    /// Per-output explicit position/scale (`monitor =` lines); empty means
    /// every output uses auto-placement.
    pub monitors: Vec<MonitorConfig>,
    /// App ids that always float (`float = <app_id>` lines), matched
    /// case-insensitively and exactly against each new window's app id.
    pub float_rules: Vec<String>,
    /// Default floating window size (`float_size = W x H`), as percentages
    /// of the output's usable area. Applies to the manual float toggle and
    /// to `float =` rule windows; dialogs and fixed-size windows keep their
    /// natural size instead.
    pub float_size: (i32, i32),
    pub gestures: Vec<GestureBind>,
    /// Commands implementing the optional virtual-keyboard controller.
    pub virtual_keyboard_show: Option<String>,
    pub virtual_keyboard_hide: Option<String>,
    /// Logical height used to place the visible keyboard's close handle.
    pub virtual_keyboard_height: i32,
    /// Whether bottom keyboard gestures get a visible compositor-owned pill.
    /// The touch target remains active when this visual hint is disabled.
    pub gesture_handle_visible: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            modifier: MOD_LOGO,
            gap: 2,
            first_split_vertical: true,
            background: (0.0, 0.6, 0.6),
            wallpaper: None,
            window_opacity: 1.0,
            corner_radius: 0,
            binds: Vec::new(),
            hold_binds: Vec::new(),
            exec_once: Vec::new(),
            monitors: Vec::new(),
            float_rules: Vec::new(),
            float_size: (60, 60),
            gestures: Vec::new(),
            virtual_keyboard_show: None,
            virtual_keyboard_hide: None,
            virtual_keyboard_height: 300,
            gesture_handle_visible: true,
        }
    }
}
