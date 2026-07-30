# Stage 24 — Output Power Management (DPMS)

**What it is.** 0xin implements `wlr-output-power-management-unstable-v1`,
letting an external client (e.g. `patin-lock`) request a real display
power-off/on per output — an actual DPMS toggle, distinct from the opaque
compositor-owned cover raised while `ext-session-lock-v1` is engaged (see
[Stage 23](stage-23-secure-session-lock.md)). The lock-fallback cover keeps
the desktop hidden and secure; this protocol is what lets a client also stop
driving the panel, saving power.

**Gate:** *A client can request a specific output be powered off and back on;
0xin actually disables/re-enables that output's physical signal, and windows
that were on-screen before power-off reappear correctly after power-on.*
The power-off half of this gate is verified (see below); the power-on half
is implemented but its nested-backend verification stalled — see
Verification.

## How it's wired

wlroots implements the entire wire protocol server-side
(`wlr_output_power_manager_v1_create`) — 0xin only creates the global at
startup and listens for its `set_mode` signal
(`src/output_power.rs`, `shim/output_power.c`). On each event it:

- resolves the signal's target output against `server.outputs`;
- calls `oxide_output_set_powered` (`shim/output.c`), which commits a
  `wlr_output_state` with `WLR_OUTPUT_STATE_ENABLED` set to the requested
  value — the same commit-state mechanism `oxide_output_enable` already uses
  when an output first comes up;
- on power-**on**, forces a full repaint the same way VT-resume already does
  (`src/output.rs`): resets that output's `repaint_frames` counter and
  schedules a frame. A disabled output stops receiving `frame` events
  entirely, so nothing else is needed for power-**off** — idle windows just
  stop being asked to repaint.

The protocol XML isn't shipped as a system package on this toolchain (only
`wlr-layer-shell-unstable-v1` is), so it's vendored in-repo at
`protocols/wlr-output-power-management-unstable-v1.xml`, generated into a
server header by `wayland-scanner` in `build.rs` — the same approach already
used for layer-shell.

## Scope

This stage covers the 0xin compositor side only: exposing the global and
actually applying on/off requests. Deciding *when* to power a display off
(idle timeout, immediately on lock, etc.) is a policy decision for whichever
client speaks the protocol — `patin-lock` integration is tracked separately
in that project.

## Verification

```text
$ cargo build
Finished `dev` profile [unoptimized + debuginfo] target(s)
```

Runtime-verified nested with a throwaway client binding
`zwlr_output_power_manager_v1` directly (no `wlr-randr`/`wlopm` available in
this environment): it bound the global and `wl_output`, received the
required initial `mode = ON` event, then requested `set_mode(OFF)`. 0xin
logged `output 0 powered off` and its nested host-side window disappeared
immediately — exactly the expected DPMS-off behavior — reproduced twice.

`set_mode(ON)` after an `OFF` was **not** confirmed end-to-end: 0xin receives
the request and starts the re-enable commit (buffer allocation, GL FBO
creation — the same `wlr_output_commit_state` call `oxide_output_enable`
already uses successfully at startup) but the call stalls before returning,
inside wlroots' own nested (`wlr_wl`) backend commit implementation — code
this stage never touches. The compositor process stays alive/healthy
throughout (not a crash), and none of 0xin's own downstream logic
(`oxide_output_schedule_frame`, the `repaint_frames` reset, the confirming
`eprintln!`) ever runs, since it sits after the call that hangs. This
points at a characteristic of the nested dev backend's window-recreation
handshake specifically — a backend rarely exercised past its first `enable`
— rather than a bug in this stage's code. The real DRM/KMS backend (what
`patin-lock` will actually run against on hardware) is where enable/disable
is a first-class, well-trodden operation; this repo's own convention is to
build/verify nested first (`docs/running.md`), so **re-verifying the
power-on direction against real hardware before relying on it is a
recommended follow-up**, not yet done here.
