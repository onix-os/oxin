//! Output (monitor) lifecycle: adding, resizing and removing outputs, and the
//! workspace each one starts on.

use smithay::output::{Output, Scale};
use smithay::utils::{Physical, Point, Rectangle, Size, Transform};

use crate::state::{OutputEntry, Oxin, WORKSPACE_COUNT};
use crate::tiling::{arrange_layers, refresh};
use crate::wallpaper;

/// Register a new output at `position` in the global layout.
///
/// A `monitor = NAME, XxY[, SCALE]` config entry for this connector name (e.g.
/// "HDMI-A-1") overrides both; otherwise the caller's position stands and the
/// scale stays 1.
pub fn add_output(state: &mut Oxin, output: Output, position: (i32, i32)) {
    let name = output.name();
    let monitor = state
        .config
        .monitors
        .iter()
        .find(|monitor| monitor.name == name)
        .cloned();

    let location = match &monitor {
        Some(monitor) => Point::from((monitor.x, monitor.y)),
        None => Point::from(position),
    };
    if let Some(monitor) = &monitor {
        output.change_current_state(None, None, Some(Scale::Fractional(monitor.scale as f64)), None);
    }
    output.change_current_state(None, None, None, Some(location));

    let size = logical_size(&output);
    let geometry = Rectangle::new(location, size);

    state.space.map_output(&output, location);

    // Give the output the lowest-numbered workspace not already on a monitor.
    let mut workspace = 0;
    for candidate in 0..WORKSPACE_COUNT {
        if !state.outputs.iter().any(|entry| entry.workspace == candidate) {
            workspace = candidate;
            break;
        }
    }

    state.outputs.push(OutputEntry {
        output: output.clone(),
        geometry,
        // No layer surfaces yet; usable area starts as the full box.
        usable: geometry,
        workspace,
        wallpaper: None,
        lock_surface: None,
    });
    let index = state.outputs.len() - 1;
    wallpaper::create_for_output(state, index);

    arrange_layers(state, &output);
    refresh(state); // tile any windows already belonging to this workspace

    eprintln!(
        "0xin: output {name} online @ {},{} {}x{} — workspace {}",
        geometry.loc.x,
        geometry.loc.y,
        geometry.size.w,
        geometry.size.h,
        workspace + 1
    );
}

/// The output's mode changed (the nested window was resized, or a monitor
/// re-modeset): recompute its box and everything that depends on it.
pub fn resize_output(state: &mut Oxin, output: &Output, _size: Size<i32, Physical>) {
    let size = logical_size(output);
    let Some(entry) = state
        .outputs
        .iter_mut()
        .find(|entry| &entry.output == output)
    else {
        return;
    };
    entry.geometry = Rectangle::new(entry.geometry.loc, size);
    entry.usable = entry.geometry;
    let index = state
        .outputs
        .iter()
        .position(|entry| &entry.output == output)
        .unwrap();
    // The wallpaper is decoded for an exact output size, so it has to be
    // rebuilt when that size changes.
    state.outputs[index].wallpaper = None;
    wallpaper::create_for_output(state, index);
    arrange_layers(state, output);
    refresh(state);
}

/// A monitor was unplugged (or logind took the seat away on a VT switch).
pub fn remove_output(state: &mut Oxin, output: &Output) {
    state.space.unmap_output(output);
    state.outputs.retain(|entry| &entry.output != output);
    refresh(state);
    eprintln!("0xin: output removed — {} left", state.outputs.len());
}

/// An output's size in logical coordinates: its mode divided by its scale.
fn logical_size(output: &Output) -> Size<i32, smithay::utils::Logical> {
    let scale = output.current_scale().fractional_scale();
    let transform = output.current_transform();
    let mode = output
        .current_mode()
        .map(|mode| mode.size)
        .unwrap_or_else(|| (0, 0).into());
    let mode = match transform {
        Transform::_90 | Transform::_270 | Transform::Flipped90 | Transform::Flipped270 => {
            Size::from((mode.h, mode.w))
        }
        _ => mode,
    };
    Size::from((
        (mode.w as f64 / scale).round() as i32,
        (mode.h as f64 / scale).round() as i32,
    ))
}
