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
//!
//! Split across a few files by concern: `types` is the vocabulary (the
//! structs/enums config lines parse into), `parse` is the low-level
//! string-to-value helpers, `defaults` is the built-in bind table, and this
//! file is the two-pass parsing engine (`Config::load`) that ties them
//! together.

mod defaults;
mod parse;
mod types;

#[cfg(test)]
mod tests;

pub use types::*;

use defaults::default_binds;
use parse::{
    config_path, keysym_from_name, lines, mod_name, parse_action, parse_color, parse_float_size,
    parse_gesture, parse_mods, parse_monitor, split_kv, warn,
};
// Only tests.rs (via `use super::*`) calls this directly — impl Config
// always goes through keysym_from_name instead.
#[cfg(test)]
use parse::key;
use std::env;
use std::fs;

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
            cfg.apply_hold_binds(text);
            cfg.apply_gestures(text);
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
                "wallpaper" => self.wallpaper = (!val.is_empty()).then(|| val.to_string()),
                "window_opacity" => match val.parse::<f32>() {
                    Ok(opacity) if (0.0..=1.0).contains(&opacity) => self.window_opacity = opacity,
                    _ => warn(
                        n,
                        "invalid window_opacity (want a number from 0.0 to 1.0)",
                        raw,
                    ),
                },
                "corner_radius" => match val.parse::<i32>() {
                    Ok(radius) if (0..=200).contains(&radius) => self.corner_radius = radius,
                    _ => warn(
                        n,
                        "invalid corner_radius (want an integer from 0 to 200)",
                        raw,
                    ),
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
                "exec_once" => {
                    if val.is_empty() {
                        warn(n, "empty exec_once command", raw);
                    } else {
                        self.exec_once.push(val.to_string());
                    }
                }
                "virtual_keyboard_show" => {
                    self.virtual_keyboard_show = (!val.is_empty()).then(|| val.to_string())
                }
                "virtual_keyboard_hide" => {
                    self.virtual_keyboard_hide = (!val.is_empty()).then(|| val.to_string())
                }
                "virtual_keyboard_height" => match val.parse::<i32>() {
                    Ok(height) if (80..=1000).contains(&height) => {
                        self.virtual_keyboard_height = height
                    }
                    _ => warn(
                        n,
                        "invalid virtual_keyboard_height (want 80..1000 logical pixels)",
                        raw,
                    ),
                },
                "gesture_handle" => match val {
                    "visible" => self.gesture_handle_visible = true,
                    "hidden" => self.gesture_handle_visible = false,
                    _ => warn(
                        n,
                        "invalid gesture_handle (want `visible` or `hidden`)",
                        raw,
                    ),
                },
                "bind" | "hold" | "gesture" => {} // handled in their second passes
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

    fn apply_hold_binds(&mut self, text: &str) {
        for (n, raw) in lines(text) {
            let Some((key, val)) = split_kv(raw) else {
                continue;
            };
            if key != "hold" {
                continue;
            }
            let Some(binding) = self.parse_hold_bind(val) else {
                warn(n, "invalid hold bind", raw);
                continue;
            };
            match self
                .hold_binds
                .iter_mut()
                .find(|item| item.mods == binding.mods && item.keysym == binding.keysym)
            {
                Some(existing) => *existing = binding,
                None => self.hold_binds.push(binding),
            }
        }
    }

    fn parse_hold_bind(&self, val: &str) -> Option<HoldBind> {
        let mut parts = val.splitn(5, ',');
        let mods = parse_mods(parts.next()?.trim(), self.modifier)?;
        let keysym = keysym_from_name(parts.next()?.trim())?;
        let duration_ms = parts.next()?.trim().parse::<i32>().ok()?;
        if !(100..=60_000).contains(&duration_ms) {
            return None;
        }
        let action_name = parts.next()?.trim();
        let arg = parts.next().map(str::trim);
        Some(HoldBind {
            mods,
            keysym,
            duration_ms,
            action: parse_action(action_name, arg)?,
        })
    }

    fn apply_gestures(&mut self, text: &str) {
        for (n, raw) in lines(text) {
            let Some((key, val)) = split_kv(raw) else {
                continue;
            };
            if key != "gesture" {
                continue;
            }
            match parse_gesture(val) {
                Some(binding) => match self
                    .gestures
                    .iter_mut()
                    .find(|existing| existing.trigger == binding.trigger)
                {
                    Some(existing) => *existing = binding,
                    None => self.gestures.push(binding),
                },
                None => warn(n, "invalid gesture", raw),
            }
        }
    }

    pub fn gesture_mask(&self) -> u32 {
        self.gestures
            .iter()
            .fold(0, |mask, binding| mask | 1 << binding.trigger as u32)
    }

    pub fn has_keyboard_handle(&self) -> bool {
        self.gesture_handle_visible
            && self.gestures.iter().any(|binding| {
                matches!(
                    binding.trigger,
                    GestureTrigger::BottomUp | GestureTrigger::BottomDown
                )
            })
    }
}
