# Stage 26 — Rounded Window Corners

**What it is.** An opt-in `corner_radius` config value that rounds tiled and
floating application windows' corners with real per-pixel masking — not an
illusion.

**Gate:** *`corner_radius = N` in config rounds every tiled/floating
window's corners with a clean, anti-aliased edge that's correct regardless
of what's behind it (wallpaper image, another window, `window_opacity <
1.0`); fullscreen windows are unaffected; `corner_radius = 0` (the default)
costs nothing.*

## Why this needed real rendering work

wlroots' scene-graph/render-pass API has no corner-radius or arbitrary-mask
primitive — confirmed by reading `wlr/types/wlr_scene.h` and
`wlr/render/pass.h` directly. `wlr_render_pass_add_texture`/`add_rect` are a
closed, two-op vtable: rectangular clip only, a flat alpha scalar, no shader
hook. (`window_opacity` was easy because `wlr_scene_buffer` has a plain
`opacity` field; there's no equivalent for radius.) A cheaper illusion
(painting background-colored cutouts over each corner) was considered and
rejected — it only looks right over a flat solid background and breaks
under a wallpaper image or `window_opacity < 1.0`, both already used by the
FP5 profile.

The only real path: pull the client's committed texture's raw GL handle
(`wlr_gles2_texture_get_attribs`), render it through a compositor-owned
custom GLES2 shader — a rounded-rect signed-distance function that discards/
fades fragments outside the radius — into a compositor-owned GPU buffer
(`wlr_swapchain_create`/`wlr_renderer_begin_buffer_pass`), then swap that
buffer into the scene graph in place of the client's own
(`wlr_scene_buffer_set_buffer`). This is a genuinely bigger piece of
engineering than any other 0xin feature so far — 0xin had never driven raw
GL directly before this; every prior frame was 100% delegated to
`wlr_scene_output_commit`.

## How it's wired

- `shim/gles2_corner.c` compiles two shader variants once at startup
  (`oxide_gles2_corner_program_create`, called from `src/main.rs` right
  after the renderer/allocator are created) — one for `GL_TEXTURE_2D`, one
  for `GL_TEXTURE_EXTERNAL_OES`/`samplerExternalOES`, since
  `wlr_gles2_texture_get_attribs` can report either depending on the
  client's buffer import path, and using the wrong sampler type for the
  bound texture renders solid black, not a GL error. NULL on any
  compile/link failure — corner-radius masking is then silently
  unavailable rather than crashing the compositor.
- `oxide_toplevel_apply_corner_radius` runs from `src/toplevel.rs`'s
  `handle_commit` — the same per-commit hook that already reapplies
  `window_opacity` — gated on `corner_radius > 0` and the window not being
  fullscreen. It finds the toplevel's own root-surface scene buffer (never
  a popup/subsurface — both are parented under the same scene tree by
  `wlr_scene_xdg_surface_create`, and must stay unmasked), renders through
  the shader into a per-toplevel swapchain (`Toplevel.corner_swapchain`,
  recreated when the surface's buffer size changes), and swaps the result
  in. Every piece of GL state the draw touches (program, texture bindings,
  viewport, blend state, vertex attrib arrays) is saved and restored around
  it, since `wlr_scene_output_commit` renders the rest of that output's
  scene moments later the same frame — leftover state there would corrupt
  other windows' rendering, not just this one.
- The per-toplevel swapchain is freed on window destroy
  (`oxide_swapchain_destroy`, from `handle_destroy`) — the shared
  `corner_program`, like the renderer/allocator it's built from, lives for
  the process's lifetime (0xin deliberately skips tearing down top-level
  wlroots globals on shutdown; see `main.rs`).
- Fullscreen windows are excluded entirely — rounding a window's edges
  against the bare screen looks wrong, and it's also a real performance win
  (fullscreen is exactly the "video playing, committing every frame" worst
  case for the extra GPU pass this costs).

## Known limitations (by design, for this first cut)

- **`corner_radius` is currently in the surface's buffer-pixel units, not
  logical pixels adjusted for output scale.** The FP5 profile's `monitor =
  DSI-1, 0x0, 2.4` (2.4× scale) means a given `corner_radius` value will
  look visually smaller there than the same value on an unscaled desktop
  output. Verified correct on the nested (unscaled) Intel path; the
  logical-vs-physical adjustment for fractional-scale outputs is an open
  follow-up, not yet resolved.
- **Damage tracking regresses for masked windows.** Every masked commit
  repaints the whole buffer (`wlr_scene_buffer_set_buffer`'s plain
  variant), losing wlroots' fine-grained per-region damage tracking for as
  long as `corner_radius > 0`. Accepted for this first cut.
- **Real, non-hypothetical battery/perf cost.** An extra GPU pass on every
  commit of every visible masked window — the same device (FP5) this
  project added DPMS power-off support *to save battery on*. No
  throttling/coalescing is implemented.
- **Scanout loss.** A masked window can never be handed straight to a KMS
  plane for direct scanout — the scene graph sees a compositor-rendered
  copy, not the client's original buffer, for as long as it's masked.
- Popups/subsurfaces, layer-shell surfaces (bars/panels), the wallpaper,
  and session-lock surfaces are all unaffected — none of them route
  through `toplevel.rs::handle_commit`.

## Verification

Verified nested (Intel Iris Xe, unscaled): `corner_radius = 24` with a
`kitty` test window — screenshotted via `grim`, all four corners show
clean, correctly anti-aliased rounding with no premultiplied-alpha fringing,
holding up across repeated commits (not just the first frame). No GL state
corruption observed in neighboring host windows sharing the same frame.
Window-destroy cleanup verified: killing a masked window produces no
wlroots assertions or buffer-lock leak warnings, and the compositor process
survives.

**Not yet verified on real hardware (FP5/Adreno).** GLES2/EGL behavior —
shader precision qualifiers, extension availability, swapchain format/
modifier compatibility — can differ from the nested Intel path; this is
exactly the class of bug a nested-only test can miss. Recommended before
enabling `corner_radius` on the FP5 profile.
