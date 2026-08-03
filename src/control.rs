//! Tiny line-based local control socket used by the bundled `0xinctl`.

use std::env;
use std::fs;
use std::io::{Read, Write};
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::sync::atomic::Ordering;

use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::{Interest, Mode, PostAction};

use crate::state::Oxin;
use crate::wallpaper;

fn socket_path() -> Result<PathBuf, String> {
    let runtime =
        env::var_os("XDG_RUNTIME_DIR").ok_or_else(|| "XDG_RUNTIME_DIR is not set".to_string())?;
    let display = env::var("WAYLAND_DISPLAY").unwrap_or_else(|_| "default".into());
    let safe_display = display.replace('/', "_");
    Ok(PathBuf::from(runtime).join(format!("0xin-control-{safe_display}.sock")))
}

pub fn setup(state: &mut Oxin) -> Result<(), String> {
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

    let source = Generic::new(listener, Interest::READ, Mode::Level);
    state
        .loop_handle
        .insert_source(source, |_, listener, state| {
            // Safety: the source is level-triggered on a nonblocking listener,
            // so accept() either yields a connection or WouldBlock.
            while let Ok((mut stream, _)) = listener.accept() {
                let mut bytes = [0u8; 16 * 1024];
                let response = match stream.read(&mut bytes) {
                    Ok(0) => "error empty request\n".to_string(),
                    Ok(length) => {
                        let request = String::from_utf8_lossy(&bytes[..length]);
                        dispatch(state, request.trim())
                    }
                    Err(error) => format!("error cannot read request: {error}\n"),
                };
                stream.write_all(response.as_bytes()).ok();
            }
            Ok(PostAction::Continue)
        })
        .map_err(|error| format!("cannot register control socket: {error}"))?;

    state.control_path = Some(path.clone());
    eprintln!("0xin: control socket = {}", path.display());
    Ok(())
}

pub fn cleanup(state: &Oxin) {
    if let Some(path) = &state.control_path {
        fs::remove_file(path).ok();
    }
}

fn dispatch(state: &mut Oxin, request: &str) -> String {
    if state.locked {
        return "error session is locked\n".into();
    }
    if request == "quit" {
        state.running.store(false, Ordering::SeqCst);
        return "ok\n".into();
    }
    let Some(argument) = request.strip_prefix("wallpaper ") else {
        return "error expected `quit`, `wallpaper PATH`, or `wallpaper clear`\n".into();
    };
    let result = if argument == "clear" {
        wallpaper::set(state, None)
    } else if argument.is_empty() {
        Err("wallpaper path is empty".into())
    } else {
        wallpaper::set(state, Some(argument))
    };
    match result {
        Ok(()) => "ok\n".into(),
        Err(error) => format!("error {error}\n"),
    }
}
