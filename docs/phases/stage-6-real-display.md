# Stage 6 — Real Display (DRM/KMS)

**What it is.** Moving off the nested Wayland backend and onto **real
display hardware**: a bare virtual terminal, DRM/KMS modesetting, and a
real login session via libseat, instead of a window inside someone else's
compositor.

**Deliverable** (from `KICKOFF.md`): *run on a VT via seatd/libseat;
multi-output layout and modesetting. 0xin as a real session on
hardware/VM.*

## How it's gone so far

`wlr_backend_autocreate` already picks the DRM/KMS backend automatically
when there's no `WAYLAND_DISPLAY` to detect — no separate code path was
needed, just a different environment to run in. On a bare TTY:
`LIBSEAT_BACKEND=logind ~/Projects/0xin/target/debug/0xin foot
2>~/0xin-tty.log`, with `LIBSEAT_BACKEND=logind` letting logind hand the
active VT its devices without a `seat` group membership.

Two real bugs came out of this that a nested session can't surface at all,
since nesting never tears down or re-modesets an output:

- **VT switching crashed on output destroy.** Switching away and losing the
  session mid-flight needed proper output-destroy handling — removing the
  frame/destroy listeners and background scene node *before* wlroots
  finishes tearing the output down, or wlroots asserts on a non-empty frame
  listener list.
- **Returning from a VT switch came back black.** The outputs aren't
  destroyed on a VT switch — they're re-modeset to black — and idle clients
  never repaint on their own, so regaining the VT showed nothing. The fix
  hooks the session's active-change signal: on resume, every window's scene
  node is torn down and recreated (a client's buffer survives, but its old
  scene node stops presenting it after the modeset), then a few forced
  repaints are scheduled per output so the freshly-rebuilt scene actually
  gets painted once the output is back.

Multi-output tiling (per-output workspaces, focus-follows-monitor,
config-driven position/scale) was built and verified nested first — see
[Stage 5](stage-5-window-management.md) — and confirmed working the same
way on real hardware.

## Status

**In progress / substantially working.** Single and multi-display both run
on real hardware, VT switching survives without crashing or losing windows,
and config-driven monitor placement matches real connector names and
dimensions. Not yet covered: hotplug removal mid-session beyond the
already-handled destroy path, and further real-hardware edge cases as they
turn up.
