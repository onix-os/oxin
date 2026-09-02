//! Resolving the names a config writes to what the compositor matches on.
//!
//! Modifier names, xkb key names, and the reverse mapping for reporting. This
//! is all that survived the line-based `0xin.conf` parser: the format is gone,
//! but the vocabulary it established is what `init.lua` still speaks, so
//! `"MOD+SHIFT+q"` means what it always did.

use super::types::{MOD_ALT, MOD_CTRL, MOD_LOGO, MOD_SHIFT};
use std::ffi::CString;
use std::os::raw::c_char;

extern "C" {
    fn oxide_keysym_from_name(name: *const c_char) -> u32;
}

/// Parse a modifier spec like `SUPER SHIFT`, `super+shift`, `MOD`, `$mod`.
/// `MOD`/`$mod`/`mainmod` expand to `primary`.
pub(crate) fn parse_mods(spec: &str, primary: u32) -> Option<u32> {
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

/// Resolve a key name to a keysym, or None if xkb doesn't know it.
pub(crate) fn keysym_from_name(name: &str) -> Option<u32> {
    let c = CString::new(name).ok()?;
    let sym = unsafe { oxide_keysym_from_name(c.as_ptr()) };
    (sym != 0).then_some(sym)
}

/// Like `keysym_from_name` but for trusted built-in defaults (must resolve).
pub(crate) fn key(name: &str) -> u32 {
    keysym_from_name(name).expect("built-in default key name should resolve")
}

/// How a modifier is named back to the user — in a log line, and when a config
/// reads `oxin.modifier` without having written it.
pub(crate) fn mod_name(m: u32) -> &'static str {
    match m {
        MOD_ALT => "Alt",
        MOD_LOGO => "Super",
        _ => "custom",
    }
}
