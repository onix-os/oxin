//! Backends: what actually turns the scene into pixels and produces input.
//!
//! `winit` runs 0xin as a window inside an existing session (the fast dev
//! loop); `udev` drives DRM/KMS and libinput directly on a TTY. Both are
//! behind one enum so the rest of the compositor never has to care, the same
//! way `wlr_backend_autocreate` used to hide it.

pub mod udev;
pub mod winit;

use smithay::backend::allocator::dmabuf::Dmabuf;

use crate::state::Oxin;

pub enum Backend {
    Winit(winit::WinitBackend),
    Udev(udev::UdevBackend),
}

impl Backend {
    /// Ctrl+Alt+F<n>. A no-op when nested: there is no session to switch.
    pub fn change_vt(&mut self, vt: i32) {
        match self {
            Backend::Winit(_) => {}
            Backend::Udev(udev) => udev.change_vt(vt),
        }
    }

    /// Real DPMS on/off for one output (wlr-output-power-management). Nested,
    /// there is no such thing — the host owns the panel.
    pub fn set_output_powered(&mut self, output: &smithay::output::Output, on: bool) {
        match self {
            Backend::Winit(_) => {}
            Backend::Udev(backend) => backend.set_output_powered(output, on),
        }
    }

    pub fn import_dmabuf(&mut self, dmabuf: &Dmabuf) -> bool {
        match self {
            Backend::Winit(backend) => backend.import_dmabuf(dmabuf),
            Backend::Udev(backend) => backend.import_dmabuf(dmabuf),
        }
    }

}

/// Run `f` with the backend taken out of the state, so it can hold `&mut Oxin`
/// while it draws. The backend is always put back.
pub fn with_backend<T>(state: &mut Oxin, f: impl FnOnce(&mut Backend, &mut Oxin) -> T) -> Option<T> {
    let mut backend = state.backend.take()?;
    let result = f(&mut backend, state);
    state.backend = Some(backend);
    Some(result)
}

/// Draw whatever is due. Winit repaints every loop iteration; udev schedules
/// its own repaints from DRM vblank events, so this is a no-op there.
pub fn render_pending(state: &mut Oxin) {
    with_backend(state, |backend, state| match backend {
        Backend::Winit(winit) => winit.render(state),
        Backend::Udev(udev) => udev.render_pending(state),
    });
}
