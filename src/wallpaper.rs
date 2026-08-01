//! Wallpapers: decode a PNG/JPEG once per output and hand it to the renderer
//! as a memory buffer.
//!
//! 0xin renders wallpapers itself rather than depending on a layer-shell
//! wallpaper client (swaybg, hyprpaper) — the phone profile has no room for an
//! extra process, and the solid `background =` colour stays the fallback when
//! no image is configured or decoding fails.

use std::env;
use std::path::PathBuf;

use image::imageops::FilterType;
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::memory::{
    MemoryRenderBuffer, MemoryRenderBufferRenderElement,
};
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::{ImportMem, Renderer};
use smithay::utils::{Logical, Scale, Size, Transform};

use crate::state::Oxin;

pub struct Wallpaper {
    buffer: MemoryRenderBuffer,
}

impl Wallpaper {
    /// Decode `path` and cover-scale it to `size` (fill the output, cropping
    /// the overflowing axis — the semantics the wlroots build had).
    pub fn load(path: &str, size: Size<i32, Logical>) -> Result<Wallpaper, String> {
        if size.w <= 0 || size.h <= 0 {
            return Err("output has no size yet".into());
        }
        let path = expand_home(path);
        let image = image::open(&path).map_err(|error| format!("{}: {error}", path.display()))?;
        let image = image.to_rgba8();

        let (iw, ih) = (image.width() as f32, image.height() as f32);
        let (ow, oh) = (size.w as f32, size.h as f32);
        // Cover: scale so both axes are at least the output's, then centre-crop.
        let factor = (ow / iw).max(oh / ih);
        let scaled = image::imageops::resize(
            &image,
            (iw * factor).ceil() as u32,
            (ih * factor).ceil() as u32,
            FilterType::Triangle,
        );
        let x = scaled.width().saturating_sub(size.w as u32) / 2;
        let y = scaled.height().saturating_sub(size.h as u32) / 2;
        let cropped =
            image::imageops::crop_imm(&scaled, x, y, size.w as u32, size.h as u32).to_image();

        let buffer = MemoryRenderBuffer::from_slice(
            cropped.as_raw(),
            // `image` gives us bytes in R,G,B,A order; Fourcc names channels
            // from the most significant byte of a little-endian word down, so
            // those same bytes are ABGR8888.
            Fourcc::Abgr8888,
            (size.w, size.h),
            1,
            Transform::Normal,
            None,
        );
        Ok(Wallpaper { buffer })
    }

    pub fn element<R>(
        &self,
        renderer: &mut R,
        _scale: Scale<f64>,
    ) -> Option<MemoryRenderBufferRenderElement<R>>
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        MemoryRenderBufferRenderElement::from_buffer(
            renderer,
            (0.0, 0.0),
            &self.buffer,
            None,
            None,
            None,
            Kind::Unspecified,
        )
        .ok()
    }
}

fn expand_home(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = env::var_os("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    PathBuf::from(path)
}

/// Apply a wallpaper (or clear it) on every output — the `0xinctl wallpaper`
/// path, and how the configured `wallpaper =` is applied at startup.
pub fn set(state: &mut Oxin, path: Option<&str>) -> Result<(), String> {
    match path {
        None => {
            for entry in state.outputs.iter_mut() {
                entry.wallpaper = None;
            }
            state.config.wallpaper = None;
            Ok(())
        }
        Some(path) => {
            // Decode for every output first: a failure aborts before anything
            // changes, so a bad path leaves the current wallpaper in place.
            let mut loaded = Vec::with_capacity(state.outputs.len());
            for entry in &state.outputs {
                loaded.push(Wallpaper::load(path, entry.geometry.size)?);
            }
            for (entry, wallpaper) in state.outputs.iter_mut().zip(loaded) {
                entry.wallpaper = Some(wallpaper);
            }
            state.config.wallpaper = Some(path.to_string());
            Ok(())
        }
    }
}

/// Give one freshly created output its wallpaper, if one is configured.
pub fn create_for_output(state: &mut Oxin, index: usize) {
    let Some(path) = state.config.wallpaper.clone() else {
        return;
    };
    let size = state.outputs[index].geometry.size;
    match Wallpaper::load(&path, size) {
        Ok(wallpaper) => state.outputs[index].wallpaper = Some(wallpaper),
        Err(error) => eprintln!("0xin: wallpaper: {error}"),
    }
}
