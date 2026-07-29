//! In-compositor PNG/JPEG wallpaper decoding and wlroots scene-buffer updates.

use crate::ffi::{oxide_scene_add_wallpaper, oxide_scene_wallpaper_destroy};
use crate::state::Server;
use image::imageops::FilterType;
use std::env;
use std::path::{Path, PathBuf};
use std::ptr;

fn expand_home(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = env::var_os("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    PathBuf::from(path)
}

fn decode_cover(path: &Path, width: u32, height: u32) -> Result<Vec<u8>, String> {
    let image = image::open(path)
        .map_err(|error| format!("cannot decode {}: {error}", path.display()))?;
    Ok(image
        // Triangle gives a good wallpaper downscale without stalling the
        // compositor event loop for seconds like Lanczos can in debug/mobile
        // builds. Decoding/replacement is intentionally synchronous so the
        // control response means the new scene buffer is already installed.
        .resize_to_fill(width, height, FilterType::Triangle)
        .to_rgba8()
        .into_raw())
}

unsafe fn create_node(
    server: &Server,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    pixels: &[u8],
) -> Result<*mut std::ffi::c_void, String> {
    let node = oxide_scene_add_wallpaper(
        server.tree_bg_fallback,
        x,
        y,
        width,
        height,
        width,
        height,
        pixels.as_ptr(),
        width as usize * 4,
    );
    if node.is_null() {
        Err("wlroots could not create the wallpaper scene buffer".into())
    } else {
        Ok(node)
    }
}

/// Apply one image to every connected output using cover scaling. All images
/// decode before any current node is replaced, avoiding partially updated
/// multi-monitor state when a path is invalid.
pub(crate) unsafe fn set(server: &mut Server, requested: Option<&str>) -> Result<(), String> {
    if requested.is_none() {
        for output in &mut server.outputs {
            if !output.wallpaper.is_null() {
                oxide_scene_wallpaper_destroy(output.wallpaper);
                output.wallpaper = ptr::null_mut();
                crate::ffi::oxide_output_schedule_frame(output.wlr_output);
            }
        }
        server.config.wallpaper = None;
        return Ok(());
    }

    let requested = requested.unwrap();
    let path = expand_home(requested);
    let mut decoded = Vec::with_capacity(server.outputs.len());
    for output in &server.outputs {
        decoded.push(decode_cover(&path, output.w as u32, output.h as u32)?);
    }

    for (index, pixels) in decoded.iter().enumerate() {
        let output = &server.outputs[index];
        let new_node = create_node(
            server,
            output.x,
            output.y,
            output.w,
            output.h,
            pixels,
        )?;
        let output = &mut server.outputs[index];
        if !output.wallpaper.is_null() {
            oxide_scene_wallpaper_destroy(output.wallpaper);
        }
        output.wallpaper = new_node;
        crate::ffi::oxide_output_schedule_frame(output.wlr_output);
    }
    server.config.wallpaper = Some(requested.to_string());
    eprintln!("0xin: wallpaper = {}", path.display());
    Ok(())
}

/// Build the configured wallpaper for an output which just appeared.
pub(crate) unsafe fn create_for_output(
    server: &Server,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
) -> *mut std::ffi::c_void {
    let Some(requested) = server.config.wallpaper.as_deref() else {
        return ptr::null_mut();
    };
    let path = expand_home(requested);
    match decode_cover(&path, width as u32, height as u32)
        .and_then(|pixels| create_node(server, x, y, width, height, &pixels))
    {
        Ok(node) => node,
        Err(error) => {
            eprintln!("0xin: wallpaper: {error}; using solid background");
            ptr::null_mut()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::expand_home;
    use std::path::PathBuf;

    #[test]
    fn absolute_wallpaper_path_is_unchanged() {
        assert_eq!(
            expand_home("/tmp/wallpaper.png"),
            PathBuf::from("/tmp/wallpaper.png")
        );
    }
}
