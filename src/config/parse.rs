//! Line-level parsing helpers shared by `mod.rs`'s two parsing passes and
//! `defaults.rs`'s built-in bind table.

use super::types::*;
use std::env;
use std::ffi::CString;
use std::os::raw::c_char;
use std::path::PathBuf;

extern "C" {
    fn oxide_keysym_from_name(name: *const c_char) -> u32;
}

/// Iterate (1-based line number, trimmed non-empty/non-comment line).
pub(super) fn lines(text: &str) -> impl Iterator<Item = (usize, &str)> {
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
pub(super) fn split_kv(line: &str) -> Option<(&str, &str)> {
    let (k, v) = line.split_once('=')?;
    Some((k.trim(), v.trim()))
}

/// Parse a modifier spec like `SUPER SHIFT`, `super+shift`, `MOD`, `$mod`.
/// `MOD`/`$mod`/`mainmod` expand to `primary`.
pub(super) fn parse_mods(spec: &str, primary: u32) -> Option<u32> {
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

pub(super) fn parse_color(spec: &str) -> Option<(f32, f32, f32)> {
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
pub(super) fn parse_monitor(spec: &str) -> Option<MonitorConfig> {
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
pub(super) fn parse_float_size(spec: &str) -> Option<(i32, i32)> {
    let pct = |s: &str| -> Option<i32> {
        let n: i32 = s.trim().trim_end_matches('%').trim().parse().ok()?;
        (1..=100).contains(&n).then_some(n)
    };
    let (ws, hs) = spec.split_once('x')?;
    Some((pct(ws)?, pct(hs)?))
}

pub(super) fn parse_action(name: &str, arg: Option<&str>) -> Option<Action> {
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
        "solo" | "togglesolo" => Some(Action::ToggleSolo),
        "float" | "togglefloating" => Some(Action::ToggleFloating),
        "workspace" => Some(Action::Workspace(workspace_index(arg?)?)),
        "movetoworkspace" => Some(Action::MoveToWorkspace(workspace_index(arg?)?)),
        "movetoworkspacenext" => Some(Action::MoveToWorkspaceNext),
        "movetoworkspaceprev" | "movetoworkspaceprevious" => Some(Action::MoveToWorkspacePrevious),
        "workspacenext" => Some(Action::WorkspaceNext),
        "workspaceprev" | "workspaceprevious" => Some(Action::WorkspacePrevious),
        "keyboardshow" => Some(Action::KeyboardShow),
        "keyboardhide" => Some(Action::KeyboardHide),
        "keyboardtoggle" => Some(Action::KeyboardToggle),
        _ => None,
    }
}

pub(super) fn parse_gesture(val: &str) -> Option<GestureBind> {
    let mut parts = val.splitn(3, ',');
    let trigger = match parts.next()?.trim().to_ascii_lowercase().as_str() {
        "bottom-up" => GestureTrigger::BottomUp,
        "bottom-down" => GestureTrigger::BottomDown,
        "edge-left-in" => GestureTrigger::EdgeLeftIn,
        "edge-right-in" => GestureTrigger::EdgeRightIn,
        "edge-left-up" => GestureTrigger::EdgeLeftUp,
        "edge-left-down" => GestureTrigger::EdgeLeftDown,
        "edge-right-up" => GestureTrigger::EdgeRightUp,
        "edge-right-down" => GestureTrigger::EdgeRightDown,
        "top-right" => GestureTrigger::TopRight,
        "top-left" => GestureTrigger::TopLeft,
        "top-down" => GestureTrigger::TopDown,
        "to-top" => GestureTrigger::ToTop,
        "to-left" => GestureTrigger::ToLeft,
        "to-right" => GestureTrigger::ToRight,
        "two-up" => GestureTrigger::TwoUp,
        "two-down" => GestureTrigger::TwoDown,
        "two-left" => GestureTrigger::TwoLeft,
        "two-right" => GestureTrigger::TwoRight,
        "three-up" => GestureTrigger::ThreeUp,
        "three-down" => GestureTrigger::ThreeDown,
        "three-left" => GestureTrigger::ThreeLeft,
        "three-right" => GestureTrigger::ThreeRight,
        "double-tap" => GestureTrigger::DoubleTap,
        _ => return None,
    };
    let action_name = parts.next()?.trim();
    let arg = parts.next().map(str::trim);
    Some(GestureBind {
        trigger,
        action: parse_action(action_name, arg)?,
    })
}

/// Parse a 1-based workspace number (`1`..`9`) to a 0-based index.
pub(super) fn workspace_index(arg: &str) -> Option<usize> {
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
pub(super) fn keysym_from_name(name: &str) -> Option<u32> {
    let c = CString::new(name).ok()?;
    let sym = unsafe { oxide_keysym_from_name(c.as_ptr()) };
    (sym != 0).then_some(sym)
}

/// Like `keysym_from_name` but for trusted built-in defaults (must resolve).
pub(super) fn key(name: &str) -> u32 {
    keysym_from_name(name).expect("built-in default key name should resolve")
}

pub(super) fn mod_name(m: u32) -> &'static str {
    match m {
        MOD_ALT => "Alt",
        MOD_LOGO => "Super",
        _ => "custom",
    }
}

pub(super) fn warn(line: usize, msg: &str, raw: &str) {
    eprintln!("0xin: config line {line}: {msg}: `{raw}`");
}

pub(super) fn config_path() -> Option<PathBuf> {
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
