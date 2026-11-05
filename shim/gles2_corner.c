#define WLR_USE_UNSTABLE
#include <drm_fourcc.h>
#include <EGL/egl.h>
#include <GLES2/gl2.h>
#include <GLES2/gl2ext.h>
#include <pixman.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <wlr/render/allocator.h>
#include <wlr/render/drm_format_set.h>
#include <wlr/render/egl.h>
#include <wlr/render/gles2.h>
#include <wlr/render/pass.h>
#include <wlr/render/swapchain.h>
#include <wlr/render/wlr_renderer.h>
#include <wlr/render/wlr_texture.h>
#include <wlr/types/wlr_compositor.h>
#include <wlr/types/wlr_scene.h>
#include <wlr/types/wlr_xdg_shell.h>
#include <wlr/util/log.h>

#include "oxide_shim.h"

// --- rounded-corner GLES2 masking --------------------------------------------
//
// wlroots' scene-graph/render-pass API has no corner-radius or arbitrary-mask
// primitive (checked: wlr_render_pass_add_texture/add_rect only support a
// rectangular clip and a flat alpha scalar). Real per-pixel rounding needs a
// compositor-owned GLES2 program that masks a window's texture into a
// compositor-owned buffer, which src/toplevel.rs's handle_commit then swaps
// into the scene graph in place of the client's own buffer.
//
// Two texture-target variants are compiled because wlr_gles2_texture_get_attribs
// can report either GL_TEXTURE_2D or GL_TEXTURE_EXTERNAL_OES depending on the
// client buffer's import path — using the wrong sampler type for the bound
// texture target renders solid black, not a GL error, so both must exist.

static const char *VERTEX_SRC =
    "attribute vec2 pos;\n"
    "attribute vec2 texcoord;\n"
    "varying vec2 v_texcoord;\n"
    "void main() {\n"
    "    gl_Position = vec4(pos, 0.0, 1.0);\n"
    "    v_texcoord = texcoord;\n"
    "}\n";

// Rounded-rect signed-distance function (Inigo Quilez's formula), evaluated
// in pixel space so u_radius is a pixel count regardless of surface size.
// The SDF is tested against the whole toplevel's own box (u_tile_size),
// not this buffer's own size (u_size) — a window is masked buffer-by-buffer
// (root surface, plus any subsurfaces it has), and every one of them must
// agree on where the toplevel's four corners actually are. u_offset is this
// buffer's own position within that shared box, so a subsurface sitting away
// from every corner comes out with coverage 1.0 everywhere (untouched), and
// one that does overlap a corner gets cut at the same edge the root would
// be. For the root surface specifically, u_offset is always (0,0) and
// u_tile_size == u_size, which collapses this back to exactly the original,
// single-surface formula — a plain window must render pixel-identical to
// before this buffer-by-buffer generalization.
static const char *FRAGMENT_SRC_TEMPLATE =
    "%s"
    "precision mediump float;\n"
    "varying vec2 v_texcoord;\n"
    "uniform %s u_tex;\n"
    "uniform vec2 u_size;\n"
    "uniform vec2 u_tile_size;\n"
    "uniform vec2 u_offset;\n"
    "uniform float u_radius;\n"
    // Textures without a real alpha channel (has_alpha == false — an
    // opaque XRGB-style buffer) can return driver-dependent garbage when
    // their alpha component is sampled, rather than a clean 1.0 — observed
    // in practice producing fully transparent or wrongly-darkened windows
    // on one GPU/driver while looking correct on another. u_has_alpha lets
    // the caller force alpha to 1.0 for such textures instead of trusting
    // whatever the sampler returns.
    "uniform float u_has_alpha;\n"
    "float rounded_box_sdf(vec2 p, vec2 half_size, float radius) {\n"
    "    vec2 q = abs(p) - half_size + radius;\n"
    "    return length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - radius;\n"
    "}\n"
    "void main() {\n"
    "    vec4 color = texture2D(u_tex, v_texcoord);\n"
    "    float src_alpha = mix(1.0, color.a, u_has_alpha);\n"
    "    vec2 pixel_pos = v_texcoord * u_size + u_offset - u_tile_size * 0.5;\n"
    "    float dist = rounded_box_sdf(pixel_pos, u_tile_size * 0.5, u_radius);\n"
    "    float coverage = 1.0 - smoothstep(-1.0, 1.0, dist);\n"
    "    gl_FragColor = vec4(color.rgb * coverage, src_alpha * coverage);\n"
    "}\n";

struct oxide_corner_variant {
    GLuint program;
    GLint attrib_pos;
    GLint attrib_texcoord;
    GLint uniform_tex;
    GLint uniform_size;
    GLint uniform_tile_size;
    GLint uniform_offset;
    GLint uniform_radius;
    GLint uniform_has_alpha;
};

struct oxide_corner_program {
    struct oxide_corner_variant tex2d;
    struct oxide_corner_variant tex_oes;
    GLuint quad_vbo;
};

static GLuint compile_shader(GLenum type, const char *src) {
    GLuint shader = glCreateShader(type);
    glShaderSource(shader, 1, &src, NULL);
    glCompileShader(shader);
    GLint ok = GL_FALSE;
    glGetShaderiv(shader, GL_COMPILE_STATUS, &ok);
    if (!ok) {
        char log[512];
        glGetShaderInfoLog(shader, sizeof(log), NULL, log);
        wlr_log(WLR_ERROR, "0xin: corner-radius shader compile failed: %s", log);
        glDeleteShader(shader);
        return 0;
    }
    return shader;
}

// Links a vertex + fragment shader into a program, caches attribute/uniform
// locations, and cleans up the (no-longer-needed-after-link) shader objects.
// Returns false (leaving `out` zeroed) on any failure.
static bool build_variant(struct oxide_corner_variant *out, GLuint vertex,
        const char *sampler_type, const char *extension_directive) {
    memset(out, 0, sizeof(*out));

    char fragment_src[2048];
    snprintf(fragment_src, sizeof(fragment_src), FRAGMENT_SRC_TEMPLATE,
            extension_directive, sampler_type);
    GLuint fragment = compile_shader(GL_FRAGMENT_SHADER, fragment_src);
    if (fragment == 0) {
        return false;
    }

    GLuint program = glCreateProgram();
    glAttachShader(program, vertex);
    glAttachShader(program, fragment);
    glLinkProgram(program);
    glDeleteShader(fragment); // vertex is shared across variants; caller keeps it

    GLint ok = GL_FALSE;
    glGetProgramiv(program, GL_LINK_STATUS, &ok);
    if (!ok) {
        char log[512];
        glGetProgramInfoLog(program, sizeof(log), NULL, log);
        wlr_log(WLR_ERROR, "0xin: corner-radius program link failed: %s", log);
        glDeleteProgram(program);
        return false;
    }

    out->program = program;
    out->attrib_pos = glGetAttribLocation(program, "pos");
    out->attrib_texcoord = glGetAttribLocation(program, "texcoord");
    out->uniform_tex = glGetUniformLocation(program, "u_tex");
    out->uniform_size = glGetUniformLocation(program, "u_size");
    out->uniform_tile_size = glGetUniformLocation(program, "u_tile_size");
    out->uniform_offset = glGetUniformLocation(program, "u_offset");
    out->uniform_radius = glGetUniformLocation(program, "u_radius");
    out->uniform_has_alpha = glGetUniformLocation(program, "u_has_alpha");
    return true;
}

// A unit quad in NDC (position) paired with 0..1 texture coordinates,
// interleaved per-vertex: [x, y, u, v] * 4, drawn as a triangle strip.
static const GLfloat QUAD_VERTICES[] = {
    -1.0f, -1.0f, 0.0f, 0.0f,
     1.0f, -1.0f, 1.0f, 0.0f,
    -1.0f,  1.0f, 0.0f, 1.0f,
     1.0f,  1.0f, 1.0f, 1.0f,
};

void *oxide_gles2_corner_program_create(struct wlr_renderer *renderer) {
    // Every GL call below reaches the renderer through wlroots' GLES2 API, and
    // `wlr_gles2_renderer_get_egl` asserts its argument really is that renderer
    // — so on a Vulkan or pixman session, calling it *aborts the compositor*
    // before this function can report anything. Ask first. wlroots picks the
    // renderer itself, so a machine that lands on another one is not a
    // misconfiguration; it just cannot have this feature.
    if (!wlr_renderer_is_gles2(renderer)) {
        wlr_log(WLR_INFO, "0xin: corner-radius masking needs the GLES2 renderer and "
                "this session is using another one (WLR_RENDERER=gles2 forces it) — "
                "corner_radius will have no effect");
        return NULL;
    }

    struct wlr_egl *egl = wlr_gles2_renderer_get_egl(renderer);
    EGLDisplay display = wlr_egl_get_display(egl);
    EGLContext context = wlr_egl_get_context(egl);
    EGLContext previous = eglGetCurrentContext();
    if (!eglMakeCurrent(display, EGL_NO_SURFACE, EGL_NO_SURFACE, context)) {
        wlr_log(WLR_ERROR, "0xin: eglMakeCurrent failed for corner-radius setup");
        return NULL;
    }

    struct oxide_corner_program *p = calloc(1, sizeof(*p));
    GLuint vertex = compile_shader(GL_VERTEX_SHADER, VERTEX_SRC);

    // The two variants are built independently, and one is enough. Which one a
    // window needs depends on how its client's buffer was imported, so a driver
    // without GL_OES_EGL_image_external should cost rounding only on the clients
    // that actually hand over external textures — not on every window on the
    // screen, which is what requiring both variants used to do.
    bool have_2d = vertex != 0
            && build_variant(&p->tex2d, vertex, "sampler2D", "");
    bool have_oes = vertex != 0
            && build_variant(&p->tex_oes, vertex, "samplerExternalOES",
                    "#extension GL_OES_EGL_image_external : enable\n");
    bool ok = have_2d || have_oes;
    if (vertex != 0) {
        glDeleteShader(vertex);
    }

    if (ok) {
        glGenBuffers(1, &p->quad_vbo);
        glBindBuffer(GL_ARRAY_BUFFER, p->quad_vbo);
        glBufferData(GL_ARRAY_BUFFER, sizeof(QUAD_VERTICES), QUAD_VERTICES,
                GL_STATIC_DRAW);
        glBindBuffer(GL_ARRAY_BUFFER, 0);
    }

    // Restore whatever was current before — matches the header's stated
    // policy that GLES2 renderer operations save/restore the prior context;
    // our own one-off setup call should leave things as it found them too.
    eglMakeCurrent(display, EGL_NO_SURFACE, EGL_NO_SURFACE, previous);

    if (!ok) {
        if (p->tex2d.program) {
            glDeleteProgram(p->tex2d.program);
        }
        if (p->tex_oes.program) {
            glDeleteProgram(p->tex_oes.program);
        }
        free(p);
        return NULL;
    }
    if (have_2d && have_oes) {
        wlr_log(WLR_INFO, "0xin: corner-radius GLES2 program ready");
    } else {
        // Worth a warning rather than silence: rounding will look inconsistent
        // between windows, and this says which half is missing and why.
        wlr_log(WLR_ERROR, "0xin: corner-radius GLES2 program ready for %s textures "
                "only — windows using the other kind stay square",
                have_2d ? "sampler2D" : "external (OES)");
    }
    return p;
}

// --- per-commit masking pass -------------------------------------------------
//
// A toplevel's scene tree holds its root surface and every wl_subsurface it
// has, each as its own wlr_scene_buffer node (wlr_scene_xdg_surface_create's
// doc comment: it covers the root surface "and all of its sub-surfaces").
// Earlier, masking matched only the one node backed by the root surface,
// leaving any subsurface — e.g. a dropdown/overlay a client renders that way
// — unmasked: square, opaque, painted right over the rounded corners. This
// masks every buffer node found, each independently, each against the same
// shared toplevel-sized rounded-rect test, so a client that happens to
// overlap a corner with a subsurface gets cut there too.
//
// Popups never show up here: they get their own, separate scene tree
// (handle_new_popup, shim/xdg_shell.c), never parented under a toplevel's
// own tree — nothing here needs to special-case or exclude them.
//
// Each masked buffer keeps its own swapchain, tracked by surface identity in
// a small pool owned opaquely by the Rust side (one `void*` on `Toplevel`,
// replacing the single corner_swapchain this used to be). A full sweep
// (driven by the root surface's own commit — same trigger as before) masks
// everything found and prunes any pool entry whose subsurface no longer
// exists. A single-surface pass (driven by one subsurface's own commit,
// wired up separately in src/toplevel.rs since a subsurface can be
// desynchronized and commit independently of the root) re-masks just that
// one node, cheaply, without touching the rest.

struct oxide_corner_mask {
    struct wlr_surface *surface; // identity key
    struct wlr_swapchain *swapchain;
    int w, h; // swapchain's current size, buffer pixels
    bool seen; // scratch flag for the full-sweep prune pass
};

struct oxide_corner_mask_pool {
    struct oxide_corner_mask *masks;
    size_t count;
    size_t cap;
};

static struct oxide_corner_mask *pool_find_or_create(
        struct oxide_corner_mask_pool *pool, struct wlr_surface *surface) {
    for (size_t i = 0; i < pool->count; i++) {
        if (pool->masks[i].surface == surface) {
            return &pool->masks[i];
        }
    }
    if (pool->count == pool->cap) {
        size_t new_cap = pool->cap == 0 ? 4 : pool->cap * 2;
        struct oxide_corner_mask *grown =
                realloc(pool->masks, new_cap * sizeof(*grown));
        if (grown == NULL) {
            return NULL;
        }
        pool->masks = grown;
        pool->cap = new_cap;
    }
    struct oxide_corner_mask *m = &pool->masks[pool->count++];
    memset(m, 0, sizeof(*m));
    m->surface = surface;
    return m;
}

static void pool_forget(struct oxide_corner_mask_pool *pool,
        struct wlr_surface *surface) {
    for (size_t i = 0; i < pool->count; i++) {
        if (pool->masks[i].surface != surface) {
            continue;
        }
        wlr_swapchain_destroy(pool->masks[i].swapchain);
        pool->masks[i] = pool->masks[pool->count - 1];
        pool->count--;
        return;
    }
}

// Drains and logs every pending GL error, tagged with `where` — GL errors
// are otherwise silent, and this masking path has already shown real,
// driver-specific breakage (Adreno vs Mesa/Intel) that produced wrong
// pixels with no crash, so every call site below checks explicitly rather
// than trusting success.
static void log_gl_errors(const char *where) {
    GLenum err;
    while ((err = glGetError()) != GL_NO_ERROR) {
        wlr_log(WLR_ERROR, "0xin: corner-radius GL error 0x%x at %s", err, where);
    }
}

// Mask one buffer node's surface into `mask`'s swapchain and swap the result
// into the scene graph in place of the client's own buffer — the only way to
// get real per-pixel masking, since wlroots' render-pass API has no
// clip/mask/shader hook. `tile_w`/`tile_h`/`offset_x`/`offset_y` are all in
// *logical* units (same space `wlr_scene_node_for_each_buffer` reports
// positions in, and the same space xdg geometry is expressed in); this
// function converts them into this specific surface's own buffer-pixel space
// before handing them to the shader, using that surface's own
// buffer-to-logical ratio — not some single ratio assumed for the whole
// toplevel — so a subsurface that happens to use a different buffer scale
// than the root still gets a correctly-aligned test.
static bool mask_one_buffer(struct wlr_renderer *renderer,
        struct wlr_allocator *allocator, struct oxide_corner_program *prog,
        struct wlr_scene_buffer *buffer, struct wlr_surface *surface,
        int offset_x, int offset_y, int tile_w, int tile_h, int radius,
        struct oxide_corner_mask *mask) {
    struct wlr_texture *texture = wlr_surface_get_texture(surface);
    if (texture == NULL || !wlr_texture_is_gles2(texture)) {
        // No content committed yet, or 0xin is running a non-GLES2 renderer
        // (Vulkan/Pixman) — corner masking is GLES2-only; fail closed rather
        // than assume.
        return false;
    }
    struct wlr_gles2_texture_attribs attribs;
    wlr_gles2_texture_get_attribs(texture, &attribs);

    // Bail before allocating a swapchain if the shader this texture would need
    // never built (see the variant handling in the program constructor). The
    // buffer then keeps its own, unmasked content instead of being masked
    // with the wrong sampler type, which renders solid black rather than
    // failing loudly.
    const struct oxide_corner_variant *needed =
            attribs.target == GL_TEXTURE_EXTERNAL_OES ? &prog->tex_oes : &prog->tex2d;
    if (needed->program == 0) {
        return false;
    }

    int w = surface->current.buffer_width;
    int h = surface->current.buffer_height;
    if (w <= 0 || h <= 0) {
        return false;
    }

    struct wlr_swapchain *swapchain = mask->swapchain;
    if (swapchain == NULL || mask->w != w || mask->h != h) {
        if (swapchain != NULL) {
            wlr_swapchain_destroy(swapchain);
        }
        const struct wlr_drm_format_set *formats =
                wlr_renderer_get_texture_formats(renderer, WLR_BUFFER_CAP_DMABUF);
        const struct wlr_drm_format *format =
                formats != NULL ? wlr_drm_format_set_get(formats, DRM_FORMAT_ARGB8888)
                                 : NULL;
        if (format == NULL) {
            wlr_log(WLR_ERROR,
                    "0xin: no ARGB8888 render format available for corner-radius masking");
            mask->swapchain = NULL;
            return false;
        }
        swapchain = wlr_swapchain_create(allocator, w, h, format);
        if (swapchain == NULL) {
            wlr_log(WLR_ERROR, "0xin: failed to create corner-radius swapchain (%dx%d)",
                    w, h);
            mask->swapchain = NULL;
            return false;
        }
        mask->swapchain = swapchain;
        mask->w = w;
        mask->h = h;
    }

    struct wlr_buffer *mask_buffer = wlr_swapchain_acquire(swapchain);
    if (mask_buffer == NULL) {
        wlr_log(WLR_ERROR, "0xin: corner-radius swapchain acquire failed");
        return false;
    }

    struct wlr_render_pass *pass =
            wlr_renderer_begin_buffer_pass(renderer, mask_buffer, NULL);
    if (pass == NULL) {
        wlr_buffer_unlock(mask_buffer);
        return false;
    }
    GLuint fbo = wlr_gles2_renderer_get_buffer_fbo(renderer, mask_buffer);

    // Save every piece of GL state this draw touches — wlr_scene_output_commit
    // renders the rest of this same output's scene moments later this same
    // frame, so anything left dirty here corrupts other windows' rendering.
    GLint prev_program, prev_array_buffer, prev_active_texture, prev_fbo;
    GLint prev_viewport[4];
    GLboolean prev_blend_enabled = glIsEnabled(GL_BLEND);
    GLint prev_blend_src_rgb, prev_blend_dst_rgb, prev_blend_src_a, prev_blend_dst_a;
    glGetIntegerv(GL_CURRENT_PROGRAM, &prev_program);
    glGetIntegerv(GL_ARRAY_BUFFER_BINDING, &prev_array_buffer);
    glGetIntegerv(GL_ACTIVE_TEXTURE, &prev_active_texture);
    glGetIntegerv(GL_FRAMEBUFFER_BINDING, &prev_fbo);
    glGetIntegerv(GL_VIEWPORT, prev_viewport);
    glGetIntegerv(GL_BLEND_SRC_RGB, &prev_blend_src_rgb);
    glGetIntegerv(GL_BLEND_DST_RGB, &prev_blend_dst_rgb);
    glGetIntegerv(GL_BLEND_SRC_ALPHA, &prev_blend_src_a);
    glGetIntegerv(GL_BLEND_DST_ALPHA, &prev_blend_dst_a);
    glActiveTexture(GL_TEXTURE0);
    GLint prev_tex2d, prev_tex_oes;
    glGetIntegerv(GL_TEXTURE_BINDING_2D, &prev_tex2d);
    glGetIntegerv(GL_TEXTURE_BINDING_EXTERNAL_OES, &prev_tex_oes);

    struct oxide_corner_variant *variant =
            attribs.target == GL_TEXTURE_EXTERNAL_OES ? &prog->tex_oes : &prog->tex2d;

    // wlr_render_pass_add_texture (the path we deliberately bypass — see the
    // function doc comment) accepts a wait_timeline/explicit-sync point
    // before sampling a client's texture; our raw GL sampling below has no
    // equivalent, so on a driver where the client's DMA-BUF import/upload
    // genuinely races our read (observed on real Adreno hardware — not
    // reproduced on the nested Intel path, which likely has different, more
    // forgiving timing), we could sample before the content is actually
    // there and render black/incomplete. Block until all prior GPU work —
    // including whatever wlr_surface_get_texture just triggered — is
    // actually complete before touching that texture. Costs real time; a
    // proper fix would wait on the buffer's specific fence instead of
    // stalling the whole pipeline, but correctness comes first.
    glFinish();

    glBindFramebuffer(GL_FRAMEBUFFER, fbo);
    GLenum fbo_status = glCheckFramebufferStatus(GL_FRAMEBUFFER);
    if (fbo_status != GL_FRAMEBUFFER_COMPLETE) {
        wlr_log(WLR_ERROR,
                "0xin: corner-radius FBO incomplete (0x%x) for %dx%d buffer — "
                "swapchain format/modifier likely unusable as a render target "
                "on this driver",
                fbo_status, w, h);
    }
    glViewport(0, 0, w, h);
    // The shader writes final (already-masked) alpha directly; no destination
    // blending wanted — the mask buffer starts undefined/opaque-black, and we
    // overwrite every texel of it unconditionally.
    glDisable(GL_BLEND);
    glUseProgram(variant->program);
    glActiveTexture(GL_TEXTURE0);
    glBindTexture(attribs.target, attribs.tex);
    // A freshly bound texture's default GL_TEXTURE_MIN_FILTER
    // (GL_NEAREST_MIPMAP_LINEAR) requires a full mipmap chain — client
    // textures are single-level, so without this the texture is
    // "incomplete" and GL silently samples it as opaque black, no error
    // raised. Desktop Mesa/Intel is lenient about this in practice; strict
    // mobile drivers (observed on Adreno) are not — this was invisible on
    // the nested dev path and consistently broken on real hardware. Setting
    // this explicitly, every draw, is cheap insurance regardless of
    // whatever state the client's texture object happened to carry in.
    glTexParameteri(attribs.target, GL_TEXTURE_MIN_FILTER, GL_LINEAR);
    glTexParameteri(attribs.target, GL_TEXTURE_MAG_FILTER, GL_LINEAR);
    glTexParameteri(attribs.target, GL_TEXTURE_WRAP_S, GL_CLAMP_TO_EDGE);
    glTexParameteri(attribs.target, GL_TEXTURE_WRAP_T, GL_CLAMP_TO_EDGE);
    glUniform1i(variant->uniform_tex, 0);
    glUniform2f(variant->uniform_size, (float)w, (float)h);
    // `radius`/`tile_w`/`tile_h`/`offset_x`/`offset_y` all arrive in *logical*
    // pixels, but u_size is this buffer's *physical* size and the shader
    // compares everything in one space. Convert using this surface's own
    // buffer-to-logical ratio, not some ratio assumed for the whole toplevel
    // (the root and a subsurface are not guaranteed to share one) — matches
    // the buffer actually being masked.
    float buffer_scale = surface->current.width > 0
            ? (float)w / (float)surface->current.width
            : 1.0f;
    glUniform2f(variant->uniform_tile_size,
            (float)tile_w * buffer_scale, (float)tile_h * buffer_scale);
    glUniform2f(variant->uniform_offset,
            (float)offset_x * buffer_scale, (float)offset_y * buffer_scale);
    glUniform1f(variant->uniform_radius, (float)radius * buffer_scale);
    glUniform1f(variant->uniform_has_alpha, attribs.has_alpha ? 1.0f : 0.0f);

    glBindBuffer(GL_ARRAY_BUFFER, prog->quad_vbo);
    glEnableVertexAttribArray(variant->attrib_pos);
    glEnableVertexAttribArray(variant->attrib_texcoord);
    glVertexAttribPointer(variant->attrib_pos, 2, GL_FLOAT, GL_FALSE,
            4 * sizeof(GLfloat), (void *)0);
    glVertexAttribPointer(variant->attrib_texcoord, 2, GL_FLOAT, GL_FALSE,
            4 * sizeof(GLfloat), (void *)(2 * sizeof(GLfloat)));
    glDrawArrays(GL_TRIANGLE_STRIP, 0, 4);
    glDisableVertexAttribArray(variant->attrib_pos);
    glDisableVertexAttribArray(variant->attrib_texcoord);
    log_gl_errors("draw");

    glBindTexture(GL_TEXTURE_2D, prev_tex2d);
    glBindTexture(GL_TEXTURE_EXTERNAL_OES, prev_tex_oes);
    glActiveTexture(prev_active_texture);
    glBindBuffer(GL_ARRAY_BUFFER, prev_array_buffer);
    glUseProgram(prev_program);
    glBindFramebuffer(GL_FRAMEBUFFER, prev_fbo);
    glViewport(prev_viewport[0], prev_viewport[1], prev_viewport[2], prev_viewport[3]);
    if (prev_blend_enabled) {
        glEnable(GL_BLEND);
    } else {
        glDisable(GL_BLEND);
    }
    glBlendFuncSeparate(prev_blend_src_rgb, prev_blend_dst_rgb, prev_blend_src_a,
            prev_blend_dst_a);

    if (!wlr_render_pass_submit(pass)) {
        wlr_log(WLR_ERROR, "0xin: corner-radius render pass submit failed");
        wlr_buffer_unlock(mask_buffer);
        return false;
    }

    wlr_scene_buffer_set_buffer(buffer, mask_buffer);

    // Tell the scene which of this buffer's pixels are still opaque. wlroots
    // uses that to decide whether anything *underneath* needs drawing at
    // all, so getting this wrong in the "too permissive" direction (claiming
    // opaque where the shader actually cut to transparent) shows up as wrong
    // pixels, not just a missed optimization — the four corners this surface
    // might lose are wherever its own box overlaps one of the toplevel's
    // *real* four corners (in shared tile space), not necessarily this
    // surface's own four corners: a subsurface positioned away from every
    // real corner loses nothing and should report its opaque region
    // unchanged.
    int logical_w = surface->current.width;
    int logical_h = surface->current.height;
    int corner = radius;
    if (corner > tile_w / 2) {
        corner = tile_w / 2;
    }
    if (corner > tile_h / 2) {
        corner = tile_h / 2;
    }
    pixman_region32_t opaque;
    pixman_region32_init(&opaque);
    pixman_region32_copy(&opaque, &surface->opaque_region);
    pixman_region32_intersect_rect(&opaque, &opaque, 0, 0, logical_w, logical_h);
    if (corner > 0) {
        // The toplevel's four corner squares, in tile-space, translated into
        // this surface's own local coordinates (local (0,0) == tile-space
        // (offset_x, offset_y)) by subtracting the offset.
        pixman_region32_t corners;
        pixman_region32_init(&corners);
        pixman_region32_union_rect(&corners, &corners,
                0 - offset_x, 0 - offset_y, corner, corner);
        pixman_region32_union_rect(&corners, &corners,
                (tile_w - corner) - offset_x, 0 - offset_y, corner, corner);
        pixman_region32_union_rect(&corners, &corners,
                0 - offset_x, (tile_h - corner) - offset_y, corner, corner);
        pixman_region32_union_rect(&corners, &corners,
                (tile_w - corner) - offset_x, (tile_h - corner) - offset_y, corner, corner);
        pixman_region32_subtract(&opaque, &opaque, &corners);
        pixman_region32_fini(&corners);
    }
    wlr_scene_buffer_set_opaque_region(buffer, &opaque);
    pixman_region32_fini(&opaque);

    // Restate this buffer's own logical display size explicitly — our mask
    // buffer is the surface's full physical size, but on a fractionally-
    // scaled output that must still be painted at the surface's own logical
    // size, not 1:1. This is per-surface, not the shared tile size: each
    // buffer keeps displaying at whatever size/position it already had in
    // the scene graph, only its pixels change. Also reset any leftover
    // source crop from the client's own buffer/viewport state, since our
    // mask buffer is a full, uncropped copy — a stale src_box would crop it
    // incorrectly.
    wlr_scene_buffer_set_dest_size(buffer, logical_w, logical_h);
    wlr_scene_buffer_set_source_box(buffer, NULL);
    wlr_buffer_unlock(mask_buffer);
    return true;
}

struct oxide_corner_sweep_ctx {
    struct wlr_renderer *renderer;
    struct wlr_allocator *allocator;
    struct oxide_corner_program *prog;
    struct oxide_corner_mask_pool *pool;
    int tree_lx, tree_ly;
    int tile_w, tile_h;
    int radius;
    // NULL: mask every buffer found (a full sweep, driven by the root's own
    // commit). Non-NULL: mask only the one buffer backed by this surface (a
    // single subsurface's own commit) — see shim/gles2_corner.c's module
    // comment for why that path exists at all.
    struct wlr_surface *only;
};

static void sweep_one_buffer(struct wlr_scene_buffer *buffer, int sx, int sy,
        void *userdata) {
    struct oxide_corner_sweep_ctx *ctx = userdata;
    struct wlr_scene_surface *scene_surface =
            wlr_scene_surface_try_from_buffer(buffer);
    if (scene_surface == NULL) {
        return;
    }
    if (ctx->only != NULL && scene_surface->surface != ctx->only) {
        return;
    }
    struct oxide_corner_mask *mask =
            pool_find_or_create(ctx->pool, scene_surface->surface);
    if (mask == NULL) {
        return; // OOM — leave this node as it was rather than crash
    }
    mask->seen = true;
    int offset_x = sx - ctx->tree_lx;
    int offset_y = sy - ctx->tree_ly;
    mask_one_buffer(ctx->renderer, ctx->allocator, ctx->prog, buffer,
            scene_surface->surface, offset_x, offset_y, ctx->tile_w,
            ctx->tile_h, ctx->radius, mask);
}

bool oxide_toplevel_apply_corner_radius(struct wlr_renderer *renderer,
        struct wlr_allocator *allocator, void *corner_program,
        struct wlr_scene_tree *scene_tree, struct wlr_xdg_toplevel *xdg_toplevel,
        struct wlr_surface *only, int radius, void **pool_inout) {
    if (corner_program == NULL) {
        return false;
    }
    struct oxide_corner_program *prog = corner_program;

    struct oxide_corner_mask_pool *pool = *pool_inout;
    if (pool == NULL) {
        pool = calloc(1, sizeof(*pool));
        if (pool == NULL) {
            return false;
        }
        *pool_inout = pool;
    }

    bool full_sweep = only == NULL;
    if (full_sweep) {
        for (size_t i = 0; i < pool->count; i++) {
            pool->masks[i].seen = false;
        }
    }

    // The toplevel's own outer box, logical units: geometry if the client
    // ever set one (the window-management-visible box, excluding any
    // invisible shadow margin a client's buffer might carry) — the same box
    // wlr_scene_xdg_surface_create already offsets every surface under this
    // tree by (-geometry.x, -geometry.y) to align to, so tree-local (0,0) is
    // already this box's top-left corner. Falls back to the root surface's
    // own committed size only when geometry was never set, matching the
    // xdg-shell spec's own default ("if never set, the value is the full
    // bounds of the surface").
    struct wlr_xdg_surface *xdg_surface = xdg_toplevel->base;
    int tile_w = xdg_surface->geometry.width;
    int tile_h = xdg_surface->geometry.height;
    if (tile_w <= 0 || tile_h <= 0) {
        tile_w = xdg_surface->surface->current.width;
        tile_h = xdg_surface->surface->current.height;
    }
    if (tile_w <= 0 || tile_h <= 0) {
        return false;
    }

    int tree_lx = 0, tree_ly = 0;
    if (!wlr_scene_node_coords(&scene_tree->node, &tree_lx, &tree_ly)) {
        // Tree (or an ancestor) is currently disabled — e.g. a hidden
        // workspace. Nothing visible to mask right now.
        return false;
    }

    struct oxide_corner_sweep_ctx ctx = {
        .renderer = renderer,
        .allocator = allocator,
        .prog = prog,
        .pool = pool,
        .tree_lx = tree_lx,
        .tree_ly = tree_ly,
        .tile_w = tile_w,
        .tile_h = tile_h,
        .radius = radius,
        .only = only,
    };
    wlr_scene_node_for_each_buffer(&scene_tree->node, sweep_one_buffer, &ctx);

    if (full_sweep) {
        // A subsurface that stopped existing since the last full sweep: free
        // its swapchain and drop it from the pool rather than let it leak.
        size_t write = 0;
        for (size_t i = 0; i < pool->count; i++) {
            if (pool->masks[i].seen) {
                pool->masks[write++] = pool->masks[i];
            } else {
                wlr_swapchain_destroy(pool->masks[i].swapchain);
            }
        }
        pool->count = write;
    }

    return true;
}

// A toplevel is about to lose one of its subsurfaces: free that surface's
// mask swapchain immediately rather than wait for the next full sweep to
// notice it's gone.
void oxide_corner_mask_pool_forget(void *pool_ptr, struct wlr_surface *surface) {
    if (pool_ptr == NULL) {
        return;
    }
    pool_forget(pool_ptr, surface);
}

// Unlike the shared corner_program (process-lifetime, see above), each
// toplevel's mask pool must be freed when that window is destroyed — called
// from Rust's per-window destroy handler. A no-op if masking was never used
// (pool_ptr NULL — corner_radius was never on, or the window never
// committed while it was).
void oxide_corner_mask_pool_destroy(void *pool_ptr) {
    if (pool_ptr == NULL) {
        return;
    }
    struct oxide_corner_mask_pool *pool = pool_ptr;
    for (size_t i = 0; i < pool->count; i++) {
        wlr_swapchain_destroy(pool->masks[i].swapchain);
    }
    free(pool->masks);
    free(pool);
}
