# Stage 0 — Foundation & FFI

**What it is.** The very first thing any wlroots compositor needs: a linked
wlroots, a backend, a renderer, and something on screen. No Wayland clients
exist yet — this stage is purely about getting Rust and wlroots talking to
each other at all.

**Why it matters.** Every later stage depends on the FFI shape decided here.
wlroots is a C library built around signals (`wl_signal`/`wl_listener`) and
structs with fields Rust can't safely see without help — get the FFI
strategy wrong here and every subsequent stage inherits the pain. This is
also where the Rust/C-shim split described in [Architecture](../architecture.md)
was decided, not assumed up front.

**Deliverable** (from `KICKOFF.md`): *link wlroots from Rust (bindgen vs C
shim, decided together); open a wlroots backend (nested) and a renderer;
clear the screen to a solid color; structured logging. A nested window shows
a solid color — "0xin alive."*

## How it went

The FFI strategy landed on **both**, not one or the other: `bindgen`
generates the raw function/type bindings from an explicit allowlist in
`build.rs`, and a thin C shim (`shim/`) handles the parts bindgen can't make
safe — signal/listener glue and opaque struct field reads. That split, made
here, held for every stage after.

`build.rs` came together as a four-step pipeline: resolve wlroots/wayland
flags via `pkg-config`, generate the `xdg-shell` protocol header with
`wayland-scanner`, compile the shim with the `cc` crate, then run `bindgen`
over `wrapper.h`. `wlr_backend_autocreate` was the first real wlroots call —
it inspects the environment and picks the nested Wayland backend when run
inside an existing session, which became the fast dev loop for every stage
from here on (see [Running & Verifying](../running.md)).

**Status: done.** `cargo nested` opens a window and clears it to a solid
color, with `oxide_log_init()` wiring wlroots' own debug log to stderr.
