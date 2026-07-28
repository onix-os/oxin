# Stage 11 — Runtime Control

**What it is.** A control socket (IPC): query the compositor's state and
drive it from outside — scripts, status bars, one-off shell commands —
without everything having to be a keybinding.

**Why it matters.** Keybindings cover interactive use; a socket covers
everything else: a bar showing the active workspace, scripted window
arrangements, toggling settings without editing the config and restarting.
It's also the natural place for a `0xinctl`-style command tool.

**Deliverable** (from `KICKOFF.md`): *a shell script lists windows and
switches workspaces without touching a keybinding.*

## Status

**Not started.** Design is open — protocol shape (plain text vs JSON), what
state to expose, and whether config reload belongs here too will be decided
when the stage begins.
