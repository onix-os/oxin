//! Bringing the Lua layer up for a compositor run.
//!
//! Kept apart from `config` so the policy — where the file lives, what happens
//! when it is broken — is in one place and testable without a VM.

use std::env;
use std::fs;
use std::path::PathBuf;

use luna::Lua;

use crate::config::Config;

use super::registry::LuaRegistry;
use super::{config, vm};

/// Where `init.lua` is looked for, in order:
///
/// 1. `$OXIN_LUA_CONFIG` — an exact path, for testing a config from the repo
///    without touching `~/.config`. Mirrors `$OXIN_CONFIG` for the old format.
/// 2. `$XDG_CONFIG_HOME/0xin/init.lua`
/// 3. `~/.config/0xin/init.lua`
pub(crate) fn config_path() -> Option<PathBuf> {
    if let Ok(p) = env::var("OXIN_LUA_CONFIG") {
        if !p.is_empty() {
            return Some(PathBuf::from(p));
        }
    }
    if let Ok(dir) = env::var("XDG_CONFIG_HOME") {
        if !dir.is_empty() {
            return Some(PathBuf::from(dir).join("0xin/init.lua"));
        }
    }
    env::var("HOME")
        .ok()
        .filter(|h| !h.is_empty())
        .map(|h| PathBuf::from(h).join(".config/0xin/init.lua"))
}

/// What a run of the Lua layer produced.
pub(crate) struct Session {
    pub(crate) vm: Lua,
    pub(crate) registry: LuaRegistry,
    pub(crate) config: Config,
}

/// Build the VM and run `init.lua`, if there is one.
///
/// Never fails: a broken config is reported and the built-in defaults are used
/// instead, because a compositor that refuses to start is a black screen with
/// nowhere to read the error. The message is kept on the registry so
/// `0xinctl` can print it — a session that silently reverted to defaults is
/// otherwise indistinguishable from one that loaded fine.
pub(crate) fn start() -> Session {
    let mut vm = vm::build();
    let mut registry = LuaRegistry::new();

    let Some(path) = config_path() else {
        println!("0xin: no config path (no $HOME) — using defaults");
        return Session {
            vm,
            registry,
            config: Config::default(),
        };
    };

    let source = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(_) => {
            println!("0xin: no {} — using defaults", path.display());
            return Session {
                vm,
                registry,
                config: Config::default(),
            };
        }
    };

    match config::load(&mut vm, &path, &source) {
        Ok(config) => {
            println!("0xin: loaded {}", path.display());
            Session {
                vm,
                registry,
                config,
            }
        }
        Err(message) => {
            report(&path, &message);
            registry.config_error = Some(message);
            Session {
                vm,
                registry,
                config: Config::default(),
            }
        }
    }
}

/// Say loudly what went wrong. A user whose config silently reverted is left
/// staring at a working compositor with no explanation and no scrollback.
fn report(path: &std::path::Path, message: &str) {
    eprintln!("0xin: ─────────────────────────────────────────");
    eprintln!("0xin: config error in {}", path.display());
    for line in message.lines() {
        eprintln!("0xin:   {line}");
    }
    eprintln!("0xin: starting with built-in defaults instead");
    eprintln!("0xin: (`0xinctl config-error` repeats this)");
    eprintln!("0xin: ─────────────────────────────────────────");
}
