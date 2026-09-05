//! Bringing the Lua layer up for a compositor run.
//!
//! Kept apart from `config` so the policy — where the file lives, what happens
//! when it is broken — is in one place and testable without a VM.

use std::env;
use std::fs;
use std::path::PathBuf;

use luna::{Executor, Lua};

use crate::config::Config;

use super::registry::LuaRegistry;
use super::{config, plugins, vm};

/// Where `init.lua` is looked for, in order:
///
/// 1. `$OXIN_CONFIG` — an exact path, for testing a config from the repo
///    without touching `~/.config`. Same variable the line-based format used:
///    the format changed, the role did not.
/// 2. `$XDG_CONFIG_HOME/0xin/init.lua`
/// 3. `~/.config/0xin/init.lua`
/// 4. `/etc/xdg/0xin/init.lua` — whatever packaged 0xin for this machine, such
///    as the NixOS module in `nix/module.nix`. Last, so a user's own file
///    always wins over the system's; it is the root `plugins::runtimepath`
///    already scans, so a package has one directory to install into.
pub(crate) fn config_path() -> Option<PathBuf> {
    if let Ok(p) = env::var("OXIN_CONFIG") {
        if !p.is_empty() {
            return Some(PathBuf::from(p));
        }
    }
    pick(user_config_path(), system_config_path())
}

/// The user's own path, if the environment says where their home is.
fn user_config_path() -> Option<PathBuf> {
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

fn system_config_path() -> PathBuf {
    PathBuf::from(plugins::SYSTEM_ROOT).join("init.lua")
}

/// The user's file when they have one, the system's when they don't — and the
/// user's *path* when neither exists, because "no ~/.config/0xin/init.lua" is
/// a more useful thing to print than the name of a file they cannot write.
///
/// Split out from `config_path` so the order is testable without setting
/// process-wide environment variables, which parallel tests cannot do.
pub(super) fn pick(user: Option<PathBuf>, system: PathBuf) -> Option<PathBuf> {
    if let Some(path) = &user {
        if path.is_file() {
            return user;
        }
    }
    if system.is_file() {
        return Some(system);
    }
    user
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

    // One executor, reused for every handler the compositor runs. `ctx.stash`
    // allocates a root, so a fresh one per keypress would be waste on the
    // hottest path there is.
    let executor = vm.enter(|ctx| ctx.stash(Executor::new(ctx)));
    registry.set_executor(executor);

    // Plugins are config somebody else wrote, found on a search path. They
    // can be turned off entirely, because the first question when a
    // compositor misbehaves is "is it me or a plugin?" and a tool with no way
    // to start without them makes that unanswerable.
    let roots = plugins::runtimepath();
    let plugin_files = if no_plugins() {
        println!("0xin: plugins disabled (OXIN_NOPLUGIN)");
        Vec::new()
    } else {
        plugins::plugin_files(&roots)
    };

    // `lua/` directories on the path so a plugin can ship a helper that only
    // runs when something requires it.
    let lua_path = plugins::lua_path(&roots);
    if !lua_path.is_empty() {
        let _ = vm.try_enter(|ctx| {
            let package: luna::Table = ctx.get_global("package")?;
            package.set(ctx, "path", lua_path.as_str())?;
            Ok(())
        });
    }

    let path = config_path();
    let source = path.as_ref().and_then(|p| fs::read(p).ok());
    let chunk = match (&path, &source) {
        (Some(p), Some(bytes)) => Some((p.as_path(), bytes.clone())),
        _ => {
            match &path {
                Some(p) => println!("0xin: no {} — using defaults", p.display()),
                None => println!("0xin: no config path (no $HOME) — using defaults"),
            }
            None
        }
    };
    if !plugin_files.is_empty() {
        println!(
            "0xin: {} plugin file(s) on the runtimepath",
            plugin_files.len()
        );
    }

    match config::load_all(&mut vm, chunk, &plugin_files) {
        Ok(finished) => {
            if let Some(p) = &path {
                if source.is_some() {
                    println!("0xin: loaded {}", p.display());
                }
            }
            registry.set_functions(finished.functions);
            registry.set_handlers(finished.handlers);
            Session {
                vm,
                registry,
                config: finished.config,
            }
        }
        Err(message) => {
            report(path.as_deref(), &message);
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
fn report(path: Option<&std::path::Path>, message: &str) {
    let _ = path;
    eprintln!("0xin: ─────────────────────────────────────────");
    eprintln!("0xin: config error");
    for line in message.lines() {
        eprintln!("0xin:   {line}");
    }
    eprintln!("0xin: starting with built-in defaults instead");
    eprintln!("0xin: (`0xinctl config-error` repeats this)");
    eprintln!("0xin: ─────────────────────────────────────────");
}

/// Whether to skip plugins entirely for this run.
fn no_plugins() -> bool {
    std::env::args().any(|a| a == "--noplugin")
        || std::env::var("OXIN_NOPLUGIN").is_ok_and(|v| !v.is_empty())
}
