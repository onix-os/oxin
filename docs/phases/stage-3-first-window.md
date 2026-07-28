# Stage 3 — First Window

**What it is.** The stage where 0xin stops being an empty colored rectangle
and starts hosting real applications: **xdg-shell**, the protocol real apps
(terminals, browsers) use to become an app window ("toplevel") rather than a
bare surface.

**Why it matters.** Nothing downstream — tiling, focus, keybindings — means
anything without a real window to apply it to.

**Deliverable** (from `KICKOFF.md`): *map a real client surface. A terminal
(`foot`) appears in 0xin.*

## How it went

`wlr_xdg_shell_create` advertises the `xdg_wm_base` global apps bind to; its
`new_toplevel` signal is hooked (via the shim, since it's a
`wl_signal`/`wl_listener`) to `handle_new_toplevel`, which puts the new
window's surface into the scene graph built in Stage 2.

Two gotchas surfaced here, both now folded into the working notes:

- **wlroots' xdg-shell header itself needs a generated file.** It
  `#include`s `xdg-shell-protocol.h`, which isn't a system header —
  `build.rs` generates it with `wayland-scanner` into `OUT_DIR` as part of
  the FFI pipeline from [Stage 0](stage-0-foundation.md).
- **A real client refuses to start without a seat.** `foot` failed with "no
  seats available" until a minimal `wl_seat` (`oxide_seat_create`) existed —
  input handling proper doesn't land until [Stage 4](stage-4-input.md), but
  the *global* has to exist earlier than that for any real app to even try
  connecting.

**Verified with:** `cargo nested -- foot` — a terminal appears in the nested
window.

**Status: done.**
