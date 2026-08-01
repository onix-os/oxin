//! The pointer cursor.
//!
//! The wlroots build used `wlr_xcursor_manager` at size 24 with the default
//! theme, and drew exactly two shapes: `default`, and `grabbing` while a
//! Mod+drag was in progress. It never honoured a client's own `set_cursor`
//! request. This reproduces that, loading the theme ourselves because Smithay
//! leaves cursor imagery to the compositor.

use std::collections::HashMap;
use std::env;

use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::memory::{
    MemoryRenderBuffer, MemoryRenderBufferRenderElement,
};
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::utils::{Logical, Physical, Point, Scale, Transform};
use xcursor::parser::{parse_xcursor, Image};
use xcursor::CursorTheme;

/// The size the wlroots build asked its xcursor manager for.
const CURSOR_SIZE: u32 = 24;

pub struct Cursor {
    /// One decoded image per shape, at the size closest to what we asked for.
    shapes: HashMap<Shape, Image>,
    /// Buffers are per (shape, scale), built on first use and kept after.
    buffers: HashMap<(Shape, u32), MemoryRenderBuffer>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Shape {
    Default,
    Grabbing,
}

impl Shape {
    fn icon(self) -> &'static str {
        match self {
            Shape::Default => "default",
            Shape::Grabbing => "grabbing",
        }
    }
}

impl Cursor {
    /// Load the user's theme (`XCURSOR_THEME`, else the theme named `default`)
    /// at `XCURSOR_SIZE`, else 24.
    pub fn load() -> Cursor {
        let name = env::var("XCURSOR_THEME").unwrap_or_else(|_| "default".into());
        let size = env::var("XCURSOR_SIZE")
            .ok()
            .and_then(|size| size.parse::<u32>().ok())
            .unwrap_or(CURSOR_SIZE);
        let theme = CursorTheme::load(&name);

        let mut shapes = HashMap::new();
        for shape in [Shape::Default, Shape::Grabbing] {
            match load_shape(&theme, shape, size) {
                Some(image) => {
                    shapes.insert(shape, image);
                }
                None => eprintln!(
                    "0xin: cursor theme `{name}` has no `{}` shape",
                    shape.icon()
                ),
            }
        }
        if shapes.is_empty() {
            eprintln!("0xin: no usable cursor theme — the pointer will be invisible");
        }

        Cursor {
            shapes,
            buffers: HashMap::new(),
        }
    }

    /// The element to draw for the pointer at `location` (output-local,
    /// logical), hotspot applied.
    pub fn element(
        &mut self,
        renderer: &mut GlesRenderer,
        shape: Shape,
        location: Point<f64, Logical>,
        scale: Scale<f64>,
    ) -> Option<MemoryRenderBufferRenderElement<GlesRenderer>> {
        // Cursor images are integer-scaled, like every other client buffer.
        let buffer_scale = scale.x.max(1.0).round() as u32;
        let image = self.shapes.get(&shape)?.clone();

        let buffer = self
            .buffers
            .entry((shape, buffer_scale))
            .or_insert_with(|| {
                MemoryRenderBuffer::from_slice(
                    &image.pixels_rgba,
                    Fourcc::Argb8888,
                    (image.width as i32, image.height as i32),
                    buffer_scale as i32,
                    Transform::Normal,
                    None,
                )
            });

        // The hotspot is in image pixels; the element is placed by its
        // top-left corner, so shift the pointer position back by it.
        let hotspot = Point::<f64, Logical>::from((
            image.xhot as f64 / buffer_scale as f64,
            image.yhot as f64 / buffer_scale as f64,
        ));
        let position: Point<f64, Physical> = (location - hotspot).to_physical(scale);

        MemoryRenderBufferRenderElement::from_buffer(
            renderer,
            position,
            buffer,
            None,
            None,
            None,
            Kind::Cursor,
        )
        .ok()
    }
}

/// Pick the image closest to the requested size, the way an xcursor manager
/// would. Animated cursors keep their first frame — 0xin never draws a busy
/// or wait cursor, so nothing here animates.
fn load_shape(theme: &CursorTheme, shape: Shape, size: u32) -> Option<Image> {
    let path = theme.load_icon(shape.icon())?;
    let bytes = std::fs::read(path).ok()?;
    let images = parse_xcursor(&bytes)?;
    images
        .into_iter()
        .min_by_key(|image| image.size.abs_diff(size))
}
