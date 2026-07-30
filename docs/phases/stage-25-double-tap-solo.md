# Stage 25 — Double-tap to Solo a Window

**What it is.** An opt-in touchscreen trigger: double-tapping a tiled window
temporarily makes it the only visible window on its workspace — everything
else hides, it fills the usable area — and double-tapping it again restores
the exact prior tiled layout.

**Gate:** *With several tiled windows on a workspace, double-tapping one
solos it (others hidden, it fills the usable area, bars stay visible);
double-tapping it again restores every window to precisely the layout —
including any manually-resized splits — it had before.*

## Configuration

```ini
gesture = double-tap, solo
```

`solo`/`togglesolo` is also a plain shared action, usable from an ordinary
keyboard bind (e.g. `bind = MOD, S, solo`) — it toggles the focused window,
the same way `fullscreen` and `togglefloating` do.

## Why not the existing fullscreen mechanism

`Mod+F` fullscreen already covers other tiled windows visually (its scene
node paints above them), but it isn't a fit for a quick "peek" gesture:

- It tells the **client** it's protocol-fullscreen
  (`wlr_xdg_toplevel_set_fullscreen`) — apps react to that (video players
  change behavior, browsers exit on Escape). Double-tap shouldn't lie about
  fullscreen state for what's meant to be a transient view.
- It removes the window's leaf from the split tree and reinserts it on
  toggle-off, which resets the *sibling* split's ratio to 0.5 each round
  trip — repeatedly peeking and restoring the same window would silently
  erode a manually-tuned layout.

Solo instead never touches the split tree at all — it's a per-workspace
`solo: Option<*mut Toplevel>` that purely decides which window's scene node
is enabled and where the solo target is placed (the usable area, not the
full output box — this isn't true fullscreen, panels/bars stay visible).
Toggling off requires no explicit restore: the very next tiling pass simply
recomputes every rect from the untouched split tree.

## Touch recognition

No tap is swallowed to disambiguate the pair — both taps of a recognized
double-tap are still delivered to the client normally, exactly like any
ordinary tap. Holding back the first tap's delivery until a second tap
confirms it (or a timeout elapses) would add a forced delivery delay to
*every* single tap on *every* window, a well-known cost of naive
double-tap disambiguators. An app with its own double-tap handling (a map's
zoom, a video's seek) reacts to both taps too — an accepted tradeoff.

A completed touch-up is classified as a tap when its total travel stays
under a small pixel threshold; two same-surface taps within a short time and
position window count as a pair. Session lock disables the trigger
automatically, the same way it disables every other gesture.

## Multi-monitor note

The trigger resolves the tapped window directly from the hit-tested surface,
not through the shared active-workspace lookup other gesture actions use —
that lookup depends on the pointer cursor's last position, which touch
events never update, so on a multi-monitor setup without a separate mouse it
could otherwise target the wrong output's focused window.

## Scope

Keyboard focus-cycling (`focusnext`/`focusprev`/`movefocus`) has no
solo-awareness yet — cycling focus while a window is solo'd can move
keyboard focus to a hidden window. Not addressed in this stage; a guard
against that is a natural follow-up if it proves surprising in practice.
