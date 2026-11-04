//! Push-event socket: unlike `control.rs`'s one-shot request/response, a
//! client that connects here stays connected and gets a line per state
//! change (`EVENT>>DATA\n`, Hyprland `socket2` style) until it disconnects.
//! `0xinctl events` is the bundled subscriber.

use crate::ffi::oxide_event_loop_add_readable;
use crate::state::Server;
use crate::wlr;
use std::env;
use std::fs;
use std::io::{ErrorKind, Write};
use std::os::fd::AsRawFd;
use std::os::raw::c_void;
use std::os::unix::net::UnixListener;
use std::path::PathBuf;

fn socket_path() -> Result<PathBuf, String> {
    let runtime =
        env::var_os("XDG_RUNTIME_DIR").ok_or_else(|| "XDG_RUNTIME_DIR is not set".to_string())?;
    let display = env::var("WAYLAND_DISPLAY").unwrap_or_else(|_| "default".into());
    let safe_display = display.replace('/', "_");
    Ok(PathBuf::from(runtime).join(format!("0xin-events-{safe_display}.sock")))
}

pub(crate) unsafe fn setup(
    server: &mut Server,
    event_loop: *mut wlr::wl_event_loop,
    userdata: *mut c_void,
) -> Result<(), String> {
    let path = socket_path()?;
    if path.exists() {
        fs::remove_file(&path)
            .map_err(|error| format!("cannot remove stale {}: {error}", path.display()))?;
    }
    let listener = UnixListener::bind(&path)
        .map_err(|error| format!("cannot bind {}: {error}", path.display()))?;
    listener
        .set_nonblocking(true)
        .map_err(|error| format!("cannot make events socket nonblocking: {error}"))?;
    let fd = listener.as_raw_fd();
    server.event_listener = Some(listener);
    server.event_path = Some(path.clone());
    if oxide_event_loop_add_readable(event_loop, fd, handle_readable, userdata).is_null() {
        server.event_listener = None;
        server.event_path = None;
        fs::remove_file(&path).ok();
        return Err("cannot register events socket with Wayland event loop".into());
    }
    eprintln!("0xin: events socket = {}", path.display());
    Ok(())
}

pub(crate) fn cleanup(server: &Server) {
    if let Some(path) = &server.event_path {
        fs::remove_file(path).ok();
    }
}

/// Accept every connection queued since the last tick — the listener is
/// nonblocking, and several subscribers can show up in the same readable
/// event.
unsafe extern "C" fn handle_readable(userdata: *mut c_void, _data: *mut c_void) {
    let server = &mut *(userdata as *mut Server);
    let Some(listener) = server.event_listener.as_ref() else {
        return;
    };
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                if stream.set_nonblocking(true).is_ok() {
                    server.event_subscribers.push(stream);
                }
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock => break,
            Err(_) => break,
        }
    }
}

/// Push one `EVENT>>DATA` line to every subscriber. A single nonblocking
/// `write` per subscriber, not `write_all`: a partial write followed by
/// `WouldBlock` would corrupt that client's line framing, so any imperfect
/// write — including `WouldBlock` — just drops the subscriber instead of
/// risking a torn line. Lines are short, so a healthy subscriber never trips
/// this.
pub(crate) fn emit(server: &mut Server, event: &str, data: &str) {
    if server.event_subscribers.is_empty() {
        return;
    }
    let line = format!("{event}>>{data}\n");
    server
        .event_subscribers
        .retain_mut(|stream| stream.write(line.as_bytes()).map(|n| n == line.len()).unwrap_or(false));
}
