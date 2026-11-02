# Stage 32 — Workspaces the Hyprland Way

**What it is.** Workspaces and monitors now follow Hyprland's model rather
than XMonad's. A workspace belongs to one monitor. Workspaces are made when
something asks for one and dropped when they are empty and off screen.
Plugging a monitor in or out moves workspaces around the way Hyprland does,
and puts them back when the monitor returns.

**Gate:** *With two monitors, switching to a workspace on the other monitor
moves focus there instead of pulling the workspace over. A newly plugged
monitor gets a fresh workspace, not a hidden one with windows on it.
Unplugging hands that monitor's workspaces to the remaining one. Plugging it
back shows what it showed before. A floating window stays on the monitor its
workspace is on, through all of it.*

## What changed, and why

Until now 0xin had nine fixed, global workspaces ([Stage 5](stage-5-window-management.md)).
MOD+N brought workspace N to the monitor under the cursor, and if another
monitor was showing it, the two swapped. That is XMonad's `greedyView`, and
Hyprland's own *opt-in* `focus({ workspace = N, on_current_monitor = true })`.
Hyprland's *default* is different, and that default is what 0xin follows now.
The decisions below are taken from Hyprland's source
(`src/state/workspace/LifecyclePolicy.cpp`, `PlacementController.cpp`,
`Resolver.cpp`, and `src/config/shared/actions/ConfigActions.cpp`), not
from its documentation, so they behave the same.

| | Before | Now (as Hyprland) |
|---|---|---|
| How many | 9, always | as many as are in use |
| A workspace belongs to | no monitor | one monitor |
| MOD+N, N on the other monitor | the monitors swap | focus and cursor move to that monitor |
| MOD+N, N does not exist | — | made on the focused monitor |
| Plug a monitor in | lowest workspace not on screen, windows and all | lowest number no workspace has, so an empty one |
| Unplug a monitor | its workspace goes hidden | its workspaces move to the first remaining monitor, hidden |
| Plug the same one back | whatever is lowest and unshown | its workspaces come back, and it shows the one it showed |
| Move a window to a workspace | you stay | you follow it (`{ follow = false }` to stay) |
| Next / previous workspace | 1–9, wrapping | Hyprland's `r+1` / `r-1` |

## The model

`src/workspaces.rs` holds the policy. `src/state.rs` holds the data:

- **`Workspaces`** is a map from number to `Workspace`, ordered by number,
  and indexed by number (`server.workspaces[id]`). A number can be any value
  up to Hyprland's 32-bit limit, so the numbers in use can have gaps. The
  numbers are 0-based internally and 1-based wherever a user sees them, as
  before.
- **`Workspace.monitor`** is the connector name of the output it belongs to.
  It is a name, not an output index, because outputs come and go and a name
  is how the same monitor is recognised when it returns.
  **`Workspace.return_to`** marks where a workspace goes back to after its
  monitor was unplugged. **`Server.remembered_workspace`** records what each
  unplugged monitor was showing.
- **`prune`** drops every workspace that is empty and on no output. That is
  the moment a Hyprland workspace stops existing. It runs at the start of
  every `tiling::refresh`, which is the one place every change already goes
  through. Any code still holding a workspace's number is holding one that is
  on screen or has windows, so the number stays valid. A dropped workspace is
  also forgotten as anyone's remembered workspace, so a later workspace that
  reuses the number is not mistaken for it.

### Switching (`focus`)

This is Hyprland's `changeWorkspace`. The workspace is made on the focused
output if it does not exist. Its owner then shows it. If the owner is another
output, the cursor moves to that output's centre. The focused output is
whichever one the cursor is on (`tiling::active_output`), so moving the
cursor moves focus. `oxide_cursor_warp` in the shim processes the warp as
a motion, so pointer focus is updated too. Keyboard focus goes to the
workspace's focused window, or to nothing if the workspace is empty. Before
this change, switching to an empty workspace left keys going to the window
you had just switched away from.

### Next and previous (`relative`)

Hyprland has several relative selectors. `oxin.action.workspace_next` and
`_prev` use `r+1` / `r-1`: the next number on this monitor, counting numbers
that do not exist yet and skipping those another monitor has. Going up
never wraps and goes on to new numbers. Going down stops at the lowest one
available. On a single monitor, swiping through workspaces works as before,
except that it no longer wraps from 9 back to 1.

### Plugging in (`output_connected`)

Hyprland's `monitorConnected`, in its order:

1. Workspaces whose `return_to` is this connector move back to it.
2. If this is the only output, any workspace whose own output is gone moves
   to it, so windows are never left with nowhere to be shown.
3. If this connector was showing a workspace when it was unplugged, and that
   workspace still exists, it is shown again. If it is on screen elsewhere,
   that output switches to another of its own workspaces, or a new one, as
   Hyprland's `moveWorkspaceToMonitor` does.
4. Otherwise the output gets the lowest number no workspace has.

### Unplugging (`output_disconnecting`)

Hyprland's `monitorDisconnected`. The output records what it was showing,
marks its workspaces to return to it, and gives them, hidden, to the first
remaining output. That output keeps showing what it was showing. The cursor
moves to the middle of the remaining output and keyboard focus follows. With
no output left, keyboard focus is cleared, the workspaces stay as they are,
and the next output to arrive takes them in.

### Floating windows follow their workspace

`refresh` used to leave floating windows wherever their scene node was. Their
position is in layout coordinates, so a floating window kept its old position
whenever its workspace moved to another monitor: on the wrong monitor, or off
every monitor. `refresh` now positions them from the recorded
`Toplevel.x/y`. Moving a workspace to another output (`move_to_output`), or
moving a window to a workspace on another output, translates that position by
the difference between the two outputs, and clamps it to the new usable area.
When the old output is gone, the position is wrapped into the new output, as
Hyprland does.

### Moving windows

`oxin.action.move_to_workspace(n)` now follows the window, which is
Hyprland's `movetoworkspace`. `move_to_workspace(n, { follow = false })` is its
`movetoworkspacesilent`. An unknown option or a non-boolean `follow` raises,
like every other config typo. `move_to_workspace_next` and `_prev` follow too.
The default keymap gains MOD+0 and MOD+SHIFT+0 for workspace 10, as in
Hyprland's example config.

## Other surfaces

- **`0xinctl workspaces`** lists only the workspaces that exist, in number
  order, and adds the owning output: `workspace 3 occupied HDMI-A-1`. Patin's
  parser ignores fields after the state and fills in gaps, so it keeps
  working.
- **`oxin.workspaces()`** likewise returns only the workspaces that exist,
  each with `monitor` alongside `number` and `windows`.
- **`oxin.action.workspace(n)`** accepts any positive number.
- **`0xinctl headless-output add WxH | remove NAME`** plugs and unplugs a
  monitor on the headless backend, so all of the above can be tested without
  hardware. It refuses real outputs.

## Found along the way: keyboard focus under the lock screen

`keybindings::focus_index` did not check `server.locked`. A window that
mapped while the session was locked therefore took keyboard focus, and keys
typed at the lock screen went to that window. It now does nothing while
locked, and so does the new `workspaces::focus_windows_of`. Unlocking still
refocuses the active workspace, as before.

## How it was verified

- **Unit tests** in `src/workspaces.rs` drive the connect and disconnect
  policy on plain `Server` data:
  - a new output gets a fresh workspace and not a hidden one;
  - empty workspaces disappear;
  - unplugging and replugging round-trips;
  - a returning workspace that is on screen elsewhere is replaced there;
  - a different monitor arriving after none takes in the orphaned
    workspaces;
  - next and previous skip the other monitor's workspaces;
  - floating windows keep their offset.

  Lua tests cover `follow`, its typo checks, and workspace numbers past 9.
- **Headless, end to end**, with quickshell windows placed by `on.window`
  rules and `0xinctl headless-output` (see [Running & Verifying](../running.md)):
  - A second monitor got workspace 3, not the hidden workspace 2.
  - A floating window sent to 3 was drawn on the second monitor.
  - Unplugging moved 3 to the first monitor, hidden, and the first monitor
    kept showing 1.
  - Unplugging the last monitor kept every window.
  - The next monitor took them all in.
- **The key actions**, through a throwaway build that called
  `dispatch_action` from the control socket. Virtual keyboards deliberately
  never trigger compositor bindings, so `wtype` cannot do it. The build was
  not committed. On two monitors:
  - `workspace 2` moved focus to the monitor showing 2 without swapping.
  - The next new workspace was made on that monitor, which shows the cursor
    really moved.
  - Next and previous skipped the other monitor's workspace and dropped the
    empty one they left.
  - Moving a window followed it, and with `follow = false` stayed.

## Boundaries

- **Not yet:** workspace rules (`workspace = 3, monitor:DP-1, default:true`),
  named and special workspaces, `workspace_back_and_forth`, and Hyprland's
  other relative selectors (`m+1`, `e+1`, `+1`).
- **`on.window`'s `workspace`** still places a window silently, as before.
  Hyprland's window rule switches to the workspace unless `silent` is given.
- **On a single monitor**, everything works as before except three things:
  next no longer wraps from 9 to 1; MOD+N to an empty workspace clears
  keyboard focus; and moving a window, including the FP5's three-finger
  swipe, now takes you along.
