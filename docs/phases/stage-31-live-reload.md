# Stage 31 — Live Config Reload

**What it is.** Changing `init.lua` changes the running session. Save the file
and 0xin reloads it, with no restart and no lost windows. `0xinctl reload` and
`oxin.action.reload` do the same on demand.

**Gate:** *Saving a new `oxin.gap` and `oxin.background` re-tiles and recolours a
session with windows open; saving a syntax error changes nothing on screen and
is reported by `0xinctl config-error`; fixing it clears the report; and one
editor save is one reload.*

## The swap is the easy half

`lua::session::start()` already built a complete, independent Lua layer: a new
VM, a new registry of bound functions and handlers, and a finished `Config`. It
touched nothing live. A reload runs the same load again (`session::reload`) and
swaps all three in (`src/reload.rs`):

- The VM and registry are `main` locals reached through the `LuaLink` raw
  pointers ([Stage 28](stage-28-lua-configuration.md) explains why they are not
  `Server` fields), so they are replaced in place with `mem::replace`. The old
  registry goes first, since its stashed handles root into the old VM.
- They are replaced *together*, because an `Action::Lua(id)` in a binding is an
  index into the registry its config built. A new config with old functions,
  or the reverse, would run the wrong function on a keypress.
- A half-held hold binding is cancelled first: `held_action` may carry one of
  those ids into the registry being dropped.
- A reload refuses to run while a Lua handler is on the stack, because
  replacing the VM would free the stack the handler runs on. Nothing reaches
  reload from Lua today, since `oxin.action.reload` is dispatched by the
  compositor and not called, so this is a guard and never a code path.

Everything the compositor looks up per event then sees the new config with no
further work: key, hold and gesture bindings, the modifier, `float_size`, the
keyboard commands, and the `on.window` / `on.workspace` handlers.

## Settings that are read once

The other half is the settings that shape something only when it is *created*.
A new `Config` does not reach those, so `reload::apply` re-applies each one:

| Setting | Read once by | Re-applied by |
|---|---|---|
| `gap` | the last re-tile | `tiling::refresh` |
| `first_split` | each `Workspace`, copied at startup | written to every workspace; new splits only |
| `window_opacity`, `corner_radius` | each window's next commit | `toplevel::apply_decorations` for every window, so idle ones change too |
| `background` | each output's rect, made on connect | `oxide_scene_rect_set_color` (new) |
| `wallpaper` | each output's node, made on connect | `wallpaper::set`, the path `0xinctl wallpaper` uses |
| `gesture_handle` | each output, on connect | `output::apply_config` creates or removes it |
| gesture mask, keyboard height | the shim, once in `main` | `oxide_cursor_set_gestures` again |

`first_split` is applied to new splits only. Reshaping a layout the user has
already arranged would be a surprise, not a setting. A wallpaper that fails to
decode keeps the old picture and is logged, without failing the rest of the
reload.

## A failure keeps what you have

At startup a broken config falls back to the built-in defaults, because the
alternative is a black screen. A reload reverses that. The session is already
running a config that works, and swapping it for defaults would take away every
binding because of one typo in a file that is still being edited. So
`session::reload` returns an `Err` and nothing is swapped. The message is
printed in the same block as a startup error, ending in "keeping the previous
config", and stored for `0xinctl config-error` as "reload failed; still running
the previous config". The next good reload builds a fresh registry, which has no
error, so the report clears itself.

A *missing* `init.lua` is treated as a failure too. Some editors save by
deleting and renaming, and a file can be moved aside for a moment; neither means
"run with no config".

## Watching for saves

`src/config_watch.rs` holds one inotify fd on the Wayland event loop. The record
parsing is in `shim/config_watch.c`, because the variable-length
`inotify_event` records are awkward to walk from Rust with no libc bindings.

- **Directories, not files.** Editors write a temporary file and rename it
  over the original, and a watch on the original file is lost with it.
  `plugins::watch_dirs` lists the config's directory and every root's
  `plugin/`, `after/plugin/` and `lua/` trees, packages' included, recursively,
  the same places `plugin_files` and `require` read from.
- **Only `.lua` matters.** A change to any other file is ignored. A directory
  event counts, because a newly created `plugin/` changes what there is to
  watch, and so does a queue overflow, which may have hidden a save. The
  directory list is rebuilt after every reload, so new directories start being
  watched.
- **Debounced.** One save is several events (write, rename, sometimes a backup
  file). Each relevant event restarts a 200 ms timer, and the reload runs once
  the burst is quiet. Overwriting in place (`sed -i`) and write-then-rename
  both reload exactly once.
- **Not while locked.** A reload refuses to run under a lock screen, as all
  mutating `0xinctl` commands do. The shim's gesture mask is zeroed while
  locked, and resetting it from a reload would re-enable gestures under the
  lock. A save seen while locked is logged with a pointer to `0xinctl reload`.

## How it was verified

Headless, per [Running & Verifying](../running.md), with two quickshell
`FloatingWindow`s tiled side by side and `grim` between saves:

- `sed -i` from `gap = 4` and a red background to `gap = 40` and green: one
  `reloaded` line, and the screenshot showed the wide gaps and the green.
- Write-then-rename to a blue background and `window_opacity = 0.5`: one
  reload. Both windows, which had not redrawn, were dimmed.
- `oxin.gap = ` (a syntax error): the screen was unchanged, and
  `0xinctl config-error` printed the parse error under "still running the
  previous config". The next good save cleared it.
- Creating `plugin/` at runtime triggered a reload that started watching it.
  Writing `plugin/p.lua` then reloaded with the plugin applied.
- Turning `gesture_handle` on with a `bottom-up` gesture drew the handle, and
  turning it off removed it. An unreadable `oxin.wallpaper` was logged and the
  old background kept.
- `0xinctl reload` answered `ok` for a good file, and `error …` for a broken
  one or a missing one.
- Unit tests cover the reload policy (a broken or missing file is an `Err`, a
  good one comes back), `watch_dirs` on a sample layout, and `oxin.action.reload`.

The lock-screen refusal was not exercised headless, since that needs a locker
client. It is a check on `server.locked` ahead of everything else.

## Boundaries

- **`on.startup` is not run again**, so a reload does not launch your bar a
  second time. Changing what starts with the session needs a restart.
- **`oxin.monitors`** applies when a monitor next connects. A changed table is
  logged as such.
- **`corner_radius` set to 0** only unrounds an idle window at its next
  commit. The masked buffer is in the scene in place of the client's, and only
  the client can supply an unmasked one.
- **A wallpaper set by `0xinctl wallpaper`** is replaced by the config's on
  reload, when the two differ. The config is the source of truth.
- **A key released after a reload that unbound it** reaches the client as a
  release without a press, which clients ignore.
- **`--noplugin`** still watches the plugin directories. A save there reloads
  without plugins, which is harmless.
