# Stage 8 — Protocol Compat

**What it is.** The protocols that make the everyday app ecosystem run:
bars and wallpaper, decoration control, screenshots, fullscreen, and
eventually X11 apps. Originally a broader "polish" catch-all, this stage
was narrowed once the daily-driver work outgrew it — features like floating
windows and layout rework now have their own stages
([9](stage-9-floating.md)–[11](stage-11-runtime-control.md)) with their own
gates, keeping the one-stage-one-deliverable rule honest.

**Deliverable** (from `KICKOFF.md`): *the everyday app ecosystem runs —
bars, screenshots, fullscreen video, X11 apps.*

## How it's gone so far

Several of these landed early, driven by real need rather than roadmap
order — a bar and a wallpaper are hard to live without day-to-day, so
layer-shell support arrived well before this stage was "next":

- **`wlr-layer-shell-unstable-v1`** — bars, panels, and wallpaper (e.g.
  [quickshell](https://quickshell.org)) render in the correct z-order (into
  the layer trees set up back in [Stage 2](stage-2-outputs-render.md)) and
  reserve their exclusive screen space, so tiled windows never sit
  underneath them. One real bug here: layer surfaces that arrive before any
  output exists yet were being silently dropped; the fix tracks them as
  pending and attaches them to the next output that shows up, instead of
  requiring output-then-surface ordering.
- **Server-side decorations** (`xdg-decoration-unstable-v1`) — 0xin always
  claims decoration mode, so clients skip drawing their own title bar/CSD:
  bare, borderless windows by default.
- **Screenshots/screen recording** (`wlr-screencopy-unstable-v1` +
  `xdg-output`) — tools like `grim` and `wf-recorder` capture 0xin's real
  composited output directly. `xdg-output` specifically exists because
  screenshot tools need to learn each output's logical position/size, or
  `grim` fails with a 0×0 capture.
- **Fullscreen** — both client-requested (F11, `mpv --fs`, honored on map
  for apps launched fullscreen) and compositor-driven (`Mod+F` toggle). A
  fullscreen window covers its output's full box in a dedicated scene layer
  above the bars but below overlay surfaces, and other windows stay tiled
  beneath it. Per the xdg-shell protocol every state request must be
  answered with a configure even when denied — 0xin previously wasn't
  listening at all, which was a protocol violation, not just a missing
  feature. Closely related fix from the same work: windows are declared
  **tiled** in their very first configure, carrying their predicted tile
  size — without a tiled state the configure size is only a floating-style
  hint, and clients with a remembered window size (browsers especially)
  would map at their own size and overflow across outputs.

**Not yet started:** XWayland (X11 app compatibility) — the one remaining
gate for this stage.

## Status

**Substantially done.** Layer-shell, decorations, screencopy, and
fullscreen are all in daily use; XWayland closes it out.
