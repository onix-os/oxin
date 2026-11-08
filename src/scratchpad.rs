//! Scratchpads: a floating window one key hides and brings back, with its
//! program running the whole time — a terminal, or a tool of your own
//! running in one.
//!
//! `oxin.scratchpads["term"] = { cmd = ..., app_id = ... }` defines one and
//! `oxin.action.scratchpad("term")` (or `0xinctl scratchpad term`) toggles
//! it. A window is recognised as that scratchpad by its app_id, which `cmd`
//! has to give it (`kitty --class term`, `foot --app-id term`): matching on
//! the app_id rather than on "the next window to map" survives a slow start,
//! and lets a scratchpad started some other way still be one.
//!
//! A *hidden* scratchpad belongs to no workspace: it sits in
//! `Server.scratchpad_hidden` with its scene tree disabled. Tiling, focus,
//! workspace listings and pruning only ever look at workspaces, so they leave
//! it alone with no changes of their own; the few things that have to reach
//! every window (destroy, the scene rebuild after a VT switch, reload) look
//! at that list too. A *shown* scratchpad is an ordinary floating window on
//! the workspace it was shown on, and stays there when you switch away.

use std::ffi::CStr;
use std::time::{Duration, Instant};

use crate::config::ScratchpadConfig;
use crate::ffi::{
    oxide_scene_tree_raise_to_top, oxide_scene_tree_reparent, oxide_scene_tree_set_enabled,
    oxide_xdg_toplevel_app_id, oxide_xdg_toplevel_parent,
};
use crate::keybindings::focus_index;
use crate::state::{GrabMode, Server, Toplevel};
use crate::tiling::{active_output, active_workspace, refresh};
use crate::toplevel::{place_floating_at, set_floating, set_fullscreen};
use crate::workspaces::{detach, focus_windows_of};

/// How long a spawned scratchpad may take to map before another toggle
/// starts it again (a `cmd` that failed must not block it for good).
const SPAWN_GRACE: Duration = Duration::from_secs(3);

/// Where a scratchpad's window is, as far as the toggle cares.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Found {
    Hidden,
    /// On a workspace. `here`: the one shown on the focused output.
    /// `focused`: it holds keyboard focus.
    Shown {
        here: bool,
        focused: bool,
    },
}

/// What a toggle does to a scratchpad that is running. (One that isn't is
/// started.)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    /// Hidden: show it here.
    Show,
    /// Shown on another workspace (or output): move it here.
    Bring,
    /// Shown here but something else has focus: focus it. Hiding a window
    /// you weren't using would be a surprise; the next press hides it.
    Focus,
    /// Shown here and focused: hide it.
    Hide,
}

fn decide(found: Found) -> Step {
    match found {
        Found::Hidden => Step::Show,
        Found::Shown { here: false, .. } => Step::Bring,
        Found::Shown {
            here: true,
            focused: false,
        } => Step::Focus,
        Found::Shown {
            here: true,
            focused: true,
        } => Step::Hide,
    }
}

/// The window that is scratchpad `name`, and where it is.
unsafe fn find(server: &Server, name: &str) -> Option<(*mut Toplevel, Found)> {
    let is = |tl: *mut Toplevel| (*tl).scratchpad.as_deref() == Some(name);
    if let Some(&tl) = server.scratchpad_hidden.iter().find(|&&tl| is(tl)) {
        return Some((tl, Found::Hidden));
    }
    let here = (!server.outputs.is_empty()).then(|| active_workspace(server));
    server.workspaces.iter().find_map(|(id, ws)| {
        let &tl = ws.windows.iter().find(|&&tl| is(tl))?;
        Some((
            tl,
            Found::Shown {
                here: here == Some(id),
                focused: server.focused_tl == tl,
            },
        ))
    })
}

/// `oxin.action.scratchpad(name)`: start it if it isn't running, otherwise
/// see `Step`.
pub(crate) unsafe fn toggle(server: &mut Server, name: &str) -> Result<(), String> {
    if server.locked {
        return Err("session is locked".into());
    }
    if server.outputs.is_empty() {
        return Err("no output to show it on".into());
    }
    let Some(spec) = server.config.scratchpad(name).cloned() else {
        return Err(format!(
            "no scratchpad {name:?}; define it in oxin.scratchpads"
        ));
    };
    let Some((tl, found)) = find(server, name) else {
        spawn(server, &spec);
        return Ok(());
    };
    let step = decide(found);
    match step {
        Step::Show => {
            server.scratchpad_hidden.retain(|&w| w != tl);
            attach_here(server, tl, &spec);
        }
        Step::Bring => {
            make_plain_floating(server, tl);
            detach(server, tl);
            attach_here(server, tl, &spec);
        }
        Step::Focus => {
            let a = active_workspace(server);
            if let Some(i) = server.workspaces[a].windows.iter().position(|&w| w == tl) {
                oxide_scene_tree_raise_to_top((*tl).scene_tree);
                focus_index(server, i);
            }
        }
        Step::Hide => hide(server, tl),
    }
    println!("0xin: scratchpad {name}: {step:?}");
    Ok(())
}

unsafe fn spawn(server: &mut Server, spec: &ScratchpadConfig) {
    let now = Instant::now();
    server
        .scratchpad_spawning
        .retain(|(_, since)| now.duration_since(*since) < SPAWN_GRACE);
    if server
        .scratchpad_spawning
        .iter()
        .any(|(n, _)| *n == spec.name)
    {
        return; // still starting from the last press
    }
    server.scratchpad_spawning.push((spec.name.clone(), now));
    println!(
        "0xin: scratchpad {}: starting `{}`, for a window with app_id {:?}",
        spec.name, spec.cmd, spec.app_id
    );
    crate::keybindings::spawn(&spec.cmd);
}

/// Floating and not fullscreen, the state a scratchpad is shown in — undoing
/// a `Mod+V` or `Mod+F` from while it was shown, so the next show looks like
/// every other.
unsafe fn make_plain_floating(server: &mut Server, tl: *mut Toplevel) {
    if (*tl).fullscreen {
        set_fullscreen(server, tl, false);
    }
    if !(*tl).floating {
        set_floating(server, tl, true);
    }
}

/// Put a (detached) scratchpad on the focused output's workspace, placed per
/// its spec, on top, and focused.
unsafe fn attach_here(server: &mut Server, tl: *mut Toplevel, spec: &ScratchpadConfig) {
    (*tl).floating = true;
    oxide_scene_tree_reparent((*tl).scene_tree, server.tree_floating);
    oxide_scene_tree_raise_to_top((*tl).scene_tree);
    place(server, tl, spec);
    let a = active_workspace(server);
    server.workspaces[a].windows.push(tl);
    refresh(server); // enables its scene tree
    focus_index(server, server.workspaces[a].windows.len() - 1);
}

unsafe fn hide(server: &mut Server, tl: *mut Toplevel) {
    if server.grab_tl == tl {
        server.grab = GrabMode::None;
        server.grab_tl = std::ptr::null_mut();
    }
    make_plain_floating(server, tl);
    detach(server, tl);
    // Nothing else will: refresh() only touches windows on a workspace.
    oxide_scene_tree_set_enabled((*tl).scene_tree, false);
    server.scratchpad_hidden.push(tl);
    refresh(server);
    // Keys go back to what is under it rather than to a hidden window.
    focus_windows_of(server, active_workspace(server));
}

/// The size a scratchpad is shown at on the focused output, in logical
/// pixels, already fitted to leave room for the border — so the very first
/// configure matches what `place` will settle on.
pub(crate) unsafe fn size(server: &Server, spec: &ScratchpadConfig) -> (i32, i32) {
    if server.outputs.is_empty() {
        return (0, 0);
    }
    let o = &server.outputs[active_output(server)];
    let bw = server.config.border_width;
    let (pw, ph) = spec.size_percent(server.config.float_size);
    (
        (o.uw * pw / 100).min(o.uw - 2 * bw).max(1),
        (o.uh * ph / 100).min(o.uh - 2 * bw).max(1),
    )
}

/// Size and position a scratchpad on the focused output per its spec.
pub(crate) unsafe fn place(server: &Server, tl: *mut Toplevel, spec: &ScratchpadConfig) {
    let (w, h) = size(server, spec);
    place_floating_at(server, tl, w, h, spec.position);
}

/// If `tl` is a scratchpad's window, mark it as one and return its spec.
///
/// Called on the first commit (so the first configure already has the
/// scratchpad's size) and again on map, for clients that set their app_id
/// late. Dialogs never are: a scratchpad app's file picker shares its
/// app_id. Nor is a second window with the same app_id while the first
/// exists — the toggle can only drive one.
pub(crate) unsafe fn claim(server: &Server, tl: *mut Toplevel) -> Option<ScratchpadConfig> {
    if let Some(name) = &(*tl).scratchpad {
        return server.config.scratchpad(name).cloned();
    }
    if !oxide_xdg_toplevel_parent((*tl).xdg_toplevel).is_null() {
        return None;
    }
    let app_id = oxide_xdg_toplevel_app_id((*tl).xdg_toplevel);
    if app_id.is_null() {
        return None;
    }
    let app_id = CStr::from_ptr(app_id).to_string_lossy();
    let spec = server.config.scratchpad_for_app_id(&app_id)?;
    if find(server, &spec.name).is_some() {
        return None;
    }
    (*tl).scratchpad = Some(spec.name.clone());
    Some(spec.clone())
}

/// After a reload: a window whose scratchpad is no longer defined becomes an
/// ordinary floating window, shown here if it was hidden — the toggle that
/// could have brought it back is gone, and it must not stay invisible.
pub(crate) unsafe fn forget_undefined(server: &mut Server) {
    let defined = |server: &Server, tl: *mut Toplevel| {
        (*tl)
            .scratchpad
            .as_deref()
            .is_none_or(|name| server.config.scratchpad(name).is_some())
    };
    let windows: Vec<*mut Toplevel> = server
        .workspaces
        .values()
        .flat_map(|ws| ws.windows.iter().copied())
        .collect();
    for tl in windows {
        if !defined(server, tl) {
            (*tl).scratchpad = None;
        }
    }
    let stranded: Vec<*mut Toplevel> = server
        .scratchpad_hidden
        .iter()
        .copied()
        .filter(|&tl| !defined(server, tl))
        .collect();
    if stranded.is_empty() || server.outputs.is_empty() {
        return;
    }
    for tl in stranded {
        let name = (*tl).scratchpad.take().unwrap_or_default();
        eprintln!("0xin: reload: scratchpad {name:?} is gone; showing its window");
        server.scratchpad_hidden.retain(|&w| w != tl);
        (*tl).floating = true;
        oxide_scene_tree_reparent((*tl).scene_tree, server.tree_floating);
        crate::toplevel::place_floating_at(
            server,
            tl,
            (*tl).w,
            (*tl).h,
            crate::config::ScratchpadPosition::Center,
        );
        let a = active_workspace(server);
        server.workspaces[a].windows.push(tl);
    }
    refresh(server);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_press_shows_brings_focuses_or_hides_by_where_the_window_is() {
        assert_eq!(decide(Found::Hidden), Step::Show);
        let shown = |here, focused| Found::Shown { here, focused };
        // Another workspace, focused or not (focus there can't be ours to
        // keep): bring it over.
        assert_eq!(decide(shown(false, false)), Step::Bring);
        assert_eq!(decide(shown(false, true)), Step::Bring);
        // Here: the first press goes to it, only a press while using it
        // hides it.
        assert_eq!(decide(shown(true, false)), Step::Focus);
        assert_eq!(decide(shown(true, true)), Step::Hide);
    }
}
