//! wlr-output-power-management-unstable-v1: real DPMS on/off per output.
//!
//! wlroots implements the whole wire protocol server-side and just signals us
//! with `set_mode` (which output, on/off) — we only apply it via
//! `oxide_output_set_powered` and, when turning an output back on, force a
//! full repaint the same way VT-resume already does (a disabled output's
//! scene doesn't repaint idle windows on its own).

use crate::ffi::*;
use crate::state::{Server, REPAINT_FRAMES};
use crate::wlr;
use std::os::raw::c_void;

pub(crate) unsafe fn setup(display: *mut wlr::wl_display, userdata: *mut c_void) {
    let manager = wlr::wlr_output_power_manager_v1_create(display);
    assert!(!manager.is_null(), "failed to create output-power manager");
    oxide_output_power_manager_add_set_mode(manager, handle_set_mode, userdata);
}

unsafe extern "C" fn handle_set_mode(userdata: *mut c_void, data: *mut c_void) {
    let server = &mut *(userdata as *mut Server);
    let output = oxide_output_power_set_mode_event_output(data);
    let on = oxide_output_power_set_mode_event_is_on(data);

    let Some(index) = server.outputs.iter().position(|o| o.wlr_output == output) else {
        eprintln!("0xin: output-power set_mode targeted an unknown output");
        return;
    };

    oxide_output_set_powered(output, on);
    if on {
        // Mirrors the VT-resume path (src/output.rs): a re-enabled output's
        // damage-tracked scene won't re-present idle windows on its own.
        server.outputs[index].repaint_frames = REPAINT_FRAMES;
        oxide_output_schedule_frame(output);
    }
    eprintln!(
        "0xin: output {} powered {}",
        index,
        if on { "on" } else { "off" }
    );
}
