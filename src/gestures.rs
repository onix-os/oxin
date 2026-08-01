//! Touch gesture recognition.
//!
//! This is the phone profile's input policy, ported from the C shim: which
//! touches belong to the compositor (edge swipes, the keyboard handle,
//! multi-finger gestures) and which belong to the client under the finger.
//! It is deliberately pure — every event returns a list of [`Outcome`]s for
//! `input.rs` to carry out — so the thresholds can be tested without a seat,
//! a client, or a GPU.

use smithay::utils::{Logical, Point, Rectangle};

use crate::config::GestureTrigger;
use crate::state::OutputEntry;

/// Max travel for a touch to still count as a tap, and the max time/distance
/// gap between two taps for them to count as a pair.
const TAP_DRAG_PX: f64 = 24.0;
const DOUBLE_TAP_MS: u32 = 400;
const DOUBLE_TAP_PX: f64 = 100.0;
/// How long a touch landing on the visible keyboard is held before being
/// forwarded as a real keypress, giving a swipe-down-to-hide gesture a chance
/// to claim it first.
pub const KEYBOARD_HOLD_MS: u64 = 120;

/// Distance a single-finger edge swipe must travel to fire.
const SWIPE_PX: f64 = 70.0;
/// Distance the keyboard-show swipe (upwards, so negative dy) must travel.
const KEYBOARD_SHOW_PX: f64 = 60.0;
/// Width of the left/right workspace-switch strips and the top-edge strip.
const EDGE_PX: f64 = 28.0;
/// Centroid travel a multi-finger swipe needs, and the minimum each single
/// finger must contribute in that direction.
const MULTI_SWIPE_PX: f64 = 70.0;
const MULTI_FINGER_PX: f64 = 35.0;
/// A locked directional swipe emits one step per 5% of the output crossed.
const STEPS_PER_SWIPE: f64 = 20.0;
/// Movement needed before a top-edge (or left-edge vertical) swipe locks its
/// direction.
const LOCK_PX: f64 = 30.0;

/// What the recognizer decided an event means.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// Run the action bound to this gesture.
    Trigger(GestureTrigger),
    /// A two-finger double tap completed at this point; the caller hit-tests
    /// it to find the window and applies the `double-tap` action to it.
    DoubleTap(Point<f64, Logical>),
    /// Deliver this touch to the client under it.
    Down {
        id: i32,
        at: Point<f64, Logical>,
        time: u32,
    },
    Motion {
        id: i32,
        at: Point<f64, Logical>,
        time: u32,
    },
    Up { id: i32, time: u32 },
    /// A compositor gesture took over a sequence the client had already seen.
    CancelClientTouch,
}

/// How a live touch point is being interpreted.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Kind {
    /// An ordinary touch, forwarded to the client.
    Client,
    /// The (hidden) keyboard handle: swipe up to show the keyboard.
    KeyboardHandle,
    /// A left/right screen-edge strip: workspace switches, and (left only)
    /// vertical volume steps.
    Edge,
    /// The top strip: brightness steps sideways, or a downward swipe.
    Top,
    /// Landed on bare background with multi-finger gestures configured — kept
    /// so a second finger can promote it, but never forwarded.
    Background,
    /// Landed on the visible keyboard: held back briefly in case this becomes
    /// a swipe-down-to-hide instead of a keypress.
    KeyboardHold,
}

struct TouchPoint {
    id: i32,
    kind: Kind,
    fired: bool,
    to_top_candidate: bool,
    /// Steps already emitted by a locked directional swipe.
    steps: i32,
    /// -1 for the left edge and +1 for the right edge.
    edge: i32,
    /// Locked axis for a directional swipe: left-edge vertical volume (Edge)
    /// or top-edge horizontal brightness (Top). 0 until locked.
    lock: i32,
    start: Point<f64, Logical>,
    last: Point<f64, Logical>,
    last_time: u32,
    down_time: u32,
    /// True once the client has seen this sequence (so a promotion has to
    /// cancel it).
    forwarded: bool,
}

#[derive(Default)]
struct Multi {
    active: bool,
    fired: bool,
    ids: Vec<i32>,
    down: Vec<bool>,
    start: Vec<Point<f64, Logical>>,
    current: Vec<Point<f64, Logical>>,
    active_count: usize,
}

impl Multi {
    fn reset(&mut self) {
        self.active = false;
        self.fired = false;
        self.ids.clear();
        self.down.clear();
        self.start.clear();
        self.current.clear();
        self.active_count = 0;
    }

    fn index(&self, id: i32) -> Option<usize> {
        self.ids
            .iter()
            .zip(&self.down)
            .position(|(other, down)| *down && *other == id)
    }

    fn add(&mut self, id: i32, at: Point<f64, Logical>) {
        if self.ids.len() >= 3 {
            return;
        }
        self.ids.push(id);
        self.down.push(true);
        self.start.push(at);
        self.current.push(at);
        self.active_count += 1;
    }
}

pub struct Recognizer {
    /// Bit per `GestureTrigger` — which gestures the config actually binds.
    mask: u32,
    keyboard_visible: bool,
    keyboard_height: i32,
    handle_visible: bool,
    points: Vec<TouchPoint>,
    multi: Multi,
    last_tap: Option<(Point<f64, Logical>, u32)>,
}

impl Recognizer {
    pub fn new(mask: u32, keyboard_height: i32, handle_visible: bool) -> Self {
        Recognizer {
            mask,
            keyboard_visible: false,
            keyboard_height,
            handle_visible,
            points: Vec::new(),
            multi: Multi::default(),
            last_tap: None,
        }
    }

    pub fn set_keyboard_visible(&mut self, visible: bool) {
        self.keyboard_visible = visible;
    }

    /// The visible keyboard's real height, once its layer surface has mapped
    /// and reserved an exclusive zone — better than the config's estimate.
    pub fn set_keyboard_height(&mut self, height: i32) {
        if height > 0 {
            self.keyboard_height = height;
        }
    }

    fn enabled(&self, trigger: GestureTrigger) -> bool {
        self.mask & (1 << trigger as u32) != 0
    }

    fn any(&self, triggers: &[GestureTrigger]) -> bool {
        triggers.iter().any(|trigger| self.enabled(*trigger))
    }

    /// Bits 8-16: two/three-finger swipes plus the two-finger double tap.
    fn multi_enabled(&self) -> bool {
        self.mask & 0x1_ff00 != 0
    }

    /// The compositor-owned pill hinting where the keyboard handle is. Follows
    /// the keyboard when it is up, so it stays reachable above it.
    pub fn handle_rect(&self, entry: &OutputEntry) -> Option<Rectangle<i32, Logical>> {
        if !self.handle_visible
            || !self.any(&[GestureTrigger::BottomUp, GestureTrigger::BottomDown])
        {
            return None;
        }
        let geometry = entry.geometry;
        let y = geometry.loc.y + geometry.size.h
            - if self.keyboard_visible {
                self.keyboard_height + 8
            } else {
                10
            };
        Some(Rectangle::new(
            Point::from((geometry.loc.x + (geometry.size.w - 120) / 2, y)),
            (120, 5).into(),
        ))
    }

    // --- zone predicates ---------------------------------------------------

    /// The hidden handle sits at the physical bottom edge and needs a larger
    /// upward-only acquisition target than the pill it draws.
    fn keyboard_handle_hit(&self, at: Point<f64, Logical>, output: Rectangle<i32, Logical>) -> bool {
        if self.keyboard_visible || !self.enabled(GestureTrigger::BottomUp) {
            return false;
        }
        let center_x = output.loc.x as f64 + output.size.w as f64 / 2.0;
        let handle_y = (output.loc.y + output.size.h - 10) as f64;
        at.x >= center_x - 100.0
            && at.x <= center_x + 100.0
            && at.y >= handle_y - 45.0
            && at.y <= handle_y + 45.0
    }

    /// -1 for the left strip, +1 for the right strip, 0 for neither.
    fn edge_zone(&self, at: Point<f64, Logical>, output: Rectangle<i32, Logical>) -> i32 {
        let left_triggers = [
            GestureTrigger::EdgeLeftIn,
            GestureTrigger::EdgeLeftUp,
            GestureTrigger::EdgeLeftDown,
        ];
        if !self.any(&left_triggers) && !self.enabled(GestureTrigger::EdgeRightIn) {
            return 0;
        }
        // The virtual keyboard owns its full surface, including edge-column
        // keys such as Tab, Backspace, P and Return. Keep workspace-edge
        // policy above it while visible instead of stealing those touches.
        if self.keyboard_visible
            && at.y >= (output.loc.y + output.size.h - self.keyboard_height) as f64
        {
            return 0;
        }
        if at.x <= (output.loc.x as f64) + EDGE_PX && self.any(&left_triggers) {
            return -1;
        }
        if at.x >= (output.loc.x + output.size.w) as f64 - EDGE_PX
            && self.enabled(GestureTrigger::EdgeRightIn)
        {
            return 1;
        }
        0
    }

    fn top_hit(&self, at: Point<f64, Logical>, output: Rectangle<i32, Logical>) -> bool {
        self.any(&[
            GestureTrigger::TopRight,
            GestureTrigger::TopLeft,
            GestureTrigger::TopDown,
        ]) && at.y <= output.loc.y as f64 + EDGE_PX
    }

    fn keyboard_hold_candidate(
        &self,
        at: Point<f64, Logical>,
        output: Rectangle<i32, Logical>,
    ) -> bool {
        self.keyboard_visible
            && self.enabled(GestureTrigger::BottomDown)
            && at.y >= (output.loc.y + output.size.h - self.keyboard_height) as f64
    }

    // --- events ------------------------------------------------------------

    /// A finger went down. `surface_under` says whether a client surface is
    /// there at all (bare background is a multi-finger candidate instead).
    pub fn down(
        &mut self,
        id: i32,
        at: Point<f64, Logical>,
        time: u32,
        output: Option<Rectangle<i32, Logical>>,
        surface_under: bool,
    ) -> Vec<Outcome> {
        let mut outcomes = Vec::new();
        if self.points.iter().any(|point| point.id == id) {
            eprintln!("0xin: duplicate touch ID {id} ignored");
            return outcomes;
        }
        if self.multi.active {
            self.multi.add(id, at);
            return outcomes;
        }
        // A second finger on an ordinary (or background) touch promotes the
        // whole sequence to a compositor gesture.
        if self.multi_enabled() {
            let candidates: Vec<usize> = self
                .points
                .iter()
                .enumerate()
                .filter(|(_, point)| matches!(point.kind, Kind::Client | Kind::Background))
                .map(|(index, _)| index)
                .collect();
            if candidates.len() == 1 {
                let first = self.points.remove(candidates[0]);
                if first.forwarded {
                    outcomes.push(Outcome::CancelClientTouch);
                }
                self.multi.reset();
                self.multi.active = true;
                self.multi.add(first.id, first.start);
                self.multi.add(id, at);
                return outcomes;
            }
        }

        let Some(output) = output else {
            return outcomes;
        };

        let mut point = TouchPoint {
            id,
            kind: Kind::Client,
            fired: false,
            to_top_candidate: false,
            steps: 0,
            edge: 0,
            lock: 0,
            start: at,
            last: at,
            last_time: time,
            down_time: time,
            forwarded: false,
        };

        if self.keyboard_handle_hit(at, output) {
            point.kind = Kind::KeyboardHandle;
            self.points.push(point);
            return outcomes;
        }
        if self.top_hit(at, output) {
            point.kind = Kind::Top;
            self.points.push(point);
            return outcomes;
        }
        let edge = self.edge_zone(at, output);
        if edge != 0 {
            point.kind = Kind::Edge;
            point.edge = edge;
            self.points.push(point);
            return outcomes;
        }
        if !surface_under {
            if self.multi_enabled() {
                point.kind = Kind::Background;
                self.points.push(point);
            }
            return outcomes;
        }
        // Touches landing on the visible keyboard are ambiguous: a normal
        // keypress, or the start of a swipe-down-to-hide over the same
        // surface. Forwarding immediately would let the keyboard register a
        // keypress before we know which — a cancel arriving later can't
        // un-type a character the client already committed. Hold these
        // briefly instead; the caller arms a KEYBOARD_HOLD_MS timer.
        if self.keyboard_hold_candidate(at, output) {
            point.kind = Kind::KeyboardHold;
            self.points.push(point);
            return outcomes;
        }

        point.to_top_candidate = self.enabled(GestureTrigger::ToTop)
            && at.y >= output.loc.y as f64 + SWIPE_PX;
        point.forwarded = true;
        self.points.push(point);
        outcomes.push(Outcome::Down { id, at, time });
        outcomes
    }

    pub fn motion(
        &mut self,
        id: i32,
        at: Point<f64, Logical>,
        time: u32,
        output: Option<Rectangle<i32, Logical>>,
    ) -> Vec<Outcome> {
        if self.multi.active {
            if let Some(index) = self.multi.index(id) {
                return self.multi_motion(index, at);
            }
        }
        let Some(position) = self.points.iter().position(|point| point.id == id) else {
            return Vec::new();
        };
        let Some(output) = output else {
            return Vec::new();
        };
        let mut outcomes = Vec::new();
        let kind = self.points[position].kind;
        let start = self.points[position].start;
        let (dx, dy) = (at.x - start.x, at.y - start.y);
        self.points[position].last = at;
        self.points[position].last_time = time;

        match kind {
            Kind::KeyboardHold => {
                // Confirmed swipe-down. The touch was never forwarded (that's
                // the whole point of holding it), so there is nothing to
                // cancel — just fire.
                if dy >= SWIPE_PX
                    && at.y >= (output.loc.y + output.size.h) as f64 - EDGE_PX
                {
                    self.points.remove(position);
                    outcomes.push(Outcome::Trigger(GestureTrigger::BottomDown));
                }
            }
            Kind::KeyboardHandle => {
                if !self.points[position].fired && dy <= -KEYBOARD_SHOW_PX {
                    self.points[position].fired = true;
                    outcomes.push(Outcome::Trigger(GestureTrigger::BottomUp));
                }
            }
            Kind::Edge => {
                let edge = self.points[position].edge;
                // Left-edge only: lock the vertical direction after a small
                // deliberate movement, then emit one volume step per 5% of
                // output height crossed, so a full-height swipe spans 0-100%.
                if edge == -1
                    && self.points[position].lock == 0
                    && self.any(&[GestureTrigger::EdgeLeftUp, GestureTrigger::EdgeLeftDown])
                    && dy.abs() >= LOCK_PX
                    && dy.abs() > dx.abs()
                {
                    self.points[position].lock = if dy > 0.0 { 1 } else { -1 };
                }
                let lock = self.points[position].lock;
                if lock != 0 {
                    let trigger = if lock > 0 {
                        GestureTrigger::EdgeLeftDown
                    } else {
                        GestureTrigger::EdgeLeftUp
                    };
                    let travel = lock as f64 * dy;
                    let steps = steps_for(travel, output.size.h);
                    while steps > self.points[position].steps && self.enabled(trigger) {
                        self.points[position].steps += 1;
                        outcomes.push(Outcome::Trigger(trigger));
                    }
                    return outcomes;
                }
                let previous = edge == -1 && dx >= SWIPE_PX;
                let next = edge == 1 && dx <= -SWIPE_PX;
                if !self.points[position].fired && (previous || next) {
                    self.points[position].fired = true;
                    outcomes.push(Outcome::Trigger(if previous {
                        GestureTrigger::EdgeLeftIn
                    } else {
                        GestureTrigger::EdgeRightIn
                    }));
                }
            }
            Kind::Top => {
                // Lock the horizontal direction after a small deliberate
                // movement; each 5% of output width crossed emits one
                // brightness step, so an edge-to-edge swipe spans 100%.
                if self.points[position].lock == 0 && dx.abs() >= LOCK_PX && dx.abs() > dy.abs() {
                    self.points[position].lock = if dx > 0.0 { 1 } else { -1 };
                }
                let lock = self.points[position].lock;
                if lock != 0 {
                    let trigger = if lock > 0 {
                        GestureTrigger::TopRight
                    } else {
                        GestureTrigger::TopLeft
                    };
                    let travel = lock as f64 * dx;
                    let steps = steps_for(travel, output.size.w);
                    while steps > self.points[position].steps && self.enabled(trigger) {
                        self.points[position].steps += 1;
                        outcomes.push(Outcome::Trigger(trigger));
                    }
                    return outcomes;
                }
                if !self.points[position].fired
                    && dy >= SWIPE_PX
                    && self.enabled(GestureTrigger::TopDown)
                {
                    self.points[position].fired = true;
                    outcomes.push(Outcome::Trigger(GestureTrigger::TopDown));
                }
            }
            Kind::Background => {}
            Kind::Client => {
                // An upward flick from anywhere below the top strip, ending in
                // it, is the "back to top" gesture; it steals the sequence.
                if self.points[position].to_top_candidate
                    && -dy >= SWIPE_PX
                    && at.y <= output.loc.y as f64 + EDGE_PX
                {
                    self.points.remove(position);
                    outcomes.push(Outcome::CancelClientTouch);
                    outcomes.push(Outcome::Trigger(GestureTrigger::ToTop));
                    return outcomes;
                }
                outcomes.push(Outcome::Motion { id, at, time });
            }
        }
        outcomes
    }

    pub fn up(&mut self, id: i32, time: u32) -> Vec<Outcome> {
        let mut outcomes = Vec::new();
        if self.multi.active {
            if let Some(index) = self.multi.index(id) {
                self.multi.down[index] = false;
                self.multi.active_count -= 1;
                if self.multi.active_count == 0 {
                    if let Some(point) = self.two_finger_tap(time) {
                        outcomes.push(Outcome::DoubleTap(point));
                    }
                    self.multi.reset();
                }
                return outcomes;
            }
        }
        let Some(position) = self.points.iter().position(|point| point.id == id) else {
            return outcomes;
        };
        let point = self.points.remove(position);
        match point.kind {
            // Lifted before the hold timer fired or the swipe committed — a
            // quick tap. Deliver the held-back down (with its real down time)
            // and immediately complete it with the up.
            Kind::KeyboardHold => {
                outcomes.push(Outcome::Down {
                    id,
                    at: point.start,
                    time: point.down_time,
                });
                outcomes.push(Outcome::Up { id, time });
            }
            Kind::Client => outcomes.push(Outcome::Up { id, time }),
            _ => {}
        }
        outcomes
    }

    pub fn cancel(&mut self, id: i32) {
        if self.multi.active && self.multi.index(id).is_some() {
            self.multi.reset();
            return;
        }
        self.points.retain(|point| point.id != id);
    }

    /// The keyboard-hold timer expired: this touch really was a keypress, so
    /// forward the down we held back and catch the client up to where the
    /// finger is now.
    pub fn hold_timeout(&mut self, id: i32) -> Vec<Outcome> {
        let Some(point) = self
            .points
            .iter_mut()
            .find(|point| point.id == id && point.kind == Kind::KeyboardHold)
        else {
            return Vec::new();
        };
        point.kind = Kind::Client;
        point.forwarded = true;
        let mut outcomes = vec![Outcome::Down {
            id,
            at: point.start,
            time: point.down_time,
        }];
        if point.last != point.start {
            outcomes.push(Outcome::Motion {
                id,
                at: point.last,
                time: point.last_time,
            });
        }
        outcomes
    }

    fn multi_motion(&mut self, index: usize, at: Point<f64, Logical>) -> Vec<Outcome> {
        self.multi.current[index] = at;
        let count = self.multi.ids.len();
        if self.multi.fired || self.multi.active_count != count || !(2..=3).contains(&count) {
            return Vec::new();
        }

        let (mut dx, mut dy) = (0.0, 0.0);
        for (current, start) in self.multi.current.iter().zip(&self.multi.start) {
            dx += current.x - start.x;
            dy += current.y - start.y;
        }
        dx /= count as f64;
        dy /= count as f64;

        let (direction, distance) = if dx.abs() > dy.abs() {
            (if dx < 0.0 { 2 } else { 3 }, dx.abs())
        } else {
            (if dy < 0.0 { 0 } else { 1 }, dy.abs())
        };
        if distance < MULTI_SWIPE_PX {
            return Vec::new();
        }

        // Require every finger to participate in the centroid direction. This
        // avoids treating one moving finger plus one stationary tap as a swipe.
        for (current, start) in self.multi.current.iter().zip(&self.multi.start) {
            let (finger_dx, finger_dy) = (current.x - start.x, current.y - start.y);
            let projected = match direction {
                0 => -finger_dy,
                1 => finger_dy,
                2 => -finger_dx,
                _ => finger_dx,
            };
            if projected < MULTI_FINGER_PX {
                return Vec::new();
            }
        }

        let base = if count == 2 { 8 } else { 12 };
        let Some(trigger) = trigger_from_raw(base + direction as u32) else {
            return Vec::new();
        };
        if !self.enabled(trigger) {
            return Vec::new();
        }
        self.multi.fired = true;
        vec![Outcome::Trigger(trigger)]
    }

    /// A tap gives no "in progress" signal to promote on (unlike the
    /// motion-based gestures), so this checks retroactively once both fingers
    /// are up: no swipe fired, and neither finger travelled past the tap
    /// threshold. Matches against the previous such tap for double-tap
    /// timing/position.
    fn two_finger_tap(&mut self, time: u32) -> Option<Point<f64, Logical>> {
        if self.multi.fired
            || self.multi.ids.len() != 2
            || !self.enabled(GestureTrigger::DoubleTap)
        {
            return None;
        }
        for (current, start) in self.multi.current.iter().zip(&self.multi.start) {
            let travel = ((current.x - start.x).powi(2) + (current.y - start.y).powi(2)).sqrt();
            if travel > TAP_DRAG_PX {
                return None;
            }
        }
        let center = Point::from((
            (self.multi.current[0].x + self.multi.current[1].x) / 2.0,
            (self.multi.current[0].y + self.multi.current[1].y) / 2.0,
        ));
        let matched = match self.last_tap {
            Some((previous, previous_time)) => {
                time.saturating_sub(previous_time) <= DOUBLE_TAP_MS
                    && ((center.x - previous.x).powi(2) + (center.y - previous.y).powi(2)).sqrt()
                        <= DOUBLE_TAP_PX
            }
            None => false,
        };
        if matched {
            // Consumed — a third tap starts a fresh pair rather than matching
            // again against this same recorded tap.
            self.last_tap = None;
            Some(center)
        } else {
            self.last_tap = Some((center, time));
            None
        }
    }
}

fn steps_for(travel: f64, extent: i32) -> i32 {
    if extent <= 0 {
        return 0;
    }
    ((travel * STEPS_PER_SWIPE / extent as f64) as i32).min(STEPS_PER_SWIPE as i32)
}

fn trigger_from_raw(raw: u32) -> Option<GestureTrigger> {
    Some(match raw {
        0 => GestureTrigger::BottomUp,
        1 => GestureTrigger::BottomDown,
        2 => GestureTrigger::EdgeLeftIn,
        3 => GestureTrigger::EdgeRightIn,
        4 => GestureTrigger::TopRight,
        5 => GestureTrigger::TopLeft,
        6 => GestureTrigger::TopDown,
        7 => GestureTrigger::ToTop,
        8 => GestureTrigger::TwoUp,
        9 => GestureTrigger::TwoDown,
        10 => GestureTrigger::TwoLeft,
        11 => GestureTrigger::TwoRight,
        12 => GestureTrigger::ThreeUp,
        13 => GestureTrigger::ThreeDown,
        14 => GestureTrigger::ThreeLeft,
        15 => GestureTrigger::ThreeRight,
        16 => GestureTrigger::DoubleTap,
        17 => GestureTrigger::EdgeLeftUp,
        18 => GestureTrigger::EdgeLeftDown,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn output() -> Rectangle<i32, Logical> {
        Rectangle::new(Point::from((0, 0)), (1080, 2400).into())
    }

    fn mask(triggers: &[GestureTrigger]) -> u32 {
        triggers
            .iter()
            .fold(0, |mask, trigger| mask | 1 << *trigger as u32)
    }

    fn at(x: f64, y: f64) -> Point<f64, Logical> {
        Point::from((x, y))
    }

    #[test]
    fn bottom_swipe_up_shows_the_keyboard() {
        let mut recognizer = Recognizer::new(mask(&[GestureTrigger::BottomUp]), 300, true);
        assert!(recognizer
            .down(1, at(540.0, 2395.0), 0, Some(output()), true)
            .is_empty());
        let outcomes = recognizer.motion(1, at(540.0, 2320.0), 10, Some(output()));
        assert_eq!(outcomes, vec![Outcome::Trigger(GestureTrigger::BottomUp)]);
    }

    // The left strip claims touches for the volume swipe even when the
    // horizontal workspace gesture is not configured, so both can share it.
    #[test]
    fn left_edge_vertical_swipe_emits_one_step_per_5_percent() {
        let mut recognizer = Recognizer::new(mask(&[GestureTrigger::EdgeLeftDown]), 300, false);
        recognizer.down(1, at(10.0, 100.0), 0, Some(output()), true);
        // 30px down locks the axis but is under one step (5% of 2400 = 120px).
        assert!(recognizer
            .motion(1, at(10.0, 130.0), 10, Some(output()))
            .is_empty());
        let outcomes = recognizer.motion(1, at(10.0, 340.0), 20, Some(output()));
        assert_eq!(
            outcomes,
            vec![Outcome::Trigger(GestureTrigger::EdgeLeftDown); 2]
        );
    }

    #[test]
    fn ordinary_touch_is_forwarded_to_the_client() {
        let mut recognizer = Recognizer::new(0, 300, false);
        let outcomes = recognizer.down(1, at(500.0, 1000.0), 5, Some(output()), true);
        assert_eq!(
            outcomes,
            vec![Outcome::Down {
                id: 1,
                at: at(500.0, 1000.0),
                time: 5
            }]
        );
        assert_eq!(recognizer.up(1, 9), vec![Outcome::Up { id: 1, time: 9 }]);
    }

    // A second finger promotes the sequence to a compositor gesture, which
    // means the client's in-flight touch has to be cancelled first.
    #[test]
    fn second_finger_promotes_and_cancels_the_client_sequence() {
        let mut recognizer = Recognizer::new(mask(&[GestureTrigger::TwoUp]), 300, false);
        recognizer.down(1, at(400.0, 1200.0), 0, Some(output()), true);
        let outcomes = recognizer.down(2, at(600.0, 1200.0), 5, Some(output()), true);
        assert_eq!(outcomes, vec![Outcome::CancelClientTouch]);

        recognizer.motion(1, at(400.0, 1100.0), 10, Some(output()));
        let outcomes = recognizer.motion(2, at(600.0, 1100.0), 10, Some(output()));
        assert_eq!(outcomes, vec![Outcome::Trigger(GestureTrigger::TwoUp)]);
    }

    // One finger moving while the other stays put is not a swipe.
    #[test]
    fn multi_swipe_requires_every_finger_to_move() {
        let mut recognizer = Recognizer::new(mask(&[GestureTrigger::TwoUp]), 300, false);
        recognizer.down(1, at(400.0, 1200.0), 0, Some(output()), true);
        recognizer.down(2, at(600.0, 1200.0), 5, Some(output()), true);
        let outcomes = recognizer.motion(1, at(400.0, 900.0), 10, Some(output()));
        assert!(outcomes.is_empty());
    }

    #[test]
    fn two_finger_double_tap_fires_on_the_second_pair() {
        let mut recognizer = Recognizer::new(mask(&[GestureTrigger::DoubleTap]), 300, false);
        for (start, time) in [(0, 0u32), (1, 100)] {
            let _ = start;
            recognizer.down(1, at(500.0, 1000.0), time, Some(output()), true);
            recognizer.down(2, at(560.0, 1000.0), time, Some(output()), true);
            let first = recognizer.up(1, time + 20);
            let second = recognizer.up(2, time + 20);
            if time == 100 {
                assert!(first.is_empty());
                assert_eq!(second, vec![Outcome::DoubleTap(at(530.0, 1000.0))]);
            }
        }
    }

    // Touches on the visible keyboard are held back, so a swipe-down can claim
    // them instead of the client committing a keypress.
    #[test]
    fn keyboard_touch_is_held_then_released_as_a_tap() {
        let mut recognizer = Recognizer::new(mask(&[GestureTrigger::BottomDown]), 300, false);
        recognizer.set_keyboard_visible(true);
        assert!(recognizer
            .down(1, at(500.0, 2200.0), 0, Some(output()), true)
            .is_empty());
        assert_eq!(
            recognizer.up(1, 40),
            vec![
                Outcome::Down {
                    id: 1,
                    at: at(500.0, 2200.0),
                    time: 0
                },
                Outcome::Up { id: 1, time: 40 }
            ]
        );
    }

    #[test]
    fn keyboard_swipe_down_hides_without_reaching_the_client() {
        let mut recognizer = Recognizer::new(mask(&[GestureTrigger::BottomDown]), 300, false);
        recognizer.set_keyboard_visible(true);
        recognizer.down(1, at(500.0, 2200.0), 0, Some(output()), true);
        let outcomes = recognizer.motion(1, at(500.0, 2390.0), 30, Some(output()));
        assert_eq!(outcomes, vec![Outcome::Trigger(GestureTrigger::BottomDown)]);
    }
}
