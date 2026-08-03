//! The nested backend: 0xin as a window inside an existing Wayland or X11
//! session. This is the fast dev loop — `cargo nested`.

use smithay::backend::allocator::dmabuf::Dmabuf;
use smithay::backend::renderer::damage::OutputDamageTracker;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::renderer::ImportDma;
use smithay::backend::winit::{self, WinitEvent, WinitGraphicsBackend};
use smithay::output::{Mode, Output, PhysicalProperties, Subpixel};
use smithay::utils::{Rectangle, Transform};

use crate::corners::Corners;
use crate::input::process_input_event;
use crate::render::{output_elements, send_frames};
use crate::state::Oxin;

pub struct WinitBackend {
    backend: WinitGraphicsBackend<GlesRenderer>,
    damage_tracker: OutputDamageTracker,
    output: Output,
    /// The rounded-corner masking program, or `None` if it failed to compile —
    /// in which case windows are drawn with square corners rather than not at
    /// all.
    corners: Option<Corners>,
}

impl WinitBackend {

    /// Lend the renderer out (screencopy re-renders an output through it).
    pub fn with_renderer<T>(
        &mut self,
        f: impl FnOnce(&mut GlesRenderer, Option<&Corners>) -> T,
    ) -> T {
        let corners = self.corners.clone();
        f(self.backend.renderer(), corners.as_ref())
    }

    pub fn import_dmabuf(&mut self, dmabuf: &Dmabuf) -> bool {
        self.backend
            .renderer()
            .import_dmabuf(dmabuf, None)
            .is_ok()
    }

    /// Draw one frame of the single nested output.
    pub fn render(&mut self, state: &mut Oxin) {
        let size = self.backend.window_size();
        let damage = Rectangle::from_size(size);

        let output = self.output.clone();
        let (elements, clear_color) = {
            let corners = self.corners.clone();
            let renderer = self.backend.renderer();
            output_elements(state, renderer, &output, corners.as_ref())
        };

        let (renderer, mut framebuffer) = match self.backend.bind() {
            Ok(bound) => bound,
            Err(error) => {
                eprintln!("0xin: cannot bind the nested window: {error}");
                return;
            }
        };
        let result =
            self.damage_tracker
                .render_output(renderer, &mut framebuffer, 0, &elements, clear_color);
        drop(framebuffer);
        match result {
            Ok(_) => {
                if let Err(error) = self.backend.submit(Some(&[damage])) {
                    eprintln!("0xin: frame submit failed: {error}");
                }
            }
            Err(error) => eprintln!("0xin: render failed: {error}"),
        }

        send_frames(state, &output, state.start_time.elapsed());
    }
}

/// Bring up the nested backend and register its event source.
pub fn init(state: &mut Oxin) -> Result<(), String> {
    let (mut backend, winit_source) =
        winit::init::<GlesRenderer>().map_err(|error| format!("winit backend: {error}"))?;

    let size = backend.window_size();
    let mode = Mode {
        size,
        refresh: 60_000,
    };
    let output = Output::new(
        "winit".to_string(),
        PhysicalProperties {
            size: (0, 0).into(),
            subpixel: Subpixel::Unknown,
            make: "0xin".into(),
            model: "nested".into(),
        },
    );
    let global = output.create_global::<Oxin>(&state.dh);
    output.change_current_state(Some(mode), Some(Transform::Flipped180), None, Some((0, 0).into()));
    output.set_preferred(mode);
    std::mem::forget(global); // the nested output lives as long as we do

    let damage_tracker = OutputDamageTracker::from_output(&output);
    crate::output::add_output(state, output.clone(), (0, 0));

    // linux-dmabuf: without this global, clients can only hand us shm buffers
    // and every GPU-rendered frame takes a copy through system memory.
    let formats: Vec<_> = backend.renderer().dmabuf_formats().into_iter().collect();
    state
        .dmabuf_state
        .create_global::<Oxin>(&state.dh, formats);

    let corners = match Corners::new(backend.renderer()) {
        Ok(corners) => Some(corners),
        Err(error) => {
            eprintln!("0xin: corner-radius shader unavailable — corner_radius will have no effect: {error}");
            None
        }
    };

    state.backend = Some(crate::backend::Backend::Winit(WinitBackend {
        backend,
        damage_tracker,
        output: output.clone(),
        corners,
    }));

    state
        .loop_handle
        .insert_source(winit_source, move |event, _, state| match event {
            WinitEvent::Resized { size, .. } => {
                let mode = Mode {
                    size,
                    refresh: 60_000,
                };
                output.change_current_state(Some(mode), None, None, None);
                output.set_preferred(mode);
                crate::output::resize_output(state, &output, size);
            }
            WinitEvent::Input(event) => process_input_event(state, event),
            WinitEvent::CloseRequested => {
                state
                    .running
                    .store(false, std::sync::atomic::Ordering::SeqCst);
            }
            WinitEvent::Redraw => {
                crate::backend::render_pending(state);
            }
            WinitEvent::Focus(_) => {}
        })
        .map_err(|error| format!("cannot register the winit event source: {error}"))?;

    Ok(())
}
