//! 0xin config: a tiny, dependency-free parser for `0xin.conf`.
//!
//! Format is line-based `key = value`, `#` starts a comment. Scalars set the
//! modifier, gap and background; `bind = MODS, KEY, ACTION[, ARG]` lines define
//! keybindings (Hyprland-ish syntax). Anything we can't parse is warned about
//! and skipped, so a typo never stops the compositor from starting.
//!
//! Binds always start from the built-in defaults; a config's `bind =` lines
//! override whichever chord (mods+key) they name and leave every other
//! default bind in place — never a wholesale replacement. So a config with
//! just a couple of `bind =` lines still has working workspace switches,
//! etc. If no config file exists at all, the defaults apply unchanged.

use std::env;
use std::ffi::CString;
use std::fs;
use std::os::raw::c_char;
use std::path::PathBuf;

/// Modifier bits (mirror the WLR_MODIFIER_* enum).
pub const MOD_SHIFT: u32 = 1 << 0; // Shift
pub const MOD_CTRL: u32 = 1 << 2; // Control
pub const MOD_ALT: u32 = 1 << 3; // Alt
pub const MOD_LOGO: u32 = 1 << 6; // Super / Logo

/// The modifier bits we consider when matching binds. Excludes Caps Lock (1<<1)
/// and Num Lock (Mod2, 1<<4) so they never break a binding.
pub const MOD_MASK: u32 = MOD_SHIFT | MOD_CTRL | MOD_ALT | MOD_LOGO;

extern "C" {
    fn oxide_keysym_from_name(name: *const c_char) -> u32;
}

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
    /// Toggle the focused window between tiled and floating.
    ToggleFloating,
    /// Switch to workspace (0-based index).
    Workspace(usize),
    /// Move the focused window to a workspace (0-based index).
    MoveToWorkspace(usize),
}

/// One key combination mapped to an action.
#[derive(Clone)]
pub struct Bind {
    pub mods: u32,
    pub keysym: u32,
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
    pub binds: Vec<Bind>,
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
    /// Process name of an on-screen keyboard controlled by the bottom swipe
    /// handle. `None` keeps the phone gesture UI disabled.
    pub gesture_keyboard: Option<String>,
    /// Logical height of that keyboard; used to keep the gesture handle on
    /// its top edge while the keyboard is visible.
    pub gesture_keyboard_height: i32,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            modifier: MOD_LOGO,
            gap: 2,
            first_split_vertical: true,
            background: (0.0, 0.6, 0.6),
            binds: Vec::new(),
            monitors: Vec::new(),
            float_rules: Vec::new(),
            float_size: (60, 60),
            gesture_keyboard: None,
            gesture_keyboard_height: 300,
        }
    }
}

impl Config {
    /// Load config from `$OXIN_CONFIG` (an exact file path, if set — handy
    /// for testing a config from the repo without touching `~/.config`), else
    /// `$XDG_CONFIG_HOME/0xin/0xin.conf`, else `~/.config/0xin/0xin.conf`.
    /// Missing file -> built-in defaults. `OXIN_MOD=alt` overrides the
    /// modifier (for nested dev under Hyprland, which grabs Super-chords
    /// before us).
    pub fn load() -> Config {
        let mut cfg = Config::default();

        let contents = config_path().and_then(|p| fs::read_to_string(&p).ok());
        match &contents {
            Some(text) => {
                println!("0xin: loaded config");
                cfg.parse_scalars(text);
            }
            None => println!("0xin: no config file — using defaults"),
        }

        // Env override wins over the config's modifier line.
        if let Ok("alt") = env::var("OXIN_MOD").as_deref() {
            cfg.modifier = MOD_ALT;
        }

        // Binds always start from the defaults for the final modifier; a
        // config's own `bind =` lines (parsed after the modifier is final,
        // so `MOD` resolves right) override matching chords or add new
        // ones — see the module doc comment.
        cfg.binds = default_binds(cfg.modifier);
        if let Some(text) = &contents {
            cfg.apply_binds(text);
        }

        println!(
            "0xin: modifier = {}, gap = {}, {} bind(s)",
            mod_name(cfg.modifier),
            cfg.gap,
            cfg.binds.len()
        );
        cfg
    }

    /// First pass: scalar settings (everything except `bind`).
    fn parse_scalars(&mut self, text: &str) {
        for (n, raw) in lines(text) {
            let Some((key, val)) = split_kv(raw) else {
                continue;
            };
            match key {
                "modifier" => match parse_mods(val, MOD_LOGO) {
                    Some(m) => self.modifier = m,
                    None => warn(n, "unknown modifier", raw),
                },
                "gap" => match val.parse::<i32>() {
                    Ok(g) if g >= 0 => self.gap = g,
                    _ => warn(n, "invalid gap", raw),
                },
                "first_split" => match val {
                    "vertical" => self.first_split_vertical = true,
                    "horizontal" => self.first_split_vertical = false,
                    _ => warn(
                        n,
                        "invalid first_split (want `vertical` or `horizontal`)",
                        raw,
                    ),
                },
                "background" => match parse_color(val) {
                    Some(c) => self.background = c,
                    None => warn(n, "invalid background (want `r g b`)", raw),
                },
                "monitor" => match parse_monitor(val) {
                    Some(m) => match self.monitors.iter_mut().find(|e| e.name == m.name) {
                        Some(existing) => *existing = m,
                        None => self.monitors.push(m),
                    },
                    None => warn(n, "invalid monitor (want `NAME, XxY[, SCALE]`)", raw),
                },
                "float_size" => match parse_float_size(val) {
                    Some(s) => self.float_size = s,
                    None => warn(n, "invalid float_size (want `W x H` percent, 1-100)", raw),
                },
                "float" => {
                    let app_id = val.to_ascii_lowercase();
                    if app_id.is_empty() {
                        warn(n, "empty float rule (want `float = APP_ID`)", raw);
                    } else if !self.float_rules.contains(&app_id) {
                        self.float_rules.push(app_id);
                    }
                }
                "gesture_keyboard" => {
                    if val.is_empty() || val.len() > 15 || val.contains(char::is_whitespace) {
                        warn(
                            n,
                            "invalid gesture_keyboard (want a Linux process name, max 15 characters)",
                            raw,
                        );
                    } else {
                        self.gesture_keyboard = Some(val.to_string());
                    }
                }
                "gesture_keyboard_height" => match val.parse::<i32>() {
                    Ok(height) if (80..=1000).contains(&height) => {
                        self.gesture_keyboard_height = height
                    }
                    _ => warn(
                        n,
                        "invalid gesture_keyboard_height (want 80..1000 logical pixels)",
                        raw,
                    ),
                },
                "bind" => {} // handled in parse_binds
                _ => warn(n, "unknown setting", raw),
            }
        }
    }

    /// Second pass: `bind = MODS, KEY, ACTION[, ARG]`. Each parsed bind
    /// overrides any existing bind on the same chord (mods+keysym) — from
    /// the defaults or an earlier line in this same file — or is appended
    /// if the chord is new.
    fn apply_binds(&mut self, text: &str) {
        for (n, raw) in lines(text) {
            let Some((key, val)) = split_kv(raw) else {
                continue;
            };
            if key != "bind" {
                continue;
            }
            match self.parse_bind(val) {
                Some(b) => match self
                    .binds
                    .iter_mut()
                    .find(|e| e.mods == b.mods && e.keysym == b.keysym)
                {
                    Some(existing) => *existing = b,
                    None => self.binds.push(b),
                },
                None => warn(n, "invalid bind", raw),
            }
        }
    }

    fn parse_bind(&self, val: &str) -> Option<Bind> {
        // mods, key, action, [arg (may contain commas, e.g. a spawn command)]
        let mut parts = val.splitn(4, ',');
        let mods = parse_mods(parts.next()?.trim(), self.modifier)?;
        let keysym = keysym_from_name(parts.next()?.trim())?;
        let action_name = parts.next()?.trim();
        let arg = parts.next().map(|s| s.trim());
        let action = parse_action(action_name, arg)?;
        Some(Bind {
            mods,
            keysym,
            action,
        })
    }
}

/// The default binds, replicating 0xin's original hardcoded behavior.
fn default_binds(modifier: u32) -> Vec<Bind> {
    let m = modifier;
    let ms = modifier | MOD_SHIFT;
    let mc = modifier | MOD_CTRL;
    let mut binds = vec![
        Bind {
            mods: m,
            keysym: key("Return"),
            action: Action::Spawn("kitty".into()),
        },
        Bind {
            mods: m,
            keysym: key("Q"),
            action: Action::Close,
        },
        Bind {
            mods: ms,
            keysym: key("Q"),
            action: Action::Quit,
        },
        Bind {
            mods: m,
            keysym: key("H"),
            action: Action::MoveFocus(Direction::Left),
        },
        Bind {
            mods: m,
            keysym: key("J"),
            action: Action::MoveFocus(Direction::Down),
        },
        Bind {
            mods: m,
            keysym: key("K"),
            action: Action::MoveFocus(Direction::Up),
        },
        Bind {
            mods: m,
            keysym: key("L"),
            action: Action::MoveFocus(Direction::Right),
        },
        Bind {
            mods: ms,
            keysym: key("H"),
            action: Action::MoveWindow(Direction::Left),
        },
        Bind {
            mods: ms,
            keysym: key("J"),
            action: Action::MoveWindow(Direction::Down),
        },
        Bind {
            mods: ms,
            keysym: key("K"),
            action: Action::MoveWindow(Direction::Up),
        },
        Bind {
            mods: ms,
            keysym: key("L"),
            action: Action::MoveWindow(Direction::Right),
        },
        Bind {
            mods: mc,
            keysym: key("H"),
            action: Action::ResizeWindow(Direction::Left),
        },
        Bind {
            mods: mc,
            keysym: key("J"),
            action: Action::ResizeWindow(Direction::Down),
        },
        Bind {
            mods: mc,
            keysym: key("K"),
            action: Action::ResizeWindow(Direction::Up),
        },
        Bind {
            mods: mc,
            keysym: key("L"),
            action: Action::ResizeWindow(Direction::Right),
        },
        Bind {
            mods: m,
            keysym: key("F"),
            action: Action::Fullscreen,
        },
        Bind {
            mods: m,
            keysym: key("V"),
            action: Action::ToggleFloating,
        },
    ];
    for i in 0..9u32 {
        let name = (b'1' + i as u8) as char;
        let name = name.to_string();
        binds.push(Bind {
            mods: m,
            keysym: key(&name),
            action: Action::Workspace(i as usize),
        });
        binds.push(Bind {
            mods: ms,
            keysym: key(&name),
            action: Action::MoveToWorkspace(i as usize),
        });
    }
    binds
}

// --- parsing helpers -------------------------------------------------------

/// Iterate (1-based line number, trimmed non-empty/non-comment line).
fn lines(text: &str) -> impl Iterator<Item = (usize, &str)> {
    text.lines().enumerate().filter_map(|(i, l)| {
        let l = l.trim();
        if l.is_empty() || l.starts_with('#') {
            None
        } else {
            Some((i + 1, l))
        }
    })
}

/// Split a `key = value` line; returns lowercased key and trimmed value.
fn split_kv(line: &str) -> Option<(&str, &str)> {
    let (k, v) = line.split_once('=')?;
    Some((k.trim(), v.trim()))
}

/// Parse a modifier spec like `SUPER SHIFT`, `super+shift`, `MOD`, `$mod`.
/// `MOD`/`$mod`/`mainmod` expand to `primary`.
fn parse_mods(spec: &str, primary: u32) -> Option<u32> {
    let mut bits = 0;
    for tok in spec.split(['+', ' ', '\t']).filter(|t| !t.is_empty()) {
        bits |= match tok.to_ascii_uppercase().trim_start_matches('$') {
            "MOD" | "MAINMOD" => primary,
            "SUPER" | "LOGO" | "WIN" => MOD_LOGO,
            "ALT" | "MOD1" => MOD_ALT,
            "SHIFT" => MOD_SHIFT,
            "CTRL" | "CONTROL" => MOD_CTRL,
            _ => return None,
        };
    }
    Some(bits)
}

fn parse_color(spec: &str) -> Option<(f32, f32, f32)> {
    let mut it = spec.split_whitespace();
    let r = it.next()?.parse().ok()?;
    let g = it.next()?.parse().ok()?;
    let b = it.next()?.parse().ok()?;
    if it.next().is_some() {
        return None;
    }
    Some((r, g, b))
}

/// Parse `NAME, XxY[, SCALE]` for a `monitor =` line.
fn parse_monitor(spec: &str) -> Option<MonitorConfig> {
    let mut parts = spec.splitn(3, ',');
    let name = parts.next()?.trim().to_string();
    let (x, y) = parse_xy(parts.next()?.trim())?;
    let scale = match parts.next() {
        Some(s) => s.trim().parse().ok()?,
        None => 1.0,
    };
    if name.is_empty() || scale <= 0.0 {
        return None;
    }
    Some(MonitorConfig { name, x, y, scale })
}

/// Parse `XxY` (e.g. `0x0`, `1920x-1080`) into layout coordinates.
fn parse_xy(spec: &str) -> Option<(i32, i32)> {
    let (xs, ys) = spec.split_once('x')?;
    Some((xs.trim().parse().ok()?, ys.trim().parse().ok()?))
}

/// Parse `W x H` for a `float_size =` line: two percentages in 1..=100,
/// each with an optional `%` suffix — `60x60`, `60% x 60%`, `55 x 70%`.
fn parse_float_size(spec: &str) -> Option<(i32, i32)> {
    let pct = |s: &str| -> Option<i32> {
        let n: i32 = s.trim().trim_end_matches('%').trim().parse().ok()?;
        (1..=100).contains(&n).then_some(n)
    };
    let (ws, hs) = spec.split_once('x')?;
    Some((pct(ws)?, pct(hs)?))
}

fn parse_action(name: &str, arg: Option<&str>) -> Option<Action> {
    match name.to_ascii_lowercase().as_str() {
        "spawn" | "exec" => Some(Action::Spawn(arg?.to_string())),
        "close" | "killactive" => Some(Action::Close),
        "quit" | "exit" => Some(Action::Quit),
        "focusnext" => Some(Action::FocusNext),
        "focusprev" => Some(Action::FocusPrev),
        "movefocus" => Some(Action::MoveFocus(direction_from_arg(arg?)?)),
        "movewindow" => Some(Action::MoveWindow(direction_from_arg(arg?)?)),
        "resizewindow" => Some(Action::ResizeWindow(direction_from_arg(arg?)?)),
        "fullscreen" | "togglefullscreen" => Some(Action::Fullscreen),
        "float" | "togglefloating" => Some(Action::ToggleFloating),
        "workspace" => Some(Action::Workspace(workspace_index(arg?)?)),
        "movetoworkspace" => Some(Action::MoveToWorkspace(workspace_index(arg?)?)),
        _ => None,
    }
}

/// Parse a 1-based workspace number (`1`..`9`) to a 0-based index.
fn workspace_index(arg: &str) -> Option<usize> {
    let n: usize = arg.trim().parse().ok()?;
    (1..=9).contains(&n).then(|| n - 1)
}

/// Parse a direction arg (`l`/`r`/`u`/`d`, case-insensitive; also accepts the
/// full words) for `movefocus`/`movewindow`.
fn direction_from_arg(arg: &str) -> Option<Direction> {
    match arg.trim().to_ascii_lowercase().as_str() {
        "l" | "left" => Some(Direction::Left),
        "r" | "right" => Some(Direction::Right),
        "u" | "up" => Some(Direction::Up),
        "d" | "down" => Some(Direction::Down),
        _ => None,
    }
}

/// Resolve a key name to a keysym, or None if xkb doesn't know it.
fn keysym_from_name(name: &str) -> Option<u32> {
    let c = CString::new(name).ok()?;
    let sym = unsafe { oxide_keysym_from_name(c.as_ptr()) };
    (sym != 0).then_some(sym)
}

/// Like `keysym_from_name` but for trusted built-in defaults (must resolve).
fn key(name: &str) -> u32 {
    keysym_from_name(name).expect("built-in default key name should resolve")
}

fn mod_name(m: u32) -> &'static str {
    match m {
        MOD_ALT => "Alt",
        MOD_LOGO => "Super",
        _ => "custom",
    }
}

fn warn(line: usize, msg: &str, raw: &str) {
    eprintln!("0xin: config line {line}: {msg}: `{raw}`");
}

fn config_path() -> Option<PathBuf> {
    if let Ok(path) = env::var("OXIN_CONFIG") {
        if !path.is_empty() {
            return Some(PathBuf::from(path));
        }
    }
    if let Ok(dir) = env::var("XDG_CONFIG_HOME") {
        if !dir.is_empty() {
            return Some(PathBuf::from(dir).join("0xin/0xin.conf"));
        }
    }
    let home = env::var("HOME").ok()?;
    Some(PathBuf::from(home).join(".config/0xin/0xin.conf"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_binds_override_only_named_chords() {
        let mut cfg = Config::default();
        cfg.binds = default_binds(cfg.modifier);
        let before = cfg.binds.len();

        // Overrides Mod+J (a default MoveFocus bind) back to the old
        // cyclic focusnext, without touching any other default bind.
        cfg.apply_binds("bind = MOD, J, focusnext\n");
        assert_eq!(
            cfg.binds.len(),
            before,
            "override must not grow the bind table"
        );

        let j = key("J");
        let overridden = cfg
            .binds
            .iter()
            .find(|b| b.mods == cfg.modifier && b.keysym == j)
            .unwrap();
        assert!(matches!(overridden.action, Action::FocusNext));

        // An untouched chord (workspace 3) still resolves to its default.
        let three = key("3");
        let untouched = cfg
            .binds
            .iter()
            .find(|b| b.mods == cfg.modifier && b.keysym == three)
            .unwrap();
        assert!(matches!(untouched.action, Action::Workspace(2)));
    }

    #[test]
    fn config_binds_append_new_chords() {
        let mut cfg = Config::default();
        cfg.binds = default_binds(cfg.modifier);
        let before = cfg.binds.len();

        cfg.apply_binds("bind = , Print, spawn, grim\n");
        assert_eq!(
            cfg.binds.len(),
            before + 1,
            "a new chord must be appended, not replace one"
        );
    }

    #[test]
    fn fullscreen_action_parses_and_has_default_bind() {
        let mut cfg = Config::default();
        cfg.binds = default_binds(cfg.modifier);

        // Default: Mod+F toggles fullscreen.
        let f = key("F");
        let default = cfg
            .binds
            .iter()
            .find(|b| b.mods == cfg.modifier && b.keysym == f)
            .unwrap();
        assert!(matches!(default.action, Action::Fullscreen));

        // Both config spellings parse, with no argument required.
        cfg.apply_binds("bind = MOD SHIFT, F, togglefullscreen\n");
        let msf = cfg
            .binds
            .iter()
            .find(|b| b.mods == (cfg.modifier | MOD_SHIFT) && b.keysym == f)
            .unwrap();
        assert!(matches!(msf.action, Action::Fullscreen));
    }

    #[test]
    fn float_rules_parse_lowercased_and_deduplicated() {
        let mut cfg = Config::default();
        cfg.parse_scalars("float = Zenity\nfloat = pavucontrol\nfloat = zenity\n");
        assert_eq!(cfg.float_rules, vec!["zenity", "pavucontrol"]);
    }

    #[test]
    fn first_split_parses_without_changing_desktop_default() {
        let mut cfg = Config::default();
        assert!(cfg.first_split_vertical);

        cfg.parse_scalars("first_split = horizontal\n");
        assert!(!cfg.first_split_vertical);

        cfg.parse_scalars("first_split = vertical\n");
        assert!(cfg.first_split_vertical);
    }

    #[test]
    fn keyboard_gesture_is_opt_in_and_parses_height() {
        let mut cfg = Config::default();
        assert!(cfg.gesture_keyboard.is_none());
        cfg.parse_scalars(
            "gesture_keyboard = wvkbd-mobintl\ngesture_keyboard_height = 280\n",
        );
        assert_eq!(cfg.gesture_keyboard.as_deref(), Some("wvkbd-mobintl"));
        assert_eq!(cfg.gesture_keyboard_height, 280);
    }

    #[test]
    fn float_size_parses_with_and_without_percent() {
        let mut cfg = Config::default();
        assert_eq!(cfg.float_size, (60, 60), "default must be 60% x 60%");

        cfg.parse_scalars("float_size = 55 x 70%\n");
        assert_eq!(cfg.float_size, (55, 70));

        cfg.parse_scalars("float_size = 80%x40%\n");
        assert_eq!(cfg.float_size, (80, 40));

        // Out-of-range or malformed values warn and leave the setting alone.
        cfg.parse_scalars("float_size = 0 x 60\n");
        cfg.parse_scalars("float_size = 60 x 120\n");
        cfg.parse_scalars("float_size = huge\n");
        assert_eq!(cfg.float_size, (80, 40));
    }

    #[test]
    fn togglefloating_action_parses_and_has_default_bind() {
        let mut cfg = Config::default();
        cfg.binds = default_binds(cfg.modifier);

        // Default: Mod+V toggles floating.
        let v = key("V");
        let default = cfg
            .binds
            .iter()
            .find(|b| b.mods == cfg.modifier && b.keysym == v)
            .unwrap();
        assert!(matches!(default.action, Action::ToggleFloating));

        // Both config spellings parse, with no argument required.
        cfg.apply_binds("bind = MOD SHIFT, V, togglefloating\n");
        let msv = cfg
            .binds
            .iter()
            .find(|b| b.mods == (cfg.modifier | MOD_SHIFT) && b.keysym == v)
            .unwrap();
        assert!(matches!(msv.action, Action::ToggleFloating));
    }

    #[test]
    fn monitor_line_parses_position_and_default_scale() {
        let mut cfg = Config::default();
        cfg.parse_scalars("monitor = HDMI-A-1, 0x-1080\n");
        let m = cfg.monitors.iter().find(|m| m.name == "HDMI-A-1").unwrap();
        assert_eq!((m.x, m.y), (0, -1080));
        assert_eq!(m.scale, 1.0);

        cfg.parse_scalars("monitor = eDP-1, 0x0, 1.5\n");
        let m = cfg.monitors.iter().find(|m| m.name == "eDP-1").unwrap();
        assert_eq!((m.x, m.y), (0, 0));
        assert_eq!(m.scale, 1.5);
    }

    #[test]
    fn monitor_line_overrides_same_name_instead_of_duplicating() {
        let mut cfg = Config::default();
        cfg.parse_scalars("monitor = HDMI-A-1, 0x0, 1.0\nmonitor = HDMI-A-1, 1920x0, 2.0\n");
        assert_eq!(cfg.monitors.len(), 1);
        let m = &cfg.monitors[0];
        assert_eq!((m.x, m.y), (1920, 0));
        assert_eq!(m.scale, 2.0);
    }
}
