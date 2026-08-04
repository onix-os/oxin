//! Tiny line-based local control socket used by the bundled `0xinctl`.

use crate::ffi::oxide_event_loop_add_readable;
use crate::state::Server;
use crate::wallpaper;
use crate::wlr;
use std::env;
use std::fs;
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::raw::c_void;
use std::os::unix::net::UnixListener;
use std::path::PathBuf;

fn socket_path() -> Result<PathBuf, String> {
    let runtime =
        env::var_os("XDG_RUNTIME_DIR").ok_or_else(|| "XDG_RUNTIME_DIR is not set".to_string())?;
    let display = env::var("WAYLAND_DISPLAY").unwrap_or_else(|_| "default".into());
    let safe_display = display.replace('/', "_");
    Ok(PathBuf::from(runtime).join(format!("0xin-control-{safe_display}.sock")))
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
        .map_err(|error| format!("cannot make control socket nonblocking: {error}"))?;
    let fd = listener.as_raw_fd();
    server.control_listener = Some(listener);
    server.control_path = Some(path.clone());
    if oxide_event_loop_add_readable(event_loop, fd, handle_readable, userdata).is_null() {
        server.control_listener = None;
        server.control_path = None;
        fs::remove_file(&path).ok();
        return Err("cannot register control socket with Wayland event loop".into());
    }
    eprintln!("0xin: control socket = {}", path.display());
    Ok(())
}

pub(crate) fn cleanup(server: &Server) {
    if let Some(path) = &server.control_path {
        fs::remove_file(path).ok();
    }
}

unsafe extern "C" fn handle_readable(userdata: *mut c_void, _data: *mut c_void) {
    let server = &mut *(userdata as *mut Server);
    let Some(listener) = server.control_listener.as_ref() else {
        return;
    };
    let Ok((mut stream, _)) = listener.accept() else {
        return;
    };

    let mut bytes = [0u8; 16 * 1024];
    let response = match stream.read(&mut bytes) {
        Ok(0) => "error empty request\n".to_string(),
        Ok(length) => {
            let request = String::from_utf8_lossy(&bytes[..length]);
            dispatch(server, request.trim())
        }
        Err(error) => format!("error cannot read request: {error}\n"),
    };
    stream.write_all(response.as_bytes()).ok();
}

unsafe fn dispatch(server: &mut Server, request: &str) -> String {
    if server.locked {
        return "error session is locked\n".into();
    }
    if request == "quit" {
        wlr::wl_display_terminate(server.display);
        return "ok\n".into();
    }
    if let Some(argument) = request.strip_prefix("rotate ") {
        return match parse_rotate(argument) {
            Some((name, transform)) => match crate::output::rotate(server, name, transform) {
                Ok(()) => "ok\n".into(),
                Err(error) => format!("error {error}\n"),
            },
            None => "error expected `rotate NAME normal|90|180|270`\n".into(),
        };
    }
    let Some(argument) = request.strip_prefix("wallpaper ") else {
        return "error expected `quit`, `wallpaper PATH`, `wallpaper clear`, or \
                `rotate NAME normal|90|180|270`\n"
            .into();
    };
    let result = if argument == "clear" {
        wallpaper::set(server, None)
    } else if argument.is_empty() {
        Err("wallpaper path is empty".into())
    } else {
        wallpaper::set(server, Some(argument))
    };
    match result {
        Ok(()) => "ok\n".into(),
        Err(error) => format!("error {error}\n"),
    }
}

/// `NAME normal|90|180|270` -> (name, WL_OUTPUT_TRANSFORM_* value). Only the
/// four unflipped rotations are exposed here — that's all the accelerometer
/// watcher and manual testing need.
fn parse_rotate(argument: &str) -> Option<(&str, u32)> {
    let (name, transform) = argument.rsplit_once(' ')?;
    let transform = match transform {
        "normal" => 0,
        "90" => 1,
        "180" => 2,
        "270" => 3,
        _ => return None,
    };
    if name.is_empty() {
        return None;
    }
    Some((name, transform))
}
