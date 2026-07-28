# Stage 2 — Outputs & Render Loop

**What it is.** Moving from "clear the screen once" (Stage 0) to a real,
continuous render loop driven by wlroots' **scene graph** — the data
structure that holds everything that gets drawn and knows how to repaint
only what changed (damage tracking).

**Why it matters.** Every window, background, and layer-shell surface added
in later stages is a node in this scene graph. Getting the output/scene/
layout wiring right here is what lets later stages just add nodes instead of
hand-rolling their own render passes.

**Deliverable** (from `KICKOFF.md`): *`wlr_output`, `wlr_scene`, per-frame
render with damage. A stable, damage-tracked frame on nested + headless.*

## How it went

Three pieces get created once in `main()` and tied together:
`wlr_scene_create` (the scene graph itself), `wlr_output_layout_create` (where
outputs sit in space), and `wlr_scene_attach_output_layout`, which keeps a
scene-output positioned to match its layout slot automatically. Each new
output (handled in `handle_new_output`, see [`src/output.rs`
](https://github.com/termworks/0xin/blob/main/src/output.rs)) gets a
`wlr_scene_output` tied to a layout slot via
`wlr_scene_output_layout_add_output`, and a frame listener that calls
`wlr_scene_output_render` on every frame the output signals it's ready for
one.

The scene is organized as an **ordered stack of layer trees** — direct
children of the scene root, created in a fixed order so creation order
becomes paint order (background → layer-shell bottom → normal app windows →
layer-shell top → layer-shell overlay). That ordering, decided here, is what
[Stage 8](stage-8-polish.md)'s layer-shell support slots into later without
needing to touch the scene wiring again.

**Status: done**, and the scene/output/layout structure from this stage is
unchanged in shape today — later stages extended it (per-output tiling in
Stage 5, forced repaint-on-resume in Stage 6) rather than replacing it.
