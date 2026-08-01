//! wlr-output-power-management-unstable-v1: real DPMS on/off per output.
//!
//! wlroots implemented the whole wire protocol for us and only signalled
//! `set_mode`; Smithay ships no such protocol, so the wire handling lives here
//! and the actual power switch happens in the DRM backend. Behaviour matches
//! the wlroots build: apply the mode, and when an output comes back on force a
//! repaint, because a re-enabled output's damage tracking won't re-present
//! idle windows on its own.

use std::sync::Mutex;

use smithay::output::Output;
use smithay::reexports::wayland_protocols_wlr::output_power_management::v1::server::{
    zwlr_output_power_manager_v1::{self, ZwlrOutputPowerManagerV1},
    zwlr_output_power_v1::{self, Mode, ZwlrOutputPowerV1},
};
use smithay::reexports::wayland_server::{
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New,
};

use crate::state::Oxin;

/// Per-`zwlr_output_power_v1` state: which output it controls.
pub struct OutputPowerState {
    output: Mutex<Option<Output>>,
}

pub struct OutputPowerManagerState;

impl OutputPowerManagerState {
    pub fn new(display: &DisplayHandle) -> Self {
        display.create_global::<Oxin, ZwlrOutputPowerManagerV1, _>(1, ());
        OutputPowerManagerState
    }
}

impl GlobalDispatch<ZwlrOutputPowerManagerV1, ()> for Oxin {
    fn bind(
        _state: &mut Self,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<ZwlrOutputPowerManagerV1>,
        _global_data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }
}

impl Dispatch<ZwlrOutputPowerManagerV1, ()> for Oxin {
    fn request(
        state: &mut Self,
        _client: &Client,
        _resource: &ZwlrOutputPowerManagerV1,
        request: zwlr_output_power_manager_v1::Request,
        _data: &(),
        _handle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            zwlr_output_power_manager_v1::Request::GetOutputPower { id, output } => {
                let output = Output::from_resource(&output);
                let control = data_init.init(
                    id,
                    OutputPowerState {
                        output: Mutex::new(output.clone()),
                    },
                );
                match output {
                    // The client learns the current mode immediately, per the
                    // protocol; we only ever power outputs we know about.
                    Some(output) => {
                        let on = state.powered.get(&output.name()).copied().unwrap_or(true);
                        control.mode(if on { Mode::On } else { Mode::Off });
                    }
                    None => control.failed(),
                }
            }
            zwlr_output_power_manager_v1::Request::Destroy => {}
            _ => {}
        }
    }
}

impl Dispatch<ZwlrOutputPowerV1, OutputPowerState> for Oxin {
    fn request(
        state: &mut Self,
        _client: &Client,
        resource: &ZwlrOutputPowerV1,
        request: zwlr_output_power_v1::Request,
        data: &OutputPowerState,
        _handle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            zwlr_output_power_v1::Request::SetMode { mode } => {
                let Some(output) = data.output.lock().unwrap().clone() else {
                    resource.failed();
                    return;
                };
                let on = matches!(mode.into_result(), Ok(Mode::On));
                set_powered(state, &output, on);
                resource.mode(if on { Mode::On } else { Mode::Off });
            }
            zwlr_output_power_v1::Request::Destroy => {}
            _ => {}
        }
    }
}

/// Apply a power mode to one output, and repaint it on the way back on.
fn set_powered(state: &mut Oxin, output: &Output, on: bool) {
    if state.output_entry(output).is_none() {
        eprintln!("0xin: output-power set_mode targeted an unknown output");
        return;
    }
    if let Some(backend) = state.backend.as_mut() {
        backend.set_output_powered(output, on);
    }
    state.powered.insert(output.name(), on);
    eprintln!(
        "0xin: output {} powered {}",
        output.name(),
        if on { "on" } else { "off" }
    );
}
