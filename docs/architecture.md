# Architecture: the two-plane split

0xin is really *two* systems layered on top of each other, with a hard rule
about who owns what — the same discipline the [Introduction](introduction.md)
described one level up, between 0xin and wlroots, repeats itself inside
0xin's own source tree.

## The ownership contract

- **Rust owns policy and flow.** Everything in `src/`: the window list, the
  tiling layout, workspaces, keybindings, config parsing, and the overall
  program flow — building the display/backend/renderer/allocator, wiring the
  scene graph, running the event loop. `Server` (the top-level state struct)
  lives in Rust and gets passed to C as an opaque `userdata` pointer.
- **A thin C shim owns the awkward FFI.** Everything in `shim/`: wlroots'
  `wl_listener`/`wl_signal` glue, and anything that needs to reach into a
  wlroots struct's actual fields.

That second point is the *why*. wlroots types are largely **opaque to Rust**
— bindgen, pointed at the real headers, emits most structs as an anonymous
`_address` byte blob rather than named fields, because the C side uses
patterns (intrusive linked lists via `wl_container_of`, unions, bitfields)
that don't have a safe 1:1 Rust representation. Two consequences follow
directly:

1. **Listener wiring goes through the shim.** wlroots signals you (a new
   output appeared, a surface mapped, a key was pressed) via `wl_signal` +
   `wl_listener` — an intrusive C list threaded through the struct you're
   listening on. The shim wraps this once, generically, as a `signal_add`
   helper that exposes a plain `(userdata, data)` callback to Rust. Rust
   never touches a `wl_listener` directly.
2. **Struct field reads go through the shim.** If Rust needs something living
   *inside* an opaque wlroots struct — an output's `width`/`height`, a
   surface's role, an array literal like `wlr_scene_rect_create`'s `const
   float[4]` — that read (or write) happens in a small C function in `shim/`,
   which returns a plain value or plain pointer that Rust *can* represent
   safely.

Everything else — creating a `wlr_scene`, adding an output to a
`wlr_output_layout`, tying a layout slot to a scene output — is a plain
function call with plain pointer arguments, so it stays directly in Rust with
no shim wrapper at all.

**Rule of thumb:** if it needs a wlroots struct's insides, a C array literal,
or the listener list, it goes in the shim; otherwise it stays in Rust.

## bindgen and the shim, concretely

`build.rs` runs `bindgen` over `wrapper.h` with an **explicit allowlist** —
`.allowlist_function(...)`, `.allowlist_type(...)` — so Rust only sees the
slice of the wlroots API 0xin actually calls, rather than the whole (huge,
partially-unstable) surface. The same `build.rs` also compiles `shim/*.c` via
the `cc` crate and generates the `xdg-shell` protocol header with
`wayland-scanner`, since wlroots' own xdg-shell header `#include`s it and
it's not a system header.

The shim itself is split one file per protocol/concern (`shim/output.c`,
`shim/input.c`, `shim/xdg_shell.c`, `shim/layer_shell.c`,
`shim/decoration.c`, `shim/core.c`), each declared in one header
(`shim/oxide_shim.h`) that `src/ffi.rs` mirrors with `extern "C"` decls.

## Where this shows up in the tree

| Path                    | What it is                                                         |
| ------------------------ | ------------------------------------------------------------------ |
| `src/main.rs`            | Orchestration: builds the compositor, runs the event loop          |
| `src/state.rs`           | `Server`, `Output`, `Toplevel`, `Workspace` — the Rust-owned state  |
| `src/config.rs`          | Dependency-free config-file parser                                 |
| `src/layout.rs`          | The split tree (`Node`) and its pure operations — insert/remove/resize |
| `src/tiling.rs`          | Tiling orchestration: syncs the tree to live state, directional focus/move, layer arrangement |
| `src/output.rs`, `input.rs`, `toplevel.rs`, `layer_shell.rs`, `decoration.rs`, `keybindings.rs` | Per-concern policy modules |
| `src/ffi.rs`             | `extern "C"` declarations mirroring `shim/oxide_shim.h`             |
| `shim/*.c`, `shim/oxide_shim.h` | The C shim — listener glue + opaque struct field access     |
| `build.rs`, `wrapper.h`  | The FFI pipeline: pkg-config, wayland-scanner, cc, bindgen          |

This division isn't fixed in stone — it's a rule of thumb refined while
building each stage, not a spec written up front. See the [build
phases](phases/README.md) for how it was actually arrived at.
