//! The DRM/KMS backend: 0xin on a bare TTY, driving the display hardware and
//! libinput directly through a libseat session.
//!
//! Single GPU by design — the same scope the wlroots build had in practice
//! (one laptop panel, or the phone's). Render offload across several GPUs is
//! deliberately out of scope.

use std::collections::HashMap;
use std::os::fd::OwnedFd;
use std::path::Path;

use smithay::backend::allocator::dmabuf::Dmabuf;
use smithay::backend::allocator::gbm::{GbmAllocator, GbmBufferFlags, GbmDevice};
use smithay::backend::allocator::Fourcc;
use smithay::backend::drm::compositor::FrameFlags;
use smithay::backend::drm::exporter::gbm::GbmFramebufferExporter;
use smithay::backend::drm::output::{DrmOutput, DrmOutputManager, DrmOutputRenderElements};
use smithay::backend::drm::{DrmDevice, DrmDeviceFd, DrmEvent, DrmNode, NodeType};
use smithay::backend::egl::{EGLContext, EGLDisplay};
use smithay::backend::libinput::{LibinputInputBackend, LibinputSessionInterface};
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::renderer::ImportDma;
use smithay::backend::session::libseat::LibSeatSession;
use smithay::backend::session::{Event as SessionEvent, Session};
use smithay::backend::udev::{all_gpus, primary_gpu, UdevBackend as UdevScanner, UdevEvent};
use smithay::output::{Mode as OutputMode, Output, PhysicalProperties, Subpixel};
use smithay::reexports::drm::control::{connector, crtc, Device as ControlDevice, ModeTypeFlags};
use smithay::reexports::input::Libinput;
use smithay::reexports::rustix::fs::OFlags;
use smithay::utils::DeviceFd;

use crate::input::process_input_event;
use crate::render::{output_elements, send_frames};
use crate::state::Oxin;

type OxinAllocator = GbmAllocator<DrmDeviceFd>;
type OxinExporter = GbmFramebufferExporter<DrmDeviceFd>;
type OxinDrmOutput = DrmOutput<OxinAllocator, OxinExporter, (), DrmDeviceFd>;
type OxinDrmOutputManager = DrmOutputManager<OxinAllocator, OxinExporter, (), DrmDeviceFd>;

pub struct UdevBackend {
    session: LibSeatSession,
    gpu: Option<Gpu>,
}

struct Gpu {
    manager: OxinDrmOutputManager,
    renderer: GlesRenderer,
    surfaces: HashMap<crtc::Handle, Surface>,
}

struct Surface {
    output: Output,
    drm_output: OxinDrmOutput,
    queued: bool,
}

impl UdevBackend {

    pub fn change_vt(&mut self, vt: i32) {
        if let Err(error) = self.session.change_vt(vt) {
            eprintln!("0xin: cannot switch to VT {vt}: {error}");
        }
    }

    pub fn import_dmabuf(&mut self, dmabuf: &Dmabuf) -> bool {
        match self.gpu.as_mut() {
            Some(gpu) => gpu.renderer.import_dmabuf(dmabuf, None).is_ok(),
            None => false,
        }
    }

    /// Draw every output that is not already waiting on a page flip.
    pub fn render_pending(&mut self, state: &mut Oxin) {
        let Some(gpu) = self.gpu.as_mut() else {
            return;
        };
        if !self.session.is_active() {
            return;
        }
        let crtcs: Vec<crtc::Handle> = gpu.surfaces.keys().copied().collect();
        for crtc in crtcs {
            let Some(surface) = gpu.surfaces.get_mut(&crtc) else {
                continue;
            };
            if surface.queued {
                continue;
            }
            let output = surface.output.clone();
            let (elements, clear_color) = output_elements(state, &mut gpu.renderer, &output);

            let result = surface.drm_output.render_frame(
                &mut gpu.renderer,
                &elements,
                clear_color,
                FrameFlags::DEFAULT,
            );
            match result {
                Ok(frame) => {
                    if !frame.is_empty {
                        if let Err(error) = surface.drm_output.queue_frame(()) {
                            eprintln!("0xin: cannot queue frame: {error}");
                        } else {
                            surface.queued = true;
                        }
                    }
                }
                Err(error) => eprintln!("0xin: render failed: {error}"),
            }
            send_frames(state, &output, state.start_time.elapsed());
        }
    }
}

/// Bring up the session, libinput and the primary GPU.
pub fn init(state: &mut Oxin) -> Result<(), String> {
    let (session, session_notifier) =
        LibSeatSession::new().map_err(|error| format!("cannot open a libseat session: {error}"))?;
    let seat_name = session.seat();

    // libinput, sharing the session so device fds survive VT switches.
    let mut libinput = Libinput::new_with_udev::<LibinputSessionInterface<LibSeatSession>>(
        session.clone().into(),
    );
    libinput
        .udev_assign_seat(&seat_name)
        .map_err(|_| format!("cannot assign libinput to seat {seat_name}"))?;
    let libinput_backend = LibinputInputBackend::new(libinput.clone());
    state
        .loop_handle
        .insert_source(libinput_backend, move |event, _, state| {
            process_input_event(state, event);
        })
        .map_err(|error| format!("cannot register libinput: {error}"))?;

    state.backend = Some(crate::backend::Backend::Udev(UdevBackend {
        session: session.clone(),
        gpu: None,
    }));

    // VT switches: libinput and the DRM device have to be paused and resumed
    // in step with the session, and everything repainted on the way back.
    let mut libinput_for_session = libinput.clone();
    state
        .loop_handle
        .insert_source(session_notifier, move |event, _, state| match event {
            SessionEvent::PauseSession => {
                libinput_for_session.suspend();
                if let Some(crate::backend::Backend::Udev(udev)) = state.backend.as_mut() {
                    if let Some(gpu) = udev.gpu.as_mut() {
                        gpu.manager.pause();
                    }
                }
                eprintln!("0xin: session inactive (VT switched away)");
            }
            SessionEvent::ActivateSession => {
                if libinput_for_session.resume().is_err() {
                    eprintln!("0xin: cannot resume libinput");
                }
                if let Some(crate::backend::Backend::Udev(udev)) = state.backend.as_mut() {
                    if let Some(gpu) = udev.gpu.as_mut() {
                        if let Err(error) = gpu.manager.activate(true) {
                            eprintln!("0xin: cannot reactivate DRM: {error}");
                        }
                        for surface in gpu.surfaces.values_mut() {
                            surface.queued = false;
                        }
                    }
                }
                eprintln!("0xin: session active — repainting");
            }
        })
        .map_err(|error| format!("cannot register the session notifier: {error}"))?;

    // The primary GPU, or the first one udev knows about.
    let path = primary_gpu(&seat_name)
        .ok()
        .flatten()
        .or_else(|| {
            all_gpus(&seat_name)
                .ok()
                .and_then(|gpus| gpus.into_iter().next())
        })
        .ok_or_else(|| "no GPU found for this seat".to_string())?;

    eprintln!("0xin: seat {seat_name}, GPU {}", path.display());
    let gpu_dev_id = open_gpu(state, &path)?;

    // Hotplug: we only care about the GPU we already opened, but keeping the
    // scanner alive means a later `device_added` for it is not missed.
    let scanner = UdevScanner::new(&seat_name)
        .map_err(|error| format!("cannot start the udev scanner: {error}"))?;
    state
        .loop_handle
        .insert_source(scanner, move |event, _, state| match event {
            UdevEvent::Removed { device_id } if device_id == gpu_dev_id => {
                // The GPU went away (rare outside of eGPU unplugs, but it also
                // covers the device disappearing on shutdown): drop its outputs
                // so nothing keeps rendering to them.
                let outputs: Vec<_> = state
                    .outputs
                    .iter()
                    .map(|entry| entry.output.clone())
                    .collect();
                for output in outputs {
                    crate::output::remove_output(state, &output);
                }
                if let Some(crate::backend::Backend::Udev(udev)) = state.backend.as_mut() {
                    udev.gpu = None;
                }
            }
            UdevEvent::Added { .. } | UdevEvent::Changed { .. } | UdevEvent::Removed { .. } => {}
        })
        .map_err(|error| format!("cannot register udev: {error}"))?;

    Ok(())
}

fn open_gpu(state: &mut Oxin, path: &Path) -> Result<u64, String> {
    let Some(crate::backend::Backend::Udev(udev)) = state.backend.as_mut() else {
        return Err("udev backend missing".into());
    };
    let fd: OwnedFd = udev
        .session
        .open(
            path,
            OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOCTTY | OFlags::NONBLOCK,
        )
        .map_err(|error| format!("cannot open {}: {error}", path.display()))?;
    let device_fd = DrmDeviceFd::new(DeviceFd::from(fd));
    let node = DrmNode::from_file(&device_fd)
        .map_err(|error| format!("{} is not a DRM node: {error}", path.display()))?;
    let node = node
        .node_with_type(NodeType::Render)
        .and_then(|node| node.ok())
        .unwrap_or(node);

    let (drm, drm_notifier) = DrmDevice::new(device_fd.clone(), true)
        .map_err(|error| format!("cannot open the DRM device: {error}"))?;
    let gbm = GbmDevice::new(device_fd)
        .map_err(|error| format!("cannot create a GBM device: {error}"))?;

    let egl_display = unsafe { EGLDisplay::new(gbm.clone()) }
        .map_err(|error| format!("cannot create an EGL display: {error}"))?;
    let egl_context = EGLContext::new(&egl_display)
        .map_err(|error| format!("cannot create an EGL context: {error}"))?;
    let render_formats = egl_context.dmabuf_render_formats().clone();
    let renderer = unsafe { GlesRenderer::new(egl_context) }
        .map_err(|error| format!("cannot create the GLES renderer: {error}"))?;

    let allocator = GbmAllocator::new(gbm.clone(), GbmBufferFlags::RENDERING);
    let manager = DrmOutputManager::new(
        drm,
        allocator,
        GbmFramebufferExporter::new(gbm.clone(), Some(node)),
        Some(gbm),
        [Fourcc::Argb8888, Fourcc::Xrgb8888],
        render_formats,
    );

    let Some(crate::backend::Backend::Udev(udev)) = state.backend.as_mut() else {
        return Err("udev backend missing".into());
    };
    let dev_id = node.dev_id();
    udev.gpu = Some(Gpu {
        manager,
        renderer,
        surfaces: HashMap::new(),
    });

    // linux-dmabuf, advertising this GPU's node so clients allocate on the
    // device we actually scan out from.
    let formats: Vec<_> = {
        let Some(crate::backend::Backend::Udev(udev)) = state.backend.as_mut() else {
            return Err("udev backend missing".into());
        };
        let gpu = udev.gpu.as_mut().expect("just installed");
        gpu.renderer.dmabuf_formats().into_iter().collect()
    };
    state
        .dmabuf_state
        .create_global::<Oxin>(&state.dh, formats);

    scan_connectors(state)?;

    // Page-flip completions: the hardware is done with the frame we queued, so
    // the next one may be drawn.
    state
        .loop_handle
        .insert_source(drm_notifier, move |event, metadata, state| match event {
            DrmEvent::VBlank(crtc) => {
                if let Some(crate::backend::Backend::Udev(udev)) = state.backend.as_mut() {
                    if let Some(gpu) = udev.gpu.as_mut() {
                        if let Some(surface) = gpu.surfaces.get_mut(&crtc) {
                            let _ = metadata;
                            if let Err(error) = surface.drm_output.frame_submitted() {
                                eprintln!("0xin: frame submission failed: {error}");
                            }
                            surface.queued = false;
                        }
                    }
                }
            }
            DrmEvent::Error(error) => eprintln!("0xin: DRM error: {error}"),
        })
        .map_err(|error| format!("cannot register the DRM device: {error}"))?;

    Ok(dev_id)
}

/// Turn every connected connector into an output.
fn scan_connectors(state: &mut Oxin) -> Result<(), String> {
    let Some(crate::backend::Backend::Udev(udev)) = state.backend.as_mut() else {
        return Err("udev backend missing".into());
    };
    let Some(gpu) = udev.gpu.as_mut() else {
        return Err("no GPU opened".into());
    };

    let resources = gpu
        .manager
        .device()
        .resource_handles()
        .map_err(|error| format!("cannot read DRM resources: {error}"))?;

    let mut created: Vec<(crtc::Handle, Output, i32)> = Vec::new();
    let mut next_x = 0;

    for handle in resources.connectors() {
        let Ok(connector) = gpu.manager.device().get_connector(*handle, false) else {
            continue;
        };
        if connector.state() != connector::State::Connected {
            continue;
        }
        let Some(mode) = connector
            .modes()
            .iter()
            .find(|mode| mode.mode_type().contains(ModeTypeFlags::PREFERRED))
            .or_else(|| connector.modes().first())
            .copied()
        else {
            continue;
        };

        // A free CRTC that can drive this connector.
        let encoders: Vec<_> = connector
            .encoders()
            .iter()
            .filter_map(|handle| gpu.manager.device().get_encoder(*handle).ok())
            .collect();
        let Some(crtc) = encoders
            .iter()
            .flat_map(|encoder| resources.filter_crtcs(encoder.possible_crtcs()))
            .find(|crtc| !gpu.surfaces.contains_key(crtc))
        else {
            continue;
        };

        let name = format!(
            "{}-{}",
            connector.interface().as_str(),
            connector.interface_id()
        );
        let (physical_w, physical_h) = connector.size().unwrap_or((0, 0));
        let output = Output::new(
            name.clone(),
            PhysicalProperties {
                size: (physical_w as i32, physical_h as i32).into(),
                subpixel: Subpixel::Unknown,
                make: "0xin".into(),
                model: "drm".into(),
            },
        );
        let output_mode = OutputMode {
            size: (mode.size().0 as i32, mode.size().1 as i32).into(),
            refresh: (mode.vrefresh() * 1000) as i32,
        };
        output.change_current_state(Some(output_mode), None, None, None);
        output.set_preferred(output_mode);
        let global = output.create_global::<Oxin>(&state.dh);
        std::mem::forget(global);

        let elements: DrmOutputRenderElements<GlesRenderer, crate::render::OxinElement> =
            DrmOutputRenderElements::default();
        let drm_output = match gpu.manager.initialize_output(
            crtc,
            mode,
            &[connector.handle()],
            &output,
            None,
            &mut gpu.renderer,
            &elements,
        ) {
            Ok(drm_output) => drm_output,
            Err(error) => {
                eprintln!("0xin: cannot bring up {name}: {error}");
                continue;
            }
        };

        gpu.surfaces.insert(
            crtc,
            Surface {
                output: output.clone(),
                drm_output,
                queued: false,
            },
        );
        created.push((crtc, output, next_x));
        next_x += output_mode.size.w;
    }

    if created.is_empty() {
        return Err("no connected outputs".into());
    }

    for (_, output, x) in created {
        crate::output::add_output(state, output, (x, 0));
    }
    Ok(())
}
