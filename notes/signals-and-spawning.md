# Signals & spawning clients

The compositor's signal state **leaks into every client it spawns** unless
explicitly reset between fork and exec. Two things survive exec:

- **`SIG_IGN` dispositions** (handlers reset to default on exec; ignores
  don't — POSIX). We ignore SIGCHLD for auto-reaping, so clients inherited
  "SIGCHLD ignored" → the kernel auto-reaps *their* children before they can
  read exit codes. Symptom that found it: quickshell's recording indicator
  (a 1 Hz `pgrep -x wf-recorder` whose QProcess exit code came back 0
  regardless) showed a permanent red "recording" dot under 0xin only.
- **The blocked-signal mask.** libwayland's `wl_event_loop_add_signal`
  (signalfd) blocks SIGINT/SIGTERM in our process; clients inherited them
  blocked, so a plain `kill -TERM` to a 0xin-spawned client sat pending
  forever (`SigBlk 0x4002` in /proc/PID/status).

Fix: `oxide_reset_child_signals()` in `shim/core.c` — `SIGCHLD → SIG_DFL` +
`sigprocmask(SIG_SETMASK, empty)` — called via `Command::pre_exec` from
**every** Rust spawn path (`main.rs` client arg, `keybindings::spawn`; the
helper is `keybindings::reset_signals`). Any new spawn path must go through
it too.

Diagnosis recipe: `grep -E 'SigIgn|SigBlk' /proc/<client-pid>/status` —
SIGCHLD is bit 17 (mask 0x10000), SIGTERM bit 15 (0x4000), SIGINT bit 2
(0x2). Compare against the same app launched from the normal desktop.

Compositor-side reaping is unchanged (`SIGCHLD SIG_IGN` in
`oxide_setup_signals`); revisit if wlroots' XWayland integration ever needs
to wait on its own child.
