//! Turning backend input events into compositor policy and seat events.
//!
//! The backends (winit, libinput) hand us device-level events; this module
//! decides what each one means — a keybinding, a pointer grab, a touch
//! gesture — and passes on whatever is left to the focused client through the
//! seat.

use smithay::backend::input::{
    AbsolutePositionEvent, Axis, AxisSource, ButtonState, Event, InputBackend, InputEvent,
    KeyState, KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent, PointerMotionEvent,
    TouchEvent, TouchSlot,
};
use smithay::desktop::{layer_map_for_output, WindowSurfaceType};
use smithay::input::keyboard::{FilterResult, ModifiersState};
use smithay::input::pointer::{AxisFrame, ButtonEvent, MotionEvent};
use smithay::input::touch::{DownEvent, MotionEvent as TouchMotionEvent, UpEvent};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Point, Rectangle, SERIAL_COUNTER};
use smithay::wayland::shell::wlr_layer::Layer;

use crate::config::{GestureTrigger, Action, MOD_ALT, MOD_CTRL, MOD_LOGO, MOD_SHIFT};
use crate::gestures::Outcome;
use crate::handlers::focus_clicked_window;
use crate::keybindings::{dispatch_action, handle_keybinding};
use crate::state::{GrabMode, Oxin};
use crate::tiling::{place, rect};
use crate::toplevel::{clamp_floating, set_solo};
use crate::window;

// Linux input-event button codes (input-event-codes.h).
const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;

/// Smallest size a resize drag can shrink a floating window to.
const MIN_FLOAT_SIZE: i32 = 50;

/// Smithay reports modifiers as flags; 0xin's config matches the WLR_MODIFIER_*
/// bit layout, so translate once here (see `config::MOD_*`).
pub fn modifier_bits(modifiers: &ModifiersState) -> u32 {
    let mut bits = 0;
    if modifiers.shift {
        bits |= MOD_SHIFT;
    }
    if modifiers.ctrl {
        bits |= MOD_CTRL;
    }
    if modifiers.alt {
        bits |= MOD_ALT;
    }
    if modifiers.logo {
        bits |= MOD_LOGO;
    }
    bits
}

pub fn process_input_event<B: InputBackend>(state: &mut Oxin, event: InputEvent<B>) {
    match event {
        InputEvent::Keyboard { event } => keyboard::<B>(state, event),
        InputEvent::PointerMotion { event } => {
            let delta = event.delta();
            let location = state.pointer_location + delta;
            pointer_motion(state, location, event.time_msec());
        }
        InputEvent::PointerMotionAbsolute { event } => {
            let Some(geometry) = state.outputs.first().map(|entry| entry.geometry) else {
                return;
            };
            let location = Point::from((
                geometry.loc.x as f64 + event.x_transformed(geometry.size.w),
                geometry.loc.y as f64 + event.y_transformed(geometry.size.h),
            ));
            pointer_motion(state, location, event.time_msec());
        }
        InputEvent::PointerButton { event } => {
            pointer_button(state, event.button_code(), event.state(), event.time_msec())
        }
        InputEvent::PointerAxis { event } => {
            let mut frame = AxisFrame::new(event.time_msec()).source(AxisSource::Wheel);
            for axis in [Axis::Horizontal, Axis::Vertical] {
                if let Some(value) = event.amount(axis) {
                    frame = frame.value(axis, value);
                }
                if let Some(discrete) = event.amount_v120(axis) {
                    frame = frame.v120(axis, discrete as i32);
                }
            }
            if let Some(pointer) = state.seat.get_pointer() {
                pointer.axis(state, frame);
                pointer.frame(state);
            }
        }
        InputEvent::TouchDown { event } => {
            let Some(location) = touch_location(state, &event) else {
                return;
            };
            let id = slot_id(event.slot());
            let time = event.time_msec();
            let output = output_rect_at(state, location);
            let under = surface_under(state, location).is_some();
            let outcomes = state.gestures.down(id, location, time, output, under);
            apply_outcomes(state, outcomes);
            arm_hold_timer(state, id);
        }
        InputEvent::TouchMotion { event } => {
            let Some(location) = touch_location(state, &event) else {
                return;
            };
            let id = slot_id(event.slot());
            let output = output_rect_at(state, location);
            let outcomes = state
                .gestures
                .motion(id, location, event.time_msec(), output);
            apply_outcomes(state, outcomes);
        }
        InputEvent::TouchUp { event } => {
            let id = slot_id(event.slot());
            let outcomes = state.gestures.up(id, event.time_msec());
            apply_outcomes(state, outcomes);
        }
        InputEvent::TouchCancel { event } => {
            let id = slot_id(event.slot());
            state.gestures.cancel(id);
            if let Some(touch) = state.seat.get_touch() {
                touch.cancel(state);
            }
        }
        InputEvent::TouchFrame { .. } => {
            if let Some(touch) = state.seat.get_touch() {
                touch.frame(state);
            }
        }
        _ => {}
    }
}

fn slot_id(slot: TouchSlot) -> i32 {
    i32::from(slot)
}

fn touch_location<B: InputBackend, E: AbsolutePositionEvent<B>>(
    state: &Oxin,
    event: &E,
) -> Option<Point<f64, Logical>> {
    let geometry = state.outputs.first().map(|entry| entry.geometry)?;
    Some(Point::from((
        geometry.loc.x as f64 + event.x_transformed(geometry.size.w),
        geometry.loc.y as f64 + event.y_transformed(geometry.size.h),
    )))
}

fn keyboard<B: InputBackend>(state: &mut Oxin, event: B::KeyboardKeyEvent) {
    let Some(keyboard) = state.seat.get_keyboard() else {
        return;
    };
    let serial = SERIAL_COUNTER.next_serial();
    let time = event.time_msec();
    let pressed = event.state() == KeyState::Pressed;
    keyboard.input::<(), _>(
        state,
        event.key_code(),
        event.state(),
        serial,
        time,
        |state, modifiers, handle| {
            let mods = modifier_bits(modifiers);
            // Match bindings on the layout level-0 (unshifted) keysym, so e.g.
            // Mod+Shift+1 reads as '1' (+Shift modifier), not the shifted '!'.
            let mut handled = false;
            for keysym in handle.raw_syms() {
                if handle_keybinding(state, keysym.raw(), mods, pressed) {
                    handled = true;
                }
            }
            if handled {
                FilterResult::Intercept(())
            } else {
                FilterResult::Forward
            }
        },
    );
}

fn pointer_motion(state: &mut Oxin, location: Point<f64, Logical>, time: u32) {
    state.pointer_location = clamp_to_outputs(state, location);
    let location = state.pointer_location;

    // An active Mod+drag owns the pointer: the grabbed window follows it and
    // no client sees enter/motion.
    if state.grab != GrabMode::None && !state.locked {
        drag_motion(state, location);
        return;
    }

    let under = surface_under(state, location);
    if let Some(pointer) = state.seat.get_pointer() {
        let serial = SERIAL_COUNTER.next_serial();
        pointer.motion(
            state,
            under,
            &MotionEvent {
                location,
                serial,
                time,
            },
        );
        pointer.frame(state);
    }
}

fn drag_motion(state: &mut Oxin, location: Point<f64, Logical>) {
    let Some(window) = state.grab_window.clone() else {
        return;
    };
    let delta = location - state.grab_cursor;
    let (dx, dy) = (delta.x as i32, delta.y as i32);
    let start = state.grab_rect;
    match state.grab {
        GrabMode::None => {}
        GrabMode::Move => {
            let (x, y) = clamp_floating(state, &window, start.loc.x + dx, start.loc.y + dy);
            place(state, &window, rect(x, y, start.size.w, start.size.h));
        }
        GrabMode::Resize => {
            // Bottom-right-corner semantics: position stays, size follows.
            let w = (start.size.w + dx).max(MIN_FLOAT_SIZE);
            let h = (start.size.h + dy).max(MIN_FLOAT_SIZE);
            place(state, &window, rect(start.loc.x, start.loc.y, w, h));
        }
    }
}

fn pointer_button(state: &mut Oxin, button: u32, button_state: ButtonState, time: u32) {
    let serial = SERIAL_COUNTER.next_serial();
    let pressed = button_state == ButtonState::Pressed;

    if !state.locked {
        if !pressed {
            // Any release ends an active grab — and is swallowed with it,
            // since the client never saw the press.
            if state.grab != GrabMode::None {
                state.grab = GrabMode::None;
                state.grab_window = None;
                return;
            }
        } else if start_grab(state, button) {
            return;
        }

        if pressed {
            if let Some((surface, _)) = surface_under(state, state.pointer_location) {
                focus_clicked_window(state, &surface);
            }
        }
    }

    if let Some(pointer) = state.seat.get_pointer() {
        pointer.button(
            state,
            &ButtonEvent {
                button,
                state: button_state,
                serial,
                time,
            },
        );
        pointer.frame(state);
    }
}

/// A press with the primary modifier held on a floating window starts a grab
/// (left button moves, right resizes).
fn start_grab(state: &mut Oxin, button: u32) -> bool {
    let modifiers = state
        .seat
        .get_keyboard()
        .map(|keyboard| modifier_bits(&keyboard.modifier_state()))
        .unwrap_or(0);
    if modifiers != state.config.modifier {
        return false;
    }
    let mode = match button {
        BTN_LEFT => GrabMode::Move,
        BTN_RIGHT => GrabMode::Resize,
        _ => return false,
    };
    let Some((surface, _)) = surface_under(state, state.pointer_location) else {
        return false;
    };
    let Some(window) = state.window_for_surface(&surface) else {
        return false;
    };
    if !window::is_floating(&window) || window::is_fullscreen(&window) {
        return false;
    }
    state.grab = mode;
    state.grab_rect = window::rect(&window);
    state.grab_window = Some(window);
    state.grab_cursor = state.pointer_location;
    true
}

/// Carry out what the gesture recognizer decided.
fn apply_outcomes(state: &mut Oxin, outcomes: Vec<Outcome>) {
    for outcome in outcomes {
        match outcome {
            Outcome::Trigger(trigger) => {
                if state.locked {
                    continue;
                }
                let action = state
                    .config
                    .gestures
                    .iter()
                    .find(|binding| binding.trigger == trigger)
                    .map(|binding| binding.action.clone());
                if let Some(action) = action {
                    dispatch_action(state, action);
                }
            }
            Outcome::DoubleTap(location) => double_tap(state, location),
            Outcome::Down { id, at, time } => {
                let Some((surface, surface_location)) = surface_under(state, at) else {
                    continue;
                };
                if let Some(touch) = state.seat.get_touch() {
                    let serial = SERIAL_COUNTER.next_serial();
                    touch.down(
                        state,
                        Some((surface, surface_location)),
                        &DownEvent {
                            slot: Some(id as u32).into(),
                            location: at,
                            serial,
                            time,
                        },
                    );
                }
            }
            Outcome::Motion { id, at, time } => {
                let under = surface_under(state, at);
                if let Some(touch) = state.seat.get_touch() {
                    touch.motion(
                        state,
                        under,
                        &TouchMotionEvent {
                            slot: Some(id as u32).into(),
                            location: at,
                            time,
                        },
                    );
                }
            }
            Outcome::Up { id, time } => {
                if let Some(touch) = state.seat.get_touch() {
                    let serial = SERIAL_COUNTER.next_serial();
                    touch.up(
                        state,
                        &UpEvent {
                            slot: Some(id as u32).into(),
                            serial,
                            time,
                        },
                    );
                }
            }
            Outcome::CancelClientTouch => {
                if let Some(touch) = state.seat.get_touch() {
                    touch.cancel(state);
                }
            }
        }
    }
}

/// A completed two-finger double tap on a window. `Action::ToggleSolo` is
/// resolved directly against the tapped window rather than through
/// `dispatch_action`'s usual active-workspace lookup: that lookup depends on
/// the pointer's last position, which touch never updates, so on a
/// multi-monitor setup it could target the wrong output's focused window — we
/// already have the exact tapped window in hand.
fn double_tap(state: &mut Oxin, location: Point<f64, Logical>) {
    if state.locked {
        return;
    }
    let Some((surface, _)) = surface_under(state, location) else {
        return;
    };
    let Some(window) = state.window_for_surface(&surface) else {
        return;
    };
    focus_clicked_window(state, &surface);

    let action = state
        .config
        .gestures
        .iter()
        .find(|binding| binding.trigger == GestureTrigger::DoubleTap)
        .map(|binding| binding.action.clone());
    match action {
        Some(Action::ToggleSolo) => {
            let workspace = state.workspace_of(&window);
            let want_on = workspace
                .map(|index| state.workspaces[index].solo.as_ref() != Some(&window))
                .unwrap_or(false);
            set_solo(state, &window, want_on);
        }
        Some(other) => dispatch_action(state, other),
        None => {}
    }
}

/// Hold a keyboard touch briefly before forwarding it, so a swipe-down-to-hide
/// can claim it first.
fn arm_hold_timer(state: &mut Oxin, id: i32) {
    use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
    use std::time::Duration;

    let timer = Timer::from_duration(Duration::from_millis(crate::gestures::KEYBOARD_HOLD_MS));
    let _ = state
        .loop_handle
        .insert_source(timer, move |_, _, state: &mut Oxin| {
            let outcomes = state.gestures.hold_timeout(id);
            apply_outcomes(state, outcomes);
            TimeoutAction::Drop
        });
}

/// The output containing a point, in global coordinates.
pub fn output_rect_at(state: &Oxin, location: Point<f64, Logical>) -> Option<Rectangle<i32, Logical>> {
    let point = location.to_i32_round();
    state
        .outputs
        .iter()
        .find(|entry| entry.geometry.contains(point))
        .map(|entry| entry.geometry)
}

fn clamp_to_outputs(state: &Oxin, location: Point<f64, Logical>) -> Point<f64, Logical> {
    if state.outputs.iter().any(|entry| {
        entry.geometry.contains(location.to_i32_round())
    }) {
        return location;
    }
    let Some(entry) = state.outputs.first() else {
        return location;
    };
    let geometry = entry.geometry;
    Point::from((
        location.x.clamp(
            geometry.loc.x as f64,
            (geometry.loc.x + geometry.size.w - 1) as f64,
        ),
        location.y.clamp(
            geometry.loc.y as f64,
            (geometry.loc.y + geometry.size.h - 1) as f64,
        ),
    ))
}

/// What is under a point, in the same z-order the renderer draws: the lock
/// surface when locked, then overlay/top layers, then windows, then the
/// bottom/background layers.
pub fn surface_under(
    state: &Oxin,
    location: Point<f64, Logical>,
) -> Option<(WlSurface, Point<f64, Logical>)> {
    let point = location.to_i32_round();
    let entry = state
        .outputs
        .iter()
        .find(|entry| entry.geometry.contains(point))?;
    let output_local = location - entry.geometry.loc.to_f64();

    if state.locked {
        return entry
            .lock_surface
            .as_ref()
            .map(|lock| (lock.wl_surface().clone(), output_local));
    }

    let map = layer_map_for_output(&entry.output);
    for layer in [Layer::Overlay, Layer::Top] {
        if let Some(surface) = layer_under(&map, layer, output_local) {
            return Some(surface);
        }
    }
    drop(map);

    // Windows, in the renderer's order: fullscreen, then floating, then tiled.
    let workspace = &state.workspaces[entry.workspace];
    let mut candidates: Vec<&smithay::desktop::Window> = Vec::new();
    candidates.extend(workspace.windows.iter().filter(|w| window::is_fullscreen(w)));
    candidates.extend(
        workspace
            .windows
            .iter()
            .filter(|w| window::is_floating(w) && !window::is_fullscreen(w)),
    );
    candidates.extend(workspace.windows.iter().filter(|w| window::is_tiled(w)));
    for window in candidates {
        let geometry = window::rect(window);
        if !geometry.contains(point) {
            continue;
        }
        let window_local = location - geometry.loc.to_f64();
        if let Some((surface, offset)) =
            window.surface_under(window_local, WindowSurfaceType::ALL)
        {
            return Some((surface, window_local - offset.to_f64() + offset.to_f64()));
        }
    }

    let map = layer_map_for_output(&entry.output);
    for layer in [Layer::Bottom, Layer::Background] {
        if let Some(surface) = layer_under(&map, layer, output_local) {
            return Some(surface);
        }
    }
    None
}

fn layer_under(
    map: &smithay::desktop::LayerMap,
    layer: Layer,
    output_local: Point<f64, Logical>,
) -> Option<(WlSurface, Point<f64, Logical>)> {
    for surface in map.layers_on(layer) {
        let geometry = map.layer_geometry(surface)?;
        if !geometry.contains(output_local.to_i32_round()) {
            continue;
        }
        let local = output_local - geometry.loc.to_f64();
        if let Some((wl_surface, offset)) = surface.surface_under(local, WindowSurfaceType::ALL) {
            return Some((wl_surface, local - offset.to_f64()));
        }
    }
    None
}
