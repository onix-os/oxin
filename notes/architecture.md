# Architecture: Rust / C-shim division of labor (as of Stage 2)

0xin is Rust-first with a thin C shim (`shim/oxide_shim.c`) for the FFI that
is unsafe or awkward over bindgen. The dividing line, learned in practice:

**Rust (`src/main.rs`) owns the flow and clean pointer-passing:**
- builds display/backend/renderer/allocator/globals/socket, runs the event loop
- scene-graph *wiring* — `wlr_scene_create`, `wlr_output_layout_*`,
  `wlr_scene_output_create`, `wlr_scene_output_layout_add_output` (all plain pointers)
- holds `Server` state; passes `&mut Server` (or a wlroots pointer) to the shim as
  `userdata` for each callback

**C shim owns the intrusive / awkward bits:**
- `wl_signal`/`wl_listener` glue via one generic `signal_add` + `wl_container_of`,
  exposed to Rust as a plain `(userdata, data)` callback
- anything needing a wlroots **struct field** — Rust sees most wlroots types as
  *opaque* (bindgen emits only `_address`), e.g. `wlr_output.width/height`. Reading
  fields happens in C (`oxide_scene_add_output_background`, which sizes the bg rect).
- C-array / time params: `wlr_scene_rect_create`'s `const float[4]`, and
  `clock_gettime` + `timespec` for `wlr_scene_output_send_frame_done`
  (both inside `oxide_scene_output_render`).

**bindgen** (allowlist in `build.rs`) generates only the functions Rust calls
directly; types come along automatically and are opaque unless fully needed.

Rule of thumb: if it needs a wlroots struct's *insides*, a C array literal, or the
listener list, it goes in the shim; otherwise it stays in Rust.
