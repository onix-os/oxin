//! Building one output's frame.
//!
//! The wlroots build expressed z-order as a stack of scene trees; Smithay has
//! no scene graph, so the same ordering lives here as the order elements are
//! pushed into the list (first element = topmost). Keeping it explicit in one
//! function is arguably clearer than the old eight-trees-in-`main` setup.
//!
//! Both backends render with `GlesRenderer`, so this is written against it
//! directly rather than being generic.

use smithay::backend::renderer::element::memory::MemoryRenderBufferRenderElement;
use smithay::backend::renderer::element::solid::{SolidColorBuffer, SolidColorRenderElement};
use smithay::backend::renderer::element::surface::{
    render_elements_from_surface_tree, WaylandSurfaceRenderElement,
};
use smithay::backend::renderer::element::{render_elements, Kind};
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::desktop::{layer_map_for_output, Window};
use smithay::output::Output;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Physical, Point, Rectangle, Scale};
use smithay::wayland::shell::wlr_layer::Layer;

use crate::state::Oxin;
use crate::window;

render_elements! {
    pub OxinElement<=GlesRenderer>;
    Surface=WaylandSurfaceRenderElement<GlesRenderer>,
    Solid=SolidColorRenderElement,
    Memory=MemoryRenderBufferRenderElement<GlesRenderer>,
}

/// Everything to draw on `output`, topmost first, plus the colour to clear to.
pub fn output_elements(
    state: &Oxin,
    renderer: &mut GlesRenderer,
    output: &Output,
) -> (Vec<OxinElement>, [f32; 4]) {
    let scale = Scale::from(output.current_scale().fractional_scale());
    let mut elements: Vec<OxinElement> = Vec::new();

    let Some(entry) = state.output_entry(output) else {
        return (elements, clear_color(state));
    };
    let origin = entry.geometry.loc;

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
        elements.extend(window_elements(renderer, win, origin, scale, alpha));
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
        elements.extend(window_elements(renderer, win, origin, scale, alpha));
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
