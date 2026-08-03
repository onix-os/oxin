//! wlr-screencopy-unstable-v1: let clients capture our composited output.
//!
//! wlroots implemented this entirely inside the library — the wlroots build
//! only had to create the global. Smithay ships no screencopy, so the wire
//! protocol and the actual read-back live here: we re-render the requested
//! output into an offscreen texture, read it back with `ExportMem`, and memcpy
//! it into the client's shm buffer. That is what `grim` and `wf-recorder` use.
//!
//! Shared-memory buffers only. wlroots also offered dma-buf capture; clients
//! that ask for it fall back to shm, which is what grim does anyway.

use std::sync::Mutex;

use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::gles::{GlesRenderer, GlesTexture};
use smithay::backend::renderer::{Bind, ExportMem, Frame, Offscreen, Renderer};
use smithay::output::Output;
use smithay::reexports::wayland_protocols_wlr::screencopy::v1::server::{
    zwlr_screencopy_frame_v1::{self, ZwlrScreencopyFrameV1},
    zwlr_screencopy_manager_v1::{self, ZwlrScreencopyManagerV1},
};
use smithay::reexports::wayland_server::protocol::wl_shm;
use smithay::reexports::wayland_server::{
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource,
};
use smithay::utils::{Physical, Rectangle, Size, Transform};
use smithay::wayland::shm::with_buffer_contents_mut;

use crate::backend::Backend;
use crate::render::output_elements;
use crate::state::Oxin;

/// What one in-flight `zwlr_screencopy_frame_v1` is capturing.
pub struct FrameState {
    output: Output,
    /// Capture region, in output-local physical pixels.
    region: Mutex<Rectangle<i32, Physical>>,
    overlay_cursor: bool,
}

pub struct ScreencopyManagerState;

impl ScreencopyManagerState {
    pub fn new(display: &DisplayHandle) -> Self {
        display.create_global::<Oxin, ZwlrScreencopyManagerV1, _>(3, ());
        ScreencopyManagerState
    }
}

impl GlobalDispatch<ZwlrScreencopyManagerV1, ()> for Oxin {
    fn bind(
        _state: &mut Self,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<ZwlrScreencopyManagerV1>,
        _global_data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }
}

impl Dispatch<ZwlrScreencopyManagerV1, ()> for Oxin {
    fn request(
        state: &mut Self,
        _client: &Client,
        _resource: &ZwlrScreencopyManagerV1,
        request: zwlr_screencopy_manager_v1::Request,
        _data: &(),
        _handle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        let (frame, output, region, overlay_cursor) = match request {
            zwlr_screencopy_manager_v1::Request::CaptureOutput {
                frame,
                overlay_cursor,
                output,
            } => {
                let Some(output) = Output::from_resource(&output) else {
                    let frame = data_init.init(frame, dead_frame_state());
                    frame.failed();
                    return;
                };
                let size = output_size(state, &output);
                (
                    frame,
                    output,
                    Rectangle::from_size(size),
                    overlay_cursor != 0,
                )
            }
            zwlr_screencopy_manager_v1::Request::CaptureOutputRegion {
                frame,
                overlay_cursor,
                output,
                x,
                y,
                width,
                height,
            } => {
                let Some(output) = Output::from_resource(&output) else {
                    let frame = data_init.init(frame, dead_frame_state());
                    frame.failed();
                    return;
                };
                // The protocol's region is in logical coordinates, relative to
                // the output layout; ours is output-local physical.
                let scale = output.current_scale().fractional_scale();
                let entry_loc = state
                    .output_entry(&output)
                    .map(|entry| entry.geometry.loc)
                    .unwrap_or_default();
                let local = Rectangle::new(
                    ((x - entry_loc.x) as f64 * scale, (y - entry_loc.y) as f64 * scale).into(),
                    ((width as f64 * scale), (height as f64 * scale)).into(),
                )
                .to_i32_round();
                let full = Rectangle::from_size(output_size(state, &output));
                let region = local.intersection(full).unwrap_or(full);
                (frame, output, region, overlay_cursor != 0)
            }
            zwlr_screencopy_manager_v1::Request::Destroy => return,
            _ => return,
        };

        let frame = data_init.init(
            frame,
            FrameState {
                output,
                region: Mutex::new(region),
                overlay_cursor,
            },
        );
        // Xrgb8888 is what our renderer reads back, and what grim expects.
        let stride = region.size.w as u32 * 4;
        frame.buffer(
            wl_shm::Format::Xrgb8888,
            region.size.w as u32,
            region.size.h as u32,
            stride,
        );
        if frame.version() >= 3 {
            frame.buffer_done();
        }
    }
}

impl Dispatch<ZwlrScreencopyFrameV1, FrameState> for Oxin {
    fn request(
        state: &mut Self,
        _client: &Client,
        resource: &ZwlrScreencopyFrameV1,
        request: zwlr_screencopy_frame_v1::Request,
        data: &FrameState,
        _handle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        let (buffer, with_damage) = match request {
            zwlr_screencopy_frame_v1::Request::Copy { buffer } => (buffer, false),
            zwlr_screencopy_frame_v1::Request::CopyWithDamage { buffer } => (buffer, true),
            zwlr_screencopy_frame_v1::Request::Destroy => return,
            _ => return,
        };

        let region = *data.region.lock().unwrap();
        match capture(state, &data.output, region, data.overlay_cursor, &buffer) {
            Ok(()) => {
                // No transform was applied on the way out.
                resource.flags(zwlr_screencopy_frame_v1::Flags::empty());
                if with_damage {
                    resource.damage(0, 0, region.size.w as u32, region.size.h as u32);
                }
                let time: std::time::Duration = state.clock.now().into();
                let secs = time.as_secs();
                resource.ready((secs >> 32) as u32, secs as u32, time.subsec_nanos());
            }
            Err(error) => {
                eprintln!("0xin: screencopy failed: {error}");
                resource.failed();
            }
        }
    }
}

/// Re-render `output` into an offscreen texture and copy `region` of it into
/// the client's shm buffer.
fn capture(
    state: &mut Oxin,
    output: &Output,
    region: Rectangle<i32, Physical>,
    overlay_cursor: bool,
    buffer: &smithay::reexports::wayland_server::protocol::wl_buffer::WlBuffer,
) -> Result<(), String> {
    if region.size.w <= 0 || region.size.h <= 0 {
        return Err("empty capture region".into());
    }

    let mut backend = state.backend.take().ok_or("no backend")?;
    let result = with_renderer(&mut backend, |renderer, corners| {
        let size = output_size(state, output);
        let (elements, clear_color) = output_elements(state, renderer, output, corners);
        // `output_elements` puts the cursor first; drop it unless the client
        // asked for it, exactly like wlroots' overlay_cursor flag.
        let elements = if overlay_cursor {
            elements
        } else {
            elements.into_iter().skip(cursor_elements(state, output)).collect()
        };

        let mut texture: GlesTexture = renderer
            .create_buffer(Fourcc::Abgr8888, (size.w, size.h).into())
            .map_err(|error| format!("cannot allocate a capture buffer: {error}"))?;
        let mapping = {
            let mut framebuffer = renderer
                .bind(&mut texture)
                .map_err(|error| format!("cannot bind the capture buffer: {error}"))?;
            {
                let mut frame = renderer
                    .render(&mut framebuffer, size, Transform::Normal)
                    .map_err(|error| format!("cannot start the capture frame: {error}"))?;
                frame
                    .clear(clear_color.into(), &[Rectangle::from_size(size)])
                    .map_err(|error| format!("cannot clear the capture frame: {error}"))?;
                smithay::backend::renderer::utils::draw_render_elements::<GlesRenderer, _, _>(
                    &mut frame,
                    output.current_scale().fractional_scale(),
                    &elements,
                    &[Rectangle::from_size(size)],
                )
                .map_err(|error| format!("cannot draw the capture frame: {error}"))?;
                frame
                    .finish()
                    .map_err(|error| format!("cannot finish the capture frame: {error}"))?
                    .wait()
                    .map_err(|error| format!("capture frame never completed: {error}"))?;
            }
            renderer
                .copy_framebuffer(
                    &framebuffer,
                    // The region is already in physical pixels; the buffer
                    // coordinate space is the same 1:1 grid here.
                    Rectangle::new(
                        (region.loc.x, region.loc.y).into(),
                        (region.size.w, region.size.h).into(),
                    ),
                    Fourcc::Xrgb8888,
                )
                .map_err(|error| format!("cannot read back the capture: {error}"))?
        };

        let pixels = renderer
            .map_texture(&mapping)
            .map_err(|error| format!("cannot map the capture: {error}"))?;

        with_buffer_contents_mut(buffer, |target, len, data| {
            if data.format != wl_shm::Format::Xrgb8888 && data.format != wl_shm::Format::Argb8888 {
                return Err("client buffer has an unsupported format".to_string());
            }
            let stride = data.stride as usize;
            let rows = region.size.h as usize;
            let row_bytes = region.size.w as usize * 4;
            if len < stride * rows || pixels.len() < row_bytes * rows {
                return Err("client buffer is too small".to_string());
            }
            for row in 0..rows {
                // SAFETY: bounds checked directly above; the pool is locked
                // for the duration of this callback.
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        pixels.as_ptr().add(row * row_bytes),
                        target.add(row * stride),
                        row_bytes,
                    );
                }
            }
            Ok(())
        })
        .map_err(|error| format!("cannot access the client buffer: {error}"))?
    });
    state.backend = Some(backend);
    result
}

/// How many leading elements `output_elements` produced for the cursor.
fn cursor_elements(state: &Oxin, output: &Output) -> usize {
    let on_output = state
        .output_entry(output)
        .map(|entry| entry.geometry.contains(state.pointer_location.to_i32_round()))
        .unwrap_or(false);
    usize::from(on_output)
}

fn output_size(state: &Oxin, output: &Output) -> Size<i32, Physical> {
    let scale = output.current_scale().fractional_scale();
    state
        .output_entry(output)
        .map(|entry| entry.geometry.size.to_physical_precise_round(scale))
        .unwrap_or_default()
}

/// Run `f` with whichever backend's renderer is live.
fn with_renderer<T>(
    backend: &mut Backend,
    f: impl FnOnce(&mut GlesRenderer, Option<&crate::corners::Corners>) -> T,
) -> T {
    match backend {
        Backend::Winit(winit) => winit.with_renderer(f),
        Backend::Udev(udev) => udev.with_renderer(f),
    }
}

/// A frame we are about to fail: the protocol still requires an object.
fn dead_frame_state() -> FrameState {
    FrameState {
        output: Output::new(
            "dead".into(),
            smithay::output::PhysicalProperties {
                size: (0, 0).into(),
                subpixel: smithay::output::Subpixel::Unknown,
                make: "0xin".into(),
                model: "none".into(),
            },
        ),
        region: Mutex::new(Rectangle::default()),
        overlay_cursor: false,
    }
}
