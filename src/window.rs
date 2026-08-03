//! Per-window policy state.
//!
//! Smithay's `Window` is a handle to a mapped toplevel; the tiling policy needs
//! a little state of its own alongside it (does this window float, is it
//! fullscreen, what rect did the last tiling pass give it). Smithay hands every
//! element a `UserDataMap` for exactly this, so the state travels with the
//! window handle instead of living in a parallel map we would have to keep in
//! sync with map/unmap.

use std::cell::{Ref, RefCell, RefMut};

use smithay::desktop::Window;
use smithay::utils::{Logical, Rectangle};

#[derive(Default)]
pub struct WindowData {
    /// Whether this window covers its output's full box, painted above bars.
    pub fullscreen: bool,
    /// Whether this window floats: keeps its own size, centred on map, painted
    /// above tiled windows, and holds no leaf in the split tree. Fullscreen
    /// wins while both are set.
    pub floating: bool,
    /// This window's rect as of the last `tiling::refresh()` pass. Not
    /// authoritative (the space position and the toplevel's configured size
    /// are) — just a cache for directional focus/move to compare windows
    /// against each other, and for restoring a floating window's geometry.
    pub rect: Rectangle<i32, Logical>,
}

fn cell(window: &Window) -> &RefCell<WindowData> {
    window
        .user_data()
        .get_or_insert(|| RefCell::new(WindowData::default()))
}

pub fn data(window: &Window) -> Ref<'_, WindowData> {
    cell(window).borrow()
}

pub fn data_mut(window: &Window) -> RefMut<'_, WindowData> {
    cell(window).borrow_mut()
}

pub fn is_floating(window: &Window) -> bool {
    data(window).floating
}

pub fn is_fullscreen(window: &Window) -> bool {
    data(window).fullscreen
}

/// Neither floating nor fullscreen: the windows the split tree arranges.
pub fn is_tiled(window: &Window) -> bool {
    let data = data(window);
    !data.floating && !data.fullscreen
}

pub fn rect(window: &Window) -> Rectangle<i32, Logical> {
    data(window).rect
}

pub fn set_rect(window: &Window, rect: Rectangle<i32, Logical>) {
    data_mut(window).rect = rect;
}
