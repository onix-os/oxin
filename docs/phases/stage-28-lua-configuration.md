# Stage 28 — Lua Configuration

**What it is.** Replacing the line-based `0xin.conf` parser with a real
configuration *language*: `~/.config/0xin/init.lua`, run by an embedded
interpreter, with keybindings that can be Lua functions, handlers that fire on
compositor events, and a neovim-shaped plugin path.

**Deliverable.** 0xin is configured and scripted in Lua. The old format and its
parser are gone.

## Why

The `.conf` parser worked, and its limits were structural rather than
cosmetic. It was a closed vocabulary: every new capability meant a new key, a
new parse arm and a new test. A config could not *compute* — nine workspaces
meant eighteen near-identical lines, a binding could not depend on whether a
program was installed, and there was no way to remove a built-in binding at
all. Behaviour that wasn't already an `Action` variant was unreachable.

## The interpreter

[luna](https://github.com/onix-os/luna) — a stackless, fuel-bounded Lua 5.4
interpreter in pure Rust, a hard fork of `piccolo`. Chosen over `mlua` mainly
because it is pure Rust with no FFI, in a project whose only other runtime
dependency was an image decoder, and because "runs untrusted scripts under a
budget" is exactly the property a compositor config needs.

The honest cost: the lockfile went from 47 crates to 70, and luna is pre-1.0
with breaking changes expected on minor bumps. The mitigation is a hard
boundary — every luna type stays inside `src/lua/`, and the rest of the
compositor sees only plain Rust (`LuaLink`, `WindowRecord`, `Action::Lua(u32)`).
A breaking bump is a change to one directory.

## The API

Assign the settings, register the behaviour, return nothing.

```lua
local oxin = require("oxin")

oxin.gap = 4                                          -- assigned
oxin.keys["MOD+Return"] = oxin.action.spawn("kitty")  -- registered
oxin.on.window(function(w)                            -- reacted to
  if w.app_id == "pavucontrol" then return { float = true } end
end)
```

Settings are assigned rather than collected into a table the host merges, so a
setting the config never mentions is left alone — which is what keeps
`OXIN_MOD` and the built-in defaults working. The usual cost of that style is
that a typo (`oxin.gpa = 4`) is silently ignored, because a host generally
cannot tell one from a user stashing a helper on the module. 0xin owns the
whole namespace, so it does not accept that: every write goes through
`__newindex`, which raises and names the nearest real setting. luna's
`set_intercept_all_writes` is what makes that fire for re-assignment too —
stock Lua fires `__newindex` only for absent keys.

The four registrars (`keys`, `hold`, `gestures`, `monitors`) are keyed by the
thing they are about, so binding a chord twice replaces rather than stacks and
`"MOD+SHIFT+q"` and `"SHIFT+MOD+q"` land on the same entry. That is the same
merge-never-replace rule the `.conf` format had, now structural. `nil` removes
a binding, which the old format could not express.

Three events — `on.startup`, `on.window`, `on.workspace` — deliberately few.
The codebase offers about thirteen plausible firing points; inventing a
taxonomy up front is the usual failure. `on.window` is the one that
*influences*: `nil` means "not mine", a table means "do this instead". It
subsumes what `float = <app_id>` used to do and is strictly more capable.

`exec_once` is gone too, and did not need a replacement feature — once the
config is a list of statements, the terse form is four lines of the user's own
Lua:

```lua
local function autostart(cmd)
  oxin.on.startup(function() oxin.spawn(cmd) end)
end
```

## What was harder than expected

**luna's own run-to-completion helpers cannot bound anything.** `Lua::finish`
refills its fuel every iteration, so it loops forever on `while true do end`;
`Lua::execute` is `finish` plus a result. Calling either from the keyboard
handler would wedge the compositor permanently on a config typo. The host has
to own the loop over `Executor::step`, where running out of fuel is *resumable*
rather than an error — that is what makes stopping on our own terms possible.
The real guarantee is a wall-clock deadline, not the fuel number: fuel counts
instructions, and one `string.rep("x", 1e9)` can blow the clock inside a single
slice.

**Errors from native callbacks carry no position.** luna positions errors
raised *by Lua* (`init.lua:3: boom`) but not ones a Rust callback returns, and
a config error with no line number is the one thing a config author most needs.
The calling frame knows both, so 0xin prefixes it — with one correction:
`current_line` is attributed to the instruction *about to run*, and luna only
steps back for a `Call` opcode. `__newindex` fires on a *store*, so a failed
assignment on line 3 reported line 4 until the lookup was done against the
store opcode itself.

**Syntax errors carry no filename at all**, whatever chunk name you pass —
`ParseError`'s message is just `parse error at line N`. Tolerable for one
config and useless once plugins are on a runtimepath, so the loader prepends
the path.

**`Lua` cannot live inside `Server`.** `Server`'s address is handed to the C
shim and every callback rebuilds `&mut Server` from it. A `lua.enter(...)`
through a `Server` field would be a reborrow covering the whole struct,
invalidating that pointer's provenance. The VM and its registry are separate
`main` locals; `Server` carries only a `Copy` pair of raw pointers, and
copying that field out ends the borrow so the three can be held at once.

**Reaching `&mut Server` from a `'static` callback** is a thread-local parked
pointer with a re-entry flag beside it, so the one realistic way to get it
wrong — a host function calling another while it still holds the `&mut` — comes
back as a catchable Lua error instead of undefined behaviour. The type is
checked on pickup too, which is what caught the first wiring mistake.

Worth recording, because the plan for this stage assumed otherwise: **luna does
not forbid Rust→Lua nesting.** It ships `tests/reentrancy.rs` proving a nested
executor works. Handler dispatch is still deferred, but for compositor reasons
— the aliasing invariant above, deterministic handler order, and a clean
per-handler budget — not because the VM prohibits it.

## Failure policy

`init.lua` raising aborts the whole load; a *plugin* raising is reported and
the others still load. The asymmetry is deliberate: carrying on with a
half-read user config silently applies settings nobody asked for, while
dropping one plugin is better than dropping the session.

Neither is fatal to startup. Assignments land on a staged `Config` adopted only
if the chunk reaches its end, so there is no path on which a partly-applied
config reaches the compositor — "0xin always starts" became a structural
property rather than a discipline. The message is kept for
`0xinctl config-error`, because a session that silently reverted to defaults
is otherwise indistinguishable from one that loaded fine.

## Sandbox

`Lua::core()` plus `package`, not `Lua::full()`. Three entries in the fuller
set are disqualifying for a compositor: `os.exit` calls `std::process::exit`
directly, killing the session and skipping client teardown; `os.execute` blocks
the whole compositor for the child's lifetime *and* bypasses the spawn path
that resets child signals; `debug.setlocal` lets a script rewrite its caller's
variables. `dofile`, `loadfile` and `load` are removed as well — loading chunks
is the host's job, with a name and a budget. `core()` ships no `print` either,
so 0xin installs one that logs to stderr in the compositor's own style.

## Plugins

neovim's model, reduced: an ordered runtimepath rather than one directory,
`plugin/` auto-run and `lua/` require-only, `after/` last, alphabetical within
a directory, packages as plain directories under `pack/*/start/*`. Deviating
buys nothing and costs everyone the transfer — most people arriving here have
already learned it. `--noplugin` exists because the first question when a
compositor misbehaves is "is it me or a plugin?", and a tool with no way to
start without them makes that unanswerable.

## Status

**Done.** The `.conf` parser, its tests, `0xin.conf.example` and the checked-in
profile config are gone; `init.lua.example` and the fp5 profile's `init.lua`
replace them. Still open: the control socket's wire is still line-based and
one-request-per-connection, so exposing a named subset of the Lua API to
sibling tools is a separate piece of work.
