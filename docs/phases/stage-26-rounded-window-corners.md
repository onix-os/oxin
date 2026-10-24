# Stage 26 — Rounded Window Corners

**What it is.** An opt-in `corner_radius` config value that rounds tiled and
floating application windows' corners with real per-pixel masking — not an
illusion. Originally root-surface-only; extended to cover a window's
subsurfaces too (see "Whole-window masking" below) once it became clear real
apps — Firefox among them — paint their visible content into a subsurface
rather than their root surface, and never got rounded at all under the first
cut.

**Gate:** *`corner_radius = N` in config rounds every tiled/floating
window's corners — root surface and any subsurfaces alike — with a clean,
anti-aliased edge that's correct regardless of what's behind it (wallpaper
image, another window, `window_opacity < 1.0`); popups stay unrounded (they
can legitimately extend past the window); fullscreen windows are unaffected;
`corner_radius = 0` (the default) costs nothing.*

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
- `oxide_toplevel_apply_corner_mask` runs from `src/toplevel.rs`'s
  `apply_decorations` — the same per-commit hook that already reapplies
  `window_opacity` — gated on `corner_radius > 0` and the window not being
  fullscreen. Every piece of GL state each pass touches (program, texture
  bindings, viewport, blend state, vertex attrib arrays) is saved and
  restored around it, since `wlr_scene_output_commit` renders the rest of
  that output's scene moments later the same frame — leftover state there
  would corrupt other windows' rendering, not just this one.

## Whole-window masking (root surface + subsurfaces)

The first cut of this feature masked only the toplevel's own root-surface
scene buffer. That's wrong for any client whose actual visible content is
painted into a `wl_subsurface` instead — Firefox is the motivating case —
which left such windows permanently square: `find_root_scene_buffer` would
simply never find a match and bail with a log line saying so.

Rounding the whole window needed two more pieces oxin didn't have before:
Hyprland can round every client because it owns its entire render pipeline
and clips per draw call; oxin bolts masking onto wlroots' stock `wlr_scene`
graph, which has no clip/mask primitive, so matching that behavior means
actually flattening a window's content first.

- **Buffer collection.** `collect_mask_source`, driven by the same
  `wlr_scene_node_for_each_buffer` primitive `oxide_scene_tree_set_opacity`
  already uses generically, walks every buffer under the toplevel's scene
  tree and keeps the root surface and any subsurfaces, in the painter's-
  order the iterator already visits them in. Popups are explicitly skipped
  (`wlr_xdg_surface_try_from_wlr_surface(surface)->role ==
  WLR_XDG_SURFACE_ROLE_POPUP`) — they are structurally distinct xdg_surfaces
  that can legitimately extend past the window, and must never be masked or
  flattened in. Each source's window-local position comes from the
  iterator's layout-coordinate callback minus the toplevel's own origin
  (`wlr_scene_node_coords`); the union of all sources' extents is the
  window's bounding box, clamped to 4x the root surface's own logical size
  (and the whole pass abandoned, falling back to "leave it unmasked for this
  commit") as a guard against a stray/misbehaving subsurface inflating it
  arbitrarily.
- **Pass A — flatten.** `render_composite_pass` draws every collected source
  into one new/reused intermediate swapchain (`Toplevel.composite_swapchain`)
  sized to the bounding box, back-to-front, through a small dedicated "blit"
  shader pair (same `sampler2D`/`samplerExternalOES` split as the mask
  shader, for the same reason) that scales each source by its own
  `wlr_scene_buffer.opacity` while keeping the output premultiplied.
  Standard premultiplied "over" blending (`GL_ONE, GL_ONE_MINUS_SRC_ALPHA`)
  onto a transparent-cleared canvas.
- **Pass B — mask.** The flattened result is re-imported as a texture
  (`wlr_texture_from_buffer`) and run through the *same, unmodified*
  rounded-rect shader the single-surface case always used
  (`render_corner_mask`), into `Toplevel.corner_swapchain` — because that
  logic already re-derives which sampler variant a texture needs from its
  own GL attribs, it works correctly regardless of what target the
  intermediate composite happens to import as.
- **Scene graph.** The masked result is swapped into the *root surface's
  own* scene buffer node (repositioned if the bounding box's origin isn't
  the root's own `(0,0)`), and every other collected node is disabled
  (`wlr_scene_node_set_enabled(..., false)`) — its content is now baked into
  the composite. `oxide_toplevel_apply_corner_mask` always calls
  `oxide_toplevel_restore_corner_mask` first, unconditionally, before
  attempting any of the above — so a transient failure anywhere in the
  pipeline (texture not ready, swapchain acquire failure, composite import
  failure, …) always leaves the window back in a fully-visible, square-
  cornered state for that commit rather than partially hidden.
  `apply_decorations` calls that same restore function on the Rust side,
  gated on a new `Toplevel.was_masked` flag, whenever `corner_radius` turns
  off (live reload) or the window enters fullscreen while previously masked
  — so turning rounding off no longer leaves a stale masked buffer on
  screen, and a window that was never masked costs nothing to check.
- **Known, accepted gap:** a *desynchronized* subsurface (common for a
  video/GL overlay) can commit independently of the root surface. Nothing
  listens for that commit — `wlr_scene_node_for_each_buffer` always re-walks
  the *live* tree on demand, so no bespoke per-subsurface tracking was added
  — so such a subsurface's content can go one frame stale until the root
  surface's own next commit. A natural, wider extension of the damage-
  tracking regression already accepted below, not a new class of bug.
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
  long as `corner_radius > 0` — now for the *whole* window (root +
  subsurfaces), not just the root surface. Accepted.
- **Real, non-hypothetical battery/perf cost, now larger.** What was one
  extra GPU pass per masked commit is now up to *N* source draws (Pass A)
  plus two full buffer passes (Pass A submit, Pass B submit) — roughly
  double the cost even for a window with no subsurfaces (the extra copy
  through the intermediate buffer), scaling further with how many
  subsurfaces a window has. Directly relevant to the FP5 profile, which
  runs `corner_radius = 40` — re-measure battery/frame-time impact there
  specifically; this was not possible in the environment this change was
  written in (no real hardware, no display at all — see Verification).
- **Scanout loss.** A masked window can never be handed straight to a KMS
  plane for direct scanout — the scene graph sees a compositor-rendered
  copy, not the client's original buffer, for as long as it's masked.
- **A source whose texture import GLES2 can't handle** (e.g. an unusual
  multi-plane format) is skipped — logged, drawn as nothing in the
  composite — rather than aborting the whole window's masking.
- **Popups** are the one thing still deliberately excluded by design — they
  can legitimately extend past the window's bounds (menus, tooltips) and
  must never be masked or flattened in. Layer-shell surfaces (bars/panels),
  the wallpaper, and session-lock surfaces remain unaffected — none of them
  route through `toplevel.rs::apply_decorations`.

## Verification

Root-only masking was verified nested (Intel Iris Xe, unscaled):
`corner_radius = 24` with a `kitty` test window — screenshotted via `grim`,
all four corners show clean, correctly anti-aliased rounding with no
premultiplied-alpha fringing, holding up across repeated commits (not just
the first frame). No GL state corruption observed in neighboring host
windows sharing the same frame. Window-destroy cleanup verified: killing a
masked window produces no wlroots assertions or buffer-lock leak warnings,
and the compositor process survives.

**Whole-window masking (root + subsurfaces) is not yet verified against a
real compositor session at all.** It was written and built (`cargo build`,
`cargo test` — both clean, 73 tests passing) in an environment with no
display and no wlroots session available, so none of the following has
actually been run yet and all of it should happen before trusting this on
the FP5 profile:

- A plain terminal (kitty, one source, no subsurfaces) should look
  identical to the already-verified root-only case — the regression check
  that the two-pass path didn't change anything for the common case.
- A subsurface-rendering client (Firefox, or `mpv --vo=gpu` with a
  subsurface-backed video overlay) should now show rounded corners on its
  actual visible content — screenshot via `grim` to confirm clean corners
  with no black/transparent artifacts at the composite's edges, and no
  upside-down or offset content (the Pass A quad placement math in
  `compute_quad` is derived from, but not re-verified against, the already-
  proven root-only orientation).
- Toggling `corner_radius` to `0` via `0xinctl reload` while such a window
  is open should snap it back to square *and fully visible* — confirming
  `oxide_toplevel_restore_corner_mask` actually un-hides every subsurface
  node it hid.
- Resizing, fullscreening, and unfullscreening such a window should not
  leave any node stuck hidden or any stale masked buffer on screen.

**Still not yet verified on real hardware (FP5/Adreno)** even for the
root-only path, let alone the whole-window one — GLES2/EGL behavior (shader
precision qualifiers, extension availability, swapchain format/modifier
compatibility) can differ from the nested Intel path. Recommended before
re-enabling `corner_radius` on the FP5 profile.
