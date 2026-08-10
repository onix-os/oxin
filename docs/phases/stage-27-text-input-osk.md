# Stage 27 — Focused Text Input and System OSK

**What it is.** This stage lets ordinary applications request the configured
session keyboard through Wayland text-input-v3 while retaining the existing
manual gesture and replaceable keyboard commands.

**Phase gate.** *A focused Patin network-settings field automatically shows
wvkbd, injected keys reach that field without moving keyboard focus, and
ending editing or closing the window hides wvkbd.*

## Protocol boundary

0xin publishes `zwp_text_input_manager_v3` through wlroots. A compositor-side
relay watches the seat's single keyboard-focus signal, sends text-input enter
and leave to clients belonging to the focused surface, and derives keyboard
visibility only from an enabled text input that still owns focus.

The relay deliberately does not name wvkbd. It invokes the existing
`virtual_keyboard_show` and `virtual_keyboard_hide` commands, so the configured
provider remains replaceable. `zwp_virtual_keyboard_manager_v1` continues to
carry key and modifier events from that provider to the focused client.

Focus loss, text-input disable, client destruction, and session-lock focus all
hide the OSK. Manual `keyboardshow` and `keyboardhide` actions remain available
as overrides; automatic and gesture requests share the same visibility state
and gesture-area placement.

## Implementation boundary

`shim/text_input.c` owns wlroots listener lifetimes and client/surface matching.
Rust receives only a boolean visibility transition and feeds it into the same
controller used by key and gesture actions. This follows the existing rule
that intrusive Wayland listener plumbing remains in C while policy stays in
Rust.

Text-input-v3 describes the focused field; it is not the OSK implementation.
wvkbd remains the FP5 reference provider, and a future Patin keyboard can
replace it without changing applications or compositor policy.

## Verification

Local verification on 2026-08-10:

```text
cargo check --all-targets
  succeeded with the existing build_dwindle dead-code warning
cargo test --all-targets
  36 passed; 0 failed
mdbook build
  HTML book written to book/
git diff --check
  no output
```

`cargo clippy --all-targets -- -D warnings` still stops on two pre-existing
findings outside this stage: the test-only `build_dwindle` helper is dead in a
non-test build, and `session_lock.rs` compares a boolean expression with
`false`. No text-input code produced a Clippy diagnostic.

The updated compositor also built natively on the FP5 with the existing local
development sysroot (`PATH`, `PKG_CONFIG_PATH`, `LIBCLANG_PATH`, and
`LD_LIBRARY_PATH` pointed inside `~/proj/0xin/.sysroot`). A temporary headless
instance started and shut down cleanly. Its registry advertised
`zwp_text_input_manager_v3` version 1 and `xdg_wm_base` version 6; the installed
Patin network-settings binary discovered and bound both.

The new executable is at the same `target/debug/0xin` path greetd launches.
The current login continues using its already-open old executable inode, so a
normal logout/login is required before the interactive show/type/hide phase
gate can be observed. The live graphical session was not terminated remotely.
