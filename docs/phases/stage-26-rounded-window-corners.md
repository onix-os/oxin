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
  bound texture renders solid black, not a GL error. The two are built
  **independently and either one is enough**: a driver missing
  `GL_OES_EGL_image_external` costs rounding only on clients that actually
  hand over external textures, and the apply path leaves a window unmasked
  when the variant it would need is absent. NULL on any compile/link
  failure — corner-radius masking is then unavailable rather than crashing
  the compositor.

  **This needs the GLES2 renderer, and checks first.** wlroots chooses a
  renderer on its own, and on a Vulkan or pixman session every entry point
  used here is unavailable: `wlr_gles2_renderer_get_egl` asserts that its
  argument really is the GLES2 renderer, so calling it aborts the whole
  compositor rather than returning an error. The constructor therefore
  starts with `wlr_renderer_is_gles2` and, when that is false, logs one line
  and returns NULL so `corner_radius` is a no-op instead of a crash.
  `WLR_RENDERER=gles2` forces the renderer if a machine picks another. This
  was a real crash, not a precaution — reproduced with
  `WLR_RENDERER=pixman`, which is also the regression test.
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
- **The mask is only half the job: the scene has to be told the corners are
  no longer opaque.** `wlr_scene_buffer_set_opaque_region` is what wlroots
  uses to decide whether anything *underneath* a buffer needs drawing at
  all, and the region it holds is the client's — which for most toplevels
  covers the whole window. Swap in a masked buffer without correcting it and
  wlroots skips painting the wallpaper beneath the corners, so the
  transparent pixels the shader just cut reveal an unpainted framebuffer:
  corners that render solid black. The shader is not at fault there and
  neither is blending — nothing was ever drawn to blend with. So the apply
  path takes the surface's own opaque region, subtracts the four corner
  boxes, and sets that alongside `set_dest_size` (the same pair, in the same
  order, that wlroots' own `surface_reconfigure` sets). Starting from the
  client's region rather than a full rect matters: a terminal with
  transparency declares nothing opaque, and claiming otherwise would drop the
  background behind it. Subtracting only the corners keeps the occlusion
  optimisation for the body of the window.
- **The radius is converted from logical to buffer pixels.** `corner_radius`
  is configured in logical pixels, but the shader compares it against
  `u_size`, which is the buffer's physical size. Those are the same unit only
  at scale 1; at scale 3 a configured 40 was being cut as 40 *buffer* pixels,
  about 13 logical, so the rounding silently shrank in proportion to the
  display's scale. The apply path now scales by the surface's own
  buffer-to-logical ratio.
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

- ~~**`corner_radius` is in the surface's buffer-pixel units, not logical
  pixels adjusted for output scale.**~~ **Resolved.** The radius was being
  compared against `u_size` in buffer pixels while arriving in logical ones,
  so a scaled output shrank the rounding in proportion to its scale — on the
  FP5 profile (`scale = 3`) a configured 40 was cut as roughly 13 logical
  pixels. The apply path now multiplies by the surface's own
  buffer-to-logical ratio. Measured nested at scale 1 and scale 2: the cut
  spans 21.0 and 21.5 logical pixels respectively, against an ideal arc of
  22.6 (the shortfall is the anti-aliasing band).
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
