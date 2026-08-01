//! Building one output's frame.
//!
//! The wlroots build expressed z-order as a stack of scene trees; Smithay has
//! no scene graph, so the same ordering lives here as the order elements are
//! pushed into the list (first element = topmost). Keeping it explicit in one
//! function is arguably clearer than the old eight-trees-in-`main` setup.
//!
//! Both backends render with `GlesRenderer`, so this is written against it
//! directly rather than being generic.

use std::cell::RefCell;

use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::memory::MemoryRenderBufferRenderElement;
use smithay::backend::renderer::element::solid::{SolidColorBuffer, SolidColorRenderElement};
use smithay::backend::renderer::element::surface::{
    render_elements_from_surface_tree, WaylandSurfaceRenderElement,
};
use smithay::backend::renderer::element::texture::TextureRenderElement;
use smithay::backend::renderer::element::{render_elements, Id, Kind};
use smithay::backend::renderer::gles::{GlesRenderer, GlesTexture};
use smithay::backend::renderer::utils::draw_render_elements;
use smithay::backend::renderer::{Bind, Color32F, Frame, Offscreen, Renderer};
use smithay::desktop::{layer_map_for_output, Window};
use smithay::output::Output;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Physical, Point, Rectangle, Scale, Size, Transform};
use smithay::wayland::shell::wlr_layer::Layer;

use crate::corners::{Corners, RoundedElement};
use crate::cursor::Shape;
use crate::state::{GrabMode, Oxin};
use crate::window;

render_elements! {
    pub OxinElement<=GlesRenderer>;
    Surface=WaylandSurfaceRenderElement<GlesRenderer>,
    Solid=SolidColorRenderElement,
    Memory=MemoryRenderBufferRenderElement<GlesRenderer>,
    Rounded=RoundedElement,
}

/// Everything to draw on `output`, topmost first, plus the colour to clear to.
pub fn output_elements(
    state: &Oxin,
    renderer: &mut GlesRenderer,
    output: &Output,
    corners: Option<&Corners>,
) -> (Vec<OxinElement>, [f32; 4]) {
    let scale = Scale::from(output.current_scale().fractional_scale());
    let mut elements: Vec<OxinElement> = Vec::new();

    let Some(entry) = state.output_entry(output) else {
        return (elements, clear_color(state));
    };
    let origin = entry.geometry.loc;

    // The pointer is drawn above everything, including a session lock — the
    // same place wlroots' cursor plane put it.
    if entry.geometry.contains(state.pointer_location.to_i32_round()) {
        let shape = if state.grab == GrabMode::None {
            Shape::Default
        } else {
            Shape::Grabbing
        };
        let location = state.pointer_location - origin.to_f64();
        if let Some(element) = state
            .cursor
            .borrow_mut()
            .element(renderer, shape, location, scale)
        {
            elements.push(OxinElement::Memory(element));
        }
    }

    // The session lock covers everything: the client's own surface when it has
    // one, and always an opaque compositor-owned cover underneath it — so the
    // session is visually locked even before the locker maps, and stays locked
    // if it crashes.
    if state.locked {
        if let Some(lock) = &entry.lock_surface {
            elements.extend(surface_elements(
                renderer,
                lock.wl_surface(),
                (0, 0).into(),
                scale,
            ));
        }
        elements.push(OxinElement::Solid(solid(
            Rectangle::new((0, 0).into(), entry.geometry.size),
            [0.0, 0.0, 0.0, 1.0],
            scale,
        )));
        return (elements, clear_color(state));
    }

    // The phone profile's keyboard handle sits above every shell layer.
    if let Some(handle) = state.gestures.handle_rect(entry) {
        let local = Rectangle::new(handle.loc - origin, handle.size);
        elements.push(OxinElement::Solid(solid(
            local,
            [0.85, 0.85, 0.88, 1.0],
            scale,
        )));
    }

    elements.extend(layer_elements(renderer, output, Layer::Overlay, scale));

    let workspace = &state.workspaces[entry.workspace];

    // Application windows honour `window_opacity`; layer-shell surfaces (bars,
    // panels, keyboards) stay fully opaque.
    let alpha = state.config.window_opacity;
    let radius = state.config.corner_radius;

    // Fullscreen windows paint over bars (layer top) but under overlay.
    for win in workspace
        .windows
        .iter()
        .filter(|win| window::is_fullscreen(win))
    {
        elements.extend(window_elements(renderer, win, origin, scale, alpha));
    }

    elements.extend(layer_elements(renderer, output, Layer::Top, scale));

    // Floating windows paint over tiled ones but under bars.
    for win in workspace
        .windows
        .iter()
        .filter(|win| window::is_floating(win) && !window::is_fullscreen(win))
    {
        elements.extend(masked_window(
            renderer, corners, win, origin, scale, alpha, radius,
        ));
    }

    for win in workspace.windows.iter().filter(|win| window::is_tiled(win)) {
        if workspace
            .solo
            .as_ref()
            .map(|solo| solo != win)
            .unwrap_or(false)
        {
            continue; // hidden by solo
        }
        elements.extend(masked_window(
            renderer, corners, win, origin, scale, alpha, radius,
        ));
    }

    elements.extend(layer_elements(renderer, output, Layer::Bottom, scale));
    elements.extend(layer_elements(renderer, output, Layer::Background, scale));

    // The wallpaper image, if any, sits on top of the solid clear colour.
    if let Some(wallpaper) = &entry.wallpaper {
        if let Some(element) = wallpaper.element(renderer, scale) {
            elements.push(OxinElement::Memory(element));
        }
    }

    (elements, clear_color(state))
}

fn clear_color(state: &Oxin) -> [f32; 4] {
    let (r, g, b) = state.config.background;
    [r, g, b, 1.0]
}

fn solid(
    rect: Rectangle<i32, Logical>,
    color: [f32; 4],
    scale: Scale<f64>,
) -> SolidColorRenderElement {
    let buffer = SolidColorBuffer::new(rect.size, color);
    SolidColorRenderElement::from_buffer(
        &buffer,
        rect.loc.to_physical_precise_round(scale),
        scale,
        1.0,
        Kind::Unspecified,
    )
}

/// A window, rounded if `corner_radius` asks for it.
///
/// Falls back to drawing the window directly whenever masking isn't possible —
/// no compiled program (the shader failed to build at startup) or an offscreen
/// pass that failed — so a GPU quirk costs the rounding, never the window.
fn masked_window(
    renderer: &mut GlesRenderer,
    corners: Option<&Corners>,
    win: &Window,
    origin: Point<i32, Logical>,
    scale: Scale<f64>,
    alpha: f32,
    radius: i32,
) -> Vec<OxinElement> {
    if radius > 0 {
        if let Some(corners) = corners {
            if let Some(element) =
                rounded_window(renderer, corners, win, origin, scale, alpha, radius)
            {
                return vec![OxinElement::Rounded(element)];
            }
        }
    }
    window_elements(renderer, win, origin, scale, alpha)
}

/// The offscreen texture a window is composited into before masking, kept on
/// the window so it is allocated once per size rather than once per frame.
struct CornerBuffer {
    id: Id,
    texture: GlesTexture,
    size: Size<i32, Physical>,
}

fn rounded_window(
    renderer: &mut GlesRenderer,
    corners: &Corners,
    win: &Window,
    origin: Point<i32, Logical>,
    scale: Scale<f64>,
    alpha: f32,
    radius: i32,
) -> Option<RoundedElement> {
    let geometry = window::rect(win);
    let size: Size<i32, Physical> = geometry.size.to_physical_precise_round(scale);
    if size.w <= 0 || size.h <= 0 {
        return None;
    }

    // Composite the window at the texture's origin, not at its place on screen.
    let elements = window_elements(renderer, win, geometry.loc, scale, alpha);

    let cache = win
        .user_data()
        .get_or_insert(|| RefCell::new(Option::<CornerBuffer>::None));
    let mut cache = cache.borrow_mut();
    if cache.as_ref().map(|buffer| buffer.size) != Some(size) {
        let texture = renderer
            .create_buffer(Fourcc::Abgr8888, (size.w, size.h).into())
            .ok()?;
        *cache = Some(CornerBuffer {
            id: Id::new(),
            texture,
            size,
        });
    }
    let buffer = cache.as_mut()?;

    let full = Rectangle::from_size(size);
    let mut texture = buffer.texture.clone();
    {
        let mut framebuffer = renderer.bind(&mut texture).ok()?;
        let mut frame = renderer
            .render(&mut framebuffer, size, Transform::Normal)
            .ok()?;
        // Transparent, so the corners we mask away show whatever is behind the
        // window rather than black.
        frame
            .clear(Color32F::new(0.0, 0.0, 0.0, 0.0), &[full])
            .ok()?;
        draw_render_elements::<GlesRenderer, _, _>(&mut frame, scale, &elements, &[full]).ok()?;
        // The mask pass samples this texture immediately, so the offscreen
        // render has to have landed before we hand it on.
        frame.finish().ok()?.wait().ok()?;
    }

    let location: Point<i32, Physical> = (geometry.loc - origin).to_physical_precise_round(scale);
    let element = TextureRenderElement::from_static_texture(
        buffer.id.clone(),
        renderer.context_id(),
        location.to_f64(),
        texture,
        1,
        Transform::Normal,
        None,
        None,
        Some(geometry.size),
        None,
        Kind::Unspecified,
    );
    // The radius is configured in logical pixels, so it grows with the output
    // scale like everything else.
    let radius = (radius as f64 * scale.x) as f32;
    Some(corners.mask(element, radius, (size.w as f32, size.h as f32)))
}

/// A window's own surface tree plus any popups it owns, positioned relative to
/// `origin` (our geometry is global; rendering is output- or texture-local).
fn window_elements(
    renderer: &mut GlesRenderer,
    win: &Window,
    origin: Point<i32, Logical>,
    scale: Scale<f64>,
    alpha: f32,
) -> Vec<OxinElement> {
    let location = window::rect(win).loc - origin;
    let mut elements = Vec::new();
    let Some(toplevel) = win.toplevel() else {
        return elements;
    };
    let surface = toplevel.wl_surface();

    for (popup, popup_offset) in smithay::desktop::PopupManager::popups_for_surface(surface) {
        let offset = win.geometry().loc + popup_offset - popup.geometry().loc;
        elements.extend(surface_elements_alpha(
            renderer,
            popup.wl_surface(),
            location + offset,
            scale,
            alpha,
        ));
    }
    elements.extend(surface_elements_alpha(
        renderer, surface, location, scale, alpha,
    ));
    elements
}

fn layer_elements(
    renderer: &mut GlesRenderer,
    output: &Output,
    layer: Layer,
    scale: Scale<f64>,
) -> Vec<OxinElement> {
    let map = layer_map_for_output(output);
    let mut elements = Vec::new();
    for surface in map.layers_on(layer).rev() {
        let Some(geometry) = map.layer_geometry(surface) else {
            continue;
        };
        elements.extend(surface_elements(
            renderer,
            surface.wl_surface(),
            geometry.loc,
            scale,
        ));
    }
    elements
}

fn surface_elements(
    renderer: &mut GlesRenderer,
    surface: &WlSurface,
    location: Point<i32, Logical>,
    scale: Scale<f64>,
) -> Vec<OxinElement> {
    surface_elements_alpha(renderer, surface, location, scale, 1.0)
}

fn surface_elements_alpha(
    renderer: &mut GlesRenderer,
    surface: &WlSurface,
    location: Point<i32, Logical>,
    scale: Scale<f64>,
    alpha: f32,
) -> Vec<OxinElement> {
    let physical: Point<i32, Physical> = location.to_physical_precise_round(scale);
    render_elements_from_surface_tree(renderer, surface, physical, scale, alpha, Kind::Unspecified)
        .into_iter()
        .map(OxinElement::Surface)
        .collect()
}

/// Tell every surface we just drew that it may draw again.
pub fn send_frames(state: &Oxin, output: &Output, time: std::time::Duration) {
    for ws in &state.workspaces {
        for win in &ws.windows {
            win.send_frame(output, time, None, |_, _| Some(output.clone()));
        }
    }
    let map = layer_map_for_output(output);
    for layer in map.layers() {
        layer.send_frame(output, time, None, |_, _| Some(output.clone()));
    }
    if let Some(entry) = state.output_entry(output) {
        if let Some(lock) = &entry.lock_surface {
            smithay::desktop::utils::send_frames_surface_tree(
                lock.wl_surface(),
                output,
                time,
                None,
                |_, _| Some(output.clone()),
            );
        }
    }
}
