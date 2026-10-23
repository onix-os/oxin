//! Reload the config when it is saved (Stage 31).
//!
//! The directories come from `lua::plugins::watch_dirs`; the inotify records
//! are read in the shim (`shim/config_watch.c`). A save is rarely one event —
//! an editor writes a temporary, renames it over the original, and may touch a
//! backup on the way — so events only arm a short timer, and the reload runs
//! once things have been quiet for `DEBOUNCE_MS`.

use crate::ffi::{
    oxide_event_loop_add_readable, oxide_event_loop_add_timer, oxide_event_source_remove,
    oxide_inotify_drain, oxide_inotify_open, oxide_inotify_watch_dir,
};
use crate::lua::{plugins, session};
use crate::state::Server;
use crate::wlr;
use std::ffi::CString;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::raw::c_void;
use std::os::unix::ffi::OsStrExt;

/// Quiet time after the last relevant event before reloading. Long enough to
/// cover an editor's write-then-rename, short enough to feel immediate.
const DEBOUNCE_MS: i32 = 200;

pub(crate) unsafe fn setup(
    server: &mut Server,
    event_loop: *mut wlr::wl_event_loop,
    userdata: *mut c_void,
) -> Result<(), String> {
    let fd = oxide_inotify_open();
    if fd < 0 {
        return Err("cannot open inotify".into());
    }
    // SAFETY: a fresh fd from inotify_init1, owned by nothing else.
    let fd = OwnedFd::from_raw_fd(fd);
    if oxide_event_loop_add_readable(event_loop, fd.as_raw_fd(), handle_readable, userdata)
        .is_null()
    {
        return Err("cannot register inotify with the Wayland event loop".into());
    }
    server.watch_fd = Some(fd);
    let watched = watch(server);
    eprintln!("0xin: watching {watched} directories for config changes");
    Ok(())
}

/// Add a watch for every directory the config can load from. Re-adding one
/// already watched is a no-op, so this is also how directories created since
/// the last reload (a new `plugin/`) start being watched.
fn watch(server: &Server) -> usize {
    let Some(fd) = &server.watch_fd else {
        return 0;
    };
    let config = session::config_path();
    let dirs = plugins::watch_dirs(config.as_deref(), &plugins::runtimepath());
    let mut watched = 0;
    for dir in dirs {
        let Ok(path) = CString::new(dir.as_os_str().as_bytes()) else {
            continue;
        };
        // SAFETY: a valid fd and a NUL-terminated path.
        if unsafe { oxide_inotify_watch_dir(fd.as_raw_fd(), path.as_ptr()) } {
            watched += 1;
        }
    }
    watched
}

unsafe extern "C" fn handle_readable(userdata: *mut c_void, _data: *mut c_void) {
    let server = &mut *(userdata as *mut Server);
    let Some(fd) = &server.watch_fd else {
        return;
    };
    if !oxide_inotify_drain(fd.as_raw_fd()) {
        return;
    }
    // Restart the quiet period: only the last event of a burst reloads.
    if !server.watch_timer.is_null() {
        oxide_event_source_remove(server.watch_timer);
    }
    server.watch_timer =
        oxide_event_loop_add_timer(server.event_loop, DEBOUNCE_MS, handle_timer, userdata);
}

unsafe extern "C" fn handle_timer(userdata: *mut c_void, source: *mut c_void) {
    let server = &mut *(userdata as *mut Server);
    if server.watch_timer != source {
        return;
    }
    server.watch_timer = std::ptr::null_mut();
    oxide_event_source_remove(source);

    if server.locked {
        eprintln!(
            "0xin: config changed while the session is locked; run `0xinctl reload` \
             after unlocking"
        );
        return;
    }
    // A failure has already been reported in full; the session carries on
    // with the config it had.
    let _ = crate::reload::reload(server);
    watch(server);
}
