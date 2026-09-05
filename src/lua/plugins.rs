//! Config somebody else wrote, found on disk.
//!
//! This follows neovim's model rather than inventing one. Its plugin layout
//! has survived twenty years of real plugins, and anyone arriving at 0xin has
//! probably already learned it — deviating buys nothing and costs everyone
//! the transfer.
//!
//! The parts worth copying, reduced to what a compositor needs:
//!
//! - **A search path, not a directory.** An ordered list of roots, each with
//!   the same layout. The tool's own files sit on it like everyone else's, a
//!   package is just another root, and later roots override earlier ones.
//! - **`plugin/` runs, `lua/` is required.** A file under `plugin/` is a
//!   statement 0xin executes for you; one under `lua/` does nothing until
//!   something requires it. Tools that auto-run everything leave plugin
//!   authors nowhere to put a helper.
//! - **`after/` runs last**, so a user who wants to undo what a plugin did has
//!   somewhere to do it that is not "edit the plugin".
//! - **Alphabetical within a directory, path order between.** Directory order
//!   is filesystem order, which differs between machines.
//!
//! Load order overall is `init.lua`, then every root's `plugin/`, then the
//! `after` roots — the same as neovim, which is why `after/plugin/` is where
//! you override a plugin.

use std::fs;
use std::path::{Path, PathBuf};

/// The system-wide config root, for anything installed by a package manager —
/// a distro package, or the NixOS module in `nix/module.nix`. Named here
/// because `session` looks for a system `init.lua` in the same place, and two
/// copies of the path would be two things to keep in step.
pub(super) const SYSTEM_ROOT: &str = "/etc/xdg/0xin";

/// The ordered list of roots plugins are looked for in.
///
/// A path with one entry is still a path; growing one later is a config break,
/// so the shape is here from the start.
pub(crate) fn runtimepath() -> Vec<PathBuf> {
    let mut roots = Vec::new();

    // System-wide, for anything installed by a package manager.
    roots.push(PathBuf::from(SYSTEM_ROOT));

    // The user's own.
    if let Some(dir) = config_dir() {
        roots.push(dir);
    }

    roots
}

fn config_dir() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("XDG_CONFIG_HOME") {
        if !dir.is_empty() {
            return Some(PathBuf::from(dir).join("0xin"));
        }
    }
    std::env::var("HOME")
        .ok()
        .filter(|h| !h.is_empty())
        .map(|h| PathBuf::from(h).join(".config/0xin"))
}

/// Every `plugin/**/*.lua` to run, in the order to run them.
///
/// Packages (`pack/*/start/*`) join the path as they are found, so installing
/// one is "put a directory here" — no registry, no manifest, no install step.
pub(crate) fn plugin_files(roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for root in roots {
        for package in packages(root) {
            collect(&package.join("plugin"), &mut files);
        }
        collect(&root.join("plugin"), &mut files);
    }
    // `after` roots last, whatever else happened.
    for root in roots {
        collect(&root.join("after/plugin"), &mut files);
    }
    files
}

/// Directories `require` should search, as `package.path` entries.
pub(crate) fn lua_path(roots: &[PathBuf]) -> String {
    let mut parts = Vec::new();
    for root in roots {
        for package in packages(root) {
            push_lua_dir(&package, &mut parts);
        }
        push_lua_dir(root, &mut parts);
    }
    parts.join(";")
}

fn push_lua_dir(root: &Path, parts: &mut Vec<String>) {
    let dir = root.join("lua");
    if dir.is_dir() {
        parts.push(format!("{}/?.lua", dir.display()));
        parts.push(format!("{}/?/init.lua", dir.display()));
    }
}

/// `<root>/pack/*/start/*` — a package is a directory laid out exactly like a
/// config root, somewhere else on the path. Not a special format.
fn packages(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let pack = root.join("pack");
    for group in sorted_dirs(&pack) {
        out.extend(sorted_dirs(&group.join("start")));
    }
    out
}

fn sorted_dirs(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    out.sort();
    out
}

/// Every `.lua` under `dir`, recursively, alphabetically per directory.
fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut files = Vec::new();
    let mut subdirs = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            subdirs.push(path);
        } else if path.extension().is_some_and(|e| e == "lua") {
            files.push(path);
        }
    }
    // Sorted, because readdir order differs between machines and after a
    // reinstall — a plugin that loads first on your laptop should load first
    // everywhere.
    files.sort();
    subdirs.sort();
    out.append(&mut files);
    for sub in subdirs {
        collect(&sub, out);
    }
}
