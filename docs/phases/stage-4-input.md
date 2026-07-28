# Stage 4 — Input

**What it is.** Wiring real keyboards and pointers into the seat created (as
a bare global) back in [Stage 3](stage-3-first-window.md), and routing their
events to the right client — keyboard focus, pointer focus, and the xkb
layer that turns raw scancodes into actual keysyms.

**Why it matters.** A tiling window manager is defined by what the keyboard
and mouse *do* — without real input routing, there's no way to drive
anything built afterward: no keybindings, no click-to-focus, no directional
navigation.

**Deliverable** (from `KICKOFF.md`): *seat, keyboard via xkb, pointer, focus
routing. I can type into and click the terminal.*

## How it went

New input devices arrive via the backend's `new_input` signal
(`handle_new_input` in [`src/input.rs`
](https://github.com/termworks/0xin/blob/main/src/input.rs)), routed by device
type — keyboards get an xkb keymap and are attached to the seat;
pointers/touch devices are attached to the cursor set up in `main()`
(`oxide_cursor_setup`), which sits over the output layout and routes motion
through the scene graph's own hit-testing to figure out which surface is
under the pointer.

With no synthetic-input tool available in the dev environment, this stage
established the verification split that [Running & Verifying](../running.md)
describes in full: wiring is checked via log markers ("keyboard attached",
"keyboard focus -> toplevel"), actual typing/clicking is checked by hand by
focusing the nested window on the host desktop.

**Status: done.** Click-to-focus and keyboard input both work; this is also
where `src/keybindings.rs` starts existing as a module, even though the
*configurable* keybinding system proper is a [Stage 5](stage-5-window-management.md)
deliverable.
