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
#include <wlr/util/log.h>

#include "oxide_shim.h"

// --- rounded-corner GLES2 masking --------------------------------------------
//
// wlroots' scene-graph/render-pass API has no corner-radius or arbitrary-mask
// primitive (checked: wlr_render_pass_add_texture/add_rect only support a
// rectangular clip and a flat alpha scalar). Real per-pixel rounding needs a
// compositor-owned GLES2 program that masks a window's texture into a
// compositor-owned buffer, which src/toplevel.rs's handle_commit then swaps
// into the scene graph in place of the client's own buffer. This file only
// compiles/links that program; the per-commit masking pass that actually
// uses it lands separately.
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
// The output is written premultiplied (rgb and a both scaled by the mask
// coverage) — the correct convention depends on how the destination buffer
// pass blends, and is one of the things Stage 3's real rendering must
// confirm visually (a straight-alpha destination would need color.rgb left
// unscaled instead).
static const char *FRAGMENT_SRC_TEMPLATE =
    "%s"
    "precision mediump float;\n"
    "varying vec2 v_texcoord;\n"
    "uniform %s u_tex;\n"
    "uniform vec2 u_size;\n"
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
    "    vec2 pixel_pos = v_texcoord * u_size - u_size * 0.5;\n"
    "    float dist = rounded_box_sdf(pixel_pos, u_size * 0.5, u_radius);\n"
    "    float coverage = 1.0 - smoothstep(-1.0, 1.0, dist);\n"
    "    gl_FragColor = vec4(color.rgb * coverage, src_alpha * coverage);\n"
    "}\n";

struct oxide_corner_variant {
    GLuint program;
    GLint attrib_pos;
    GLint attrib_texcoord;
    GLint uniform_tex;
    GLint uniform_size;
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

struct oxide_corner_target {
    struct wlr_surface *root_surface;
    struct wlr_scene_buffer *found;
};

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

// Walks every wlr_scene_buffer under a toplevel's scene tree (same primitive
// oxide_scene_tree_set_opacity above already uses) looking for the one node
// backed by the toplevel's own root surface — never a popup/subsurface, which
// wlr_scene_xdg_surface_create also parents under the same tree and which
// must stay unmasked.
static void find_root_scene_buffer(struct wlr_scene_buffer *buffer, int sx,
        int sy, void *userdata) {
    (void)sx;
    (void)sy;
    struct oxide_corner_target *target = userdata;
    if (target->found != NULL) {
        return;
    }
    struct wlr_scene_surface *scene_surface =
            wlr_scene_surface_try_from_buffer(buffer);
    if (scene_surface != NULL && scene_surface->surface == target->root_surface) {
        target->found = buffer;
    }
}

// Renders `root_surface`'s current texture through the corner-radius shader
// into a compositor-owned GPU buffer, then swaps that buffer into the scene
// graph in place of the client's own — the only way to get real per-pixel
// masking, since wlroots' render-pass API has no clip/mask/shader hook.
// `radius` is in the same pixel units as the surface's buffer size (not yet
// adjusted for output scale — see the caller in src/toplevel.rs for the
// current state of that open question). The destination size restated below
// is read from the surface's OWN committed logical size
// (`root_surface->current.width/height` — already post any viewport/scale
// the client applied), not from 0xin's separately-tracked tile size: on a
// fractionally-scaled output, wlroots' automatic buffer-to-viewport scaling
// is driven by that same committed surface state via
// wlr_scene_xdg_surface_create's internal commit listener, which our own
// raw wlr_scene_buffer_set_buffer() call below does not go through — so
// without restating it explicitly the mask buffer displays at its full
// physical size instead. Using the surface's own authoritative value here
// (rather than a compositor-side tracked size that can rack up its own
// rounding, e.g. for clients — GTK4/libadwaita observed in practice — that
// compute their own viewport scale slightly differently) avoids a second,
// independent source of truth that can drift out of sync and skew or
// mis-center the result. Returns false (no-op, previous buffer untouched)
// on any failure — masking is best-effort, never fatal.
bool oxide_toplevel_apply_corner_radius(struct wlr_renderer *renderer,
        struct wlr_allocator *allocator, void *corner_program,
        struct wlr_scene_tree *scene_tree, struct wlr_surface *root_surface,
        int radius, void **swapchain_inout, int *swapchain_w_inout,
        int *swapchain_h_inout) {
    if (corner_program == NULL) {
        return false;
    }
    struct oxide_corner_program *prog = corner_program;

    struct oxide_corner_target target = {
        .root_surface = root_surface,
        .found = NULL,
    };
    wlr_scene_node_for_each_buffer(&scene_tree->node, find_root_scene_buffer,
            &target);
    if (target.found == NULL) {
        wlr_log(WLR_ERROR,
                "0xin: corner-radius found no scene_buffer for root surface "
                "%p under this toplevel's tree — likely rendering via a "
                "subsurface instead of its root surface; masking skipped",
                (void *)root_surface);
        return false;
    }

    struct wlr_texture *texture = wlr_surface_get_texture(root_surface);
    if (texture == NULL || !wlr_texture_is_gles2(texture)) {
        wlr_log(WLR_ERROR,
                "0xin: corner-radius: no GLES2 texture for root surface %p "
                "(texture=%p) — masking skipped",
                (void *)root_surface, (void *)texture);
        // No content committed yet, or 0xin is running a non-GLES2 renderer
        // (Vulkan/Pixman) — corner masking is GLES2-only; fail closed rather
        // than assume.
        return false;
    }
    struct wlr_gles2_texture_attribs attribs;
    wlr_gles2_texture_get_attribs(texture, &attribs);

    // Bail before allocating a swapchain if the shader this texture would need
    // never built (see the variant handling in the program constructor). The
    // window then keeps square corners instead of being masked with the wrong
    // sampler type, which renders solid black rather than failing loudly.
    const struct oxide_corner_variant *needed =
            attribs.target == GL_TEXTURE_EXTERNAL_OES ? &prog->tex_oes : &prog->tex2d;
    if (needed->program == 0) {
        return false;
    }

    int w = root_surface->current.buffer_width;
    int h = root_surface->current.buffer_height;
    if (w <= 0 || h <= 0) {
        return false;
    }

    struct wlr_swapchain *swapchain = *swapchain_inout;
    if (swapchain == NULL || *swapchain_w_inout != w || *swapchain_h_inout != h) {
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
            *swapchain_inout = NULL;
            return false;
        }
        swapchain = wlr_swapchain_create(allocator, w, h, format);
        if (swapchain == NULL) {
            wlr_log(WLR_ERROR, "0xin: failed to create corner-radius swapchain (%dx%d)",
                    w, h);
            *swapchain_inout = NULL;
            return false;
        }
        *swapchain_inout = swapchain;
        *swapchain_w_inout = w;
        *swapchain_h_inout = h;
        wlr_log(WLR_INFO,
                "0xin: corner-radius swapchain (re)created %dx%d, format 0x%08x, "
                "%zu modifier(s), source texture target %s",
                w, h, format->format, format->len,
                attribs.target == GL_TEXTURE_EXTERNAL_OES ? "EXTERNAL_OES" : "2D");
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
    // `radius` arrives in *logical* pixels (it is the config value), but u_size
    // is the buffer's *physical* size and the shader compares both in one
    // space. On a scaled output those are not the same unit: at scale 3 a
    // configured 40 would be cut as 40 buffer pixels — about 13 logical — so
    // the rounding silently shrank in proportion to the display's scale, and
    // was only ever correct at scale 1. Convert here, using the surface's own
    // buffer-to-logical ratio rather than an output scale looked up elsewhere,
    // so it matches the very buffer being masked.
    float buffer_scale = root_surface->current.width > 0
            ? (float)w / (float)root_surface->current.width
            : 1.0f;
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

    // Diagnostic: read back the buffer's actual center pixel (guaranteed
    // full corner-mask coverage — not near any edge) to see what the
    // shader really produced, rather than inferring from what shows on
    // screen afterward. Logged every call — this is temporary debugging
    // instrumentation, not meant to stay this verbose long-term.
    {
        unsigned char px[4] = {0, 0, 0, 0};
        glReadPixels(w / 2, h / 2, 1, 1, GL_RGBA, GL_UNSIGNED_BYTE, px);
        wlr_log(WLR_INFO,
                "0xin: corner-radius center pixel for surface %p: "
                "rgba(%u,%u,%u,%u), has_alpha=%d, target=%s, tex=%u",
                (void *)root_surface, px[0], px[1], px[2], px[3],
                attribs.has_alpha,
                attribs.target == GL_TEXTURE_EXTERNAL_OES ? "OES" : "2D",
                attribs.tex);
    }

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

    wlr_scene_buffer_set_buffer(target.found, mask_buffer);
    // Restate the logical display size explicitly (see the function's doc
    // comment) — our mask buffer is the surface's full physical size, but
    // on a fractionally-scaled output that must still be painted at the
    // compositor's own logical tile size, not 1:1. Read from the surface's
    // own committed state, not a separately-tracked compositor-side value —
    // this is exactly the size the client itself intended when it committed
    // this content, so it can never drift out of sync with what the buffer
    // actually contains. Also reset any leftover source crop from the
    // client's own buffer/viewport state, since our mask buffer is a full,
    // uncropped copy — a stale src_box would crop it incorrectly.
    // Tell the scene which of this buffer's pixels are still opaque. wlroots
    // uses that to decide whether anything *underneath* needs drawing at all,
    // and what it currently holds is the client's own region — for most
    // toplevels, the entire window. Leave it alone and the wallpaper beneath
    // the corners is never painted, so the transparent pixels the mask just
    // cut reveal an unpainted framebuffer: corners that read as solid black
    // instead of as whatever is behind the window. The shader is not at fault
    // there and neither is blending; nothing was ever drawn to blend with.
    //
    // Start from what the client actually declared rather than a full rect: a
    // terminal with transparency declares nothing opaque, and claiming
    // otherwise would drop the background behind *it*. Subtracting only the
    // four corners, rather than clearing the region outright, keeps the
    // occlusion optimisation for the body of the window — which is the whole
    // reason the hint exists.
    //
    // Logical coordinates throughout, matching set_dest_size below and
    // wlroots' own surface_reconfigure, which sets these two together in this
    // order.
    int logical_w = root_surface->current.width;
    int logical_h = root_surface->current.height;
    int corner = radius;
    if (corner > logical_w / 2) {
        corner = logical_w / 2;
    }
    if (corner > logical_h / 2) {
        corner = logical_h / 2;
    }
    pixman_region32_t opaque;
    pixman_region32_init(&opaque);
    pixman_region32_copy(&opaque, &root_surface->opaque_region);
    pixman_region32_intersect_rect(&opaque, &opaque, 0, 0, logical_w, logical_h);
    if (corner > 0) {
        pixman_region32_t corners;
        pixman_region32_init_rect(&corners, 0, 0, corner, corner);
        pixman_region32_union_rect(&corners, &corners,
                logical_w - corner, 0, corner, corner);
        pixman_region32_union_rect(&corners, &corners,
                0, logical_h - corner, corner, corner);
        pixman_region32_union_rect(&corners, &corners,
                logical_w - corner, logical_h - corner, corner, corner);
        pixman_region32_subtract(&opaque, &opaque, &corners);
        pixman_region32_fini(&corners);
    }
    wlr_scene_buffer_set_opaque_region(target.found, &opaque);
    pixman_region32_fini(&opaque);

    wlr_scene_buffer_set_dest_size(target.found, logical_w, logical_h);
    wlr_scene_buffer_set_source_box(target.found, NULL);
    wlr_buffer_unlock(mask_buffer);
    return true;
}

// Unlike the shared corner_program (process-lifetime, see above), each
// toplevel's swapchain must be freed when that window is destroyed — called
// from Rust's per-window destroy handler.
void oxide_swapchain_destroy(void *swapchain) {
    if (swapchain != NULL) {
        wlr_swapchain_destroy(swapchain);
    }
}
