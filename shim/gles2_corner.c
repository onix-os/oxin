#define WLR_USE_UNSTABLE
#include <drm_fourcc.h>
#include <EGL/egl.h>
#include <GLES2/gl2.h>
#include <GLES2/gl2ext.h>
#include <math.h>
#include <pixman.h>
#include <stdint.h>
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
#include <wlr/util/box.h>
#include <wlr/util/log.h>

#include "oxide_shim.h"

// --- rounded-corner GLES2 masking --------------------------------------------
//
// wlroots' scene-graph/render-pass API has no corner-radius or arbitrary-mask
// primitive (checked: wlr_render_pass_add_texture/add_rect only support a
// rectangular clip and a flat alpha scalar). Real per-pixel rounding needs a
// compositor-owned GLES2 program that masks a window's content into a
// compositor-owned buffer, which this file's per-commit entry point then
// swaps into the scene graph in place of the client's own buffer(s).
//
// A window's visible content is not always just its root xdg_surface —
// Firefox, for one, paints into a wl_subsurface rather than its root surface.
// So "masking a window" here is a two-pass job: Pass A flattens every
// relevant buffer under the toplevel's scene tree (root surface + any
// subsurfaces; popups are excluded on purpose — they can legitimately extend
// past the window's own bounds) into one intermediate compositor-owned
// buffer, in the same back-to-front order the scene graph itself would paint
// them. Pass B runs the rounded-rect mask shader over that flattened result,
// exactly as it would over a single surface's texture.
//
// Two texture-target variants of both the mask and the blit (flattening)
// shaders are compiled because wlr_gles2_texture_get_attribs can report
// either GL_TEXTURE_2D or GL_TEXTURE_EXTERNAL_OES depending on the buffer's
// import path — using the wrong sampler type for the bound texture target
// renders solid black, not a GL error, so both must exist.

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
// The output is written premultiplied: color.rgb is assumed already
// premultiplied by the source's own alpha (standard Wayland buffer
// convention), so scaling it by `coverage` alone — without an extra
// multiply by src_alpha — keeps it premultiplied once the final alpha is
// also `src_alpha * coverage`.
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

// Flattens one source texture into the (otherwise untouched) destination,
// scaled by its own scene-buffer opacity. Same premultiplied-output
// reasoning as the mask shader above: scaling a premultiplied color by a
// single scalar (here u_opacity, there coverage) keeps it premultiplied.
static const char *BLIT_FRAGMENT_SRC_TEMPLATE =
    "%s"
    "precision mediump float;\n"
    "varying vec2 v_texcoord;\n"
    "uniform %s u_tex;\n"
    "uniform float u_opacity;\n"
    "uniform float u_has_alpha;\n"
    "void main() {\n"
    "    vec4 color = texture2D(u_tex, v_texcoord);\n"
    "    float a = mix(1.0, color.a, u_has_alpha);\n"
    "    gl_FragColor = vec4(color.rgb * u_opacity, a * u_opacity);\n"
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

struct oxide_blit_variant {
    GLuint program;
    GLint attrib_pos;
    GLint attrib_texcoord;
    GLint uniform_tex;
    GLint uniform_opacity;
    GLint uniform_has_alpha;
};

struct oxide_corner_program {
    struct oxide_corner_variant tex2d;
    struct oxide_corner_variant tex_oes;
    struct oxide_blit_variant blit_tex2d;
    struct oxide_blit_variant blit_tex_oes;
    GLuint quad_vbo;  // static fullscreen unit quad, used by the mask pass
    GLuint blit_vbo;  // dynamic, re-uploaded per source in the compositing pass
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

// Same linking dance as build_variant, for the simpler blit (flattening)
// shader pair.
static bool build_blit_variant(struct oxide_blit_variant *out, GLuint vertex,
        const char *sampler_type, const char *extension_directive) {
    memset(out, 0, sizeof(*out));

    char fragment_src[1024];
    snprintf(fragment_src, sizeof(fragment_src), BLIT_FRAGMENT_SRC_TEMPLATE,
            extension_directive, sampler_type);
    GLuint fragment = compile_shader(GL_FRAGMENT_SHADER, fragment_src);
    if (fragment == 0) {
        return false;
    }

    GLuint program = glCreateProgram();
    glAttachShader(program, vertex);
    glAttachShader(program, fragment);
    glLinkProgram(program);
    glDeleteShader(fragment);

    GLint ok = GL_FALSE;
    glGetProgramiv(program, GL_LINK_STATUS, &ok);
    if (!ok) {
        char log[512];
        glGetProgramInfoLog(program, sizeof(log), NULL, log);
        wlr_log(WLR_ERROR, "0xin: corner-radius blit program link failed: %s", log);
        glDeleteProgram(program);
        return false;
    }

    out->program = program;
    out->attrib_pos = glGetAttribLocation(program, "pos");
    out->attrib_texcoord = glGetAttribLocation(program, "texcoord");
    out->uniform_tex = glGetUniformLocation(program, "u_tex");
    out->uniform_opacity = glGetUniformLocation(program, "u_opacity");
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

    // The two variants of each shader are built independently. Which one a
    // given texture needs depends on how its buffer was imported, so a driver
    // without GL_OES_EGL_image_external should cost rounding only on the
    // buffers that actually need it, not every window on the screen.
    bool have_2d = vertex != 0 && build_variant(&p->tex2d, vertex, "sampler2D", "");
    bool have_oes = vertex != 0 && build_variant(&p->tex_oes, vertex,
            "samplerExternalOES", "#extension GL_OES_EGL_image_external : enable\n");
    bool have_blit_2d = vertex != 0
            && build_blit_variant(&p->blit_tex2d, vertex, "sampler2D", "");
    bool have_blit_oes = vertex != 0
            && build_blit_variant(&p->blit_tex_oes, vertex, "samplerExternalOES",
                    "#extension GL_OES_EGL_image_external : enable\n");
    // Masking a window is now always a composite-then-mask job (even a
    // window with no subsurfaces is "composited" through a single source),
    // so both the mask and the blit shader need at least one working
    // variant — losing either disables corner_radius outright rather than
    // degrading to some partial mode.
    bool ok = (have_2d || have_oes) && (have_blit_2d || have_blit_oes);
    if (vertex != 0) {
        glDeleteShader(vertex);
    }

    if (ok) {
        glGenBuffers(1, &p->quad_vbo);
        glBindBuffer(GL_ARRAY_BUFFER, p->quad_vbo);
        glBufferData(GL_ARRAY_BUFFER, sizeof(QUAD_VERTICES), QUAD_VERTICES,
                GL_STATIC_DRAW);
        glGenBuffers(1, &p->blit_vbo);
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
        if (p->blit_tex2d.program) {
            glDeleteProgram(p->blit_tex2d.program);
        }
        if (p->blit_tex_oes.program) {
            glDeleteProgram(p->blit_tex_oes.program);
        }
        free(p);
        return NULL;
    }
    if (have_2d && have_oes && have_blit_2d && have_blit_oes) {
        wlr_log(WLR_INFO, "0xin: corner-radius GLES2 program ready");
    } else {
        // Worth a warning rather than silence: rounding/compositing will
        // look inconsistent between windows, and this says which half is
        // missing and why.
        wlr_log(WLR_ERROR, "0xin: corner-radius GLES2 program ready with gaps "
                "(mask: 2d=%d oes=%d, blit: 2d=%d oes=%d) — some windows or "
                "some of their subsurfaces stay unrounded/unflattened",
                have_2d, have_oes, have_blit_2d, have_blit_oes);
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
// oxide_scene_tree_set_opacity in xdg_shell.c already uses) looking for the
// one node backed by the toplevel's own root surface.
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

static void enable_buffer_node(struct wlr_scene_buffer *buffer, int sx, int sy,
        void *userdata) {
    (void)sx;
    (void)sy;
    (void)userdata;
    wlr_scene_node_set_enabled(&buffer->node, true);
}

// One buffer collected from under a toplevel's scene tree for compositing:
// the root surface itself, or one of its subsurfaces. Never a popup (see
// collect_mask_source).
struct oxide_mask_source {
    struct wlr_texture *texture;
    bool owns_texture; // true only for the no-wlr_surface fallback below
    struct wlr_scene_buffer *scene_buffer;
    float opacity;
    struct wlr_fbox src_box;
    int x, y; // window-local, logical pixels (layout coords minus tree origin)
    int w, h; // dest size, logical pixels
};

struct oxide_mask_collect {
    int tree_lx, tree_ly;
    struct wlr_surface *root_surface;
    struct wlr_renderer *renderer;
    struct oxide_mask_source *items;
    size_t count, cap;
    bool overflow;
    struct wlr_scene_buffer *root_buffer;
};

// Collects every buffer under a toplevel's scene tree that should be part of
// its rounded, composited whole — the root surface and any wl_subsurfaces
// (e.g. Firefox, which paints its real content into one) — in the same
// back-to-front order wlr_scene_node_for_each_buffer already visits them.
// Popups are skipped: they are structurally distinct xdg_surfaces (own role,
// own node — see shim/xdg_shell.c's handle_new_popup) that can legitimately
// extend past the window's bounds, and must never be masked or flattened in.
static void collect_mask_source(struct wlr_scene_buffer *buffer, int sx, int sy,
        void *userdata) {
    struct oxide_mask_collect *c = userdata;
    if (c->overflow) {
        return;
    }

    struct wlr_scene_surface *scene_surface = wlr_scene_surface_try_from_buffer(buffer);
    struct wlr_surface *surface = scene_surface != NULL ? scene_surface->surface : NULL;
    if (surface != NULL) {
        struct wlr_xdg_surface *xdg = wlr_xdg_surface_try_from_wlr_surface(surface);
        if (xdg != NULL && xdg->role == WLR_XDG_SURFACE_ROLE_POPUP) {
            return;
        }
    }

    struct wlr_texture *texture = NULL;
    bool owns_texture = false;
    if (surface != NULL) {
        texture = wlr_surface_get_texture(surface);
    } else if (buffer->buffer != NULL) {
        // A bare scene buffer under this tree with no wl_surface backing it
        // — not expected for an ordinary toplevel today, but handled
        // conservatively rather than dropped silently, since it's still
        // visually part of the window. This texture is ours; destroy it
        // once the compositing pass is done with it (see free_collect).
        texture = wlr_texture_from_buffer(c->renderer, buffer->buffer);
        owns_texture = texture != NULL;
    }
    if (texture == NULL || !wlr_texture_is_gles2(texture)) {
        if (owns_texture && texture != NULL) {
            wlr_texture_destroy(texture);
        }
        // Nothing committed on this buffer yet, or a non-GLES2 renderer —
        // skip this one source rather than failing the whole composite.
        return;
    }

    int x = sx - c->tree_lx;
    int y = sy - c->tree_ly;
    int w = buffer->dst_width > 0 ? buffer->dst_width : (int)texture->width;
    int h = buffer->dst_height > 0 ? buffer->dst_height : (int)texture->height;
    if (w <= 0 || h <= 0) {
        if (owns_texture) {
            wlr_texture_destroy(texture);
        }
        return;
    }

    if (c->count == c->cap) {
        size_t new_cap = c->cap == 0 ? 8 : c->cap * 2;
        struct oxide_mask_source *resized = realloc(c->items, new_cap * sizeof(*resized));
        if (resized == NULL) {
            if (owns_texture) {
                wlr_texture_destroy(texture);
            }
            c->overflow = true;
            return;
        }
        c->items = resized;
        c->cap = new_cap;
    }

    c->items[c->count] = (struct oxide_mask_source){
        .texture = texture,
        .owns_texture = owns_texture,
        .scene_buffer = buffer,
        .opacity = buffer->opacity,
        .src_box = buffer->src_box,
        .x = x,
        .y = y,
        .w = w,
        .h = h,
    };
    if (surface == c->root_surface) {
        c->root_buffer = buffer;
    }
    c->count++;
}

static void free_collect(struct oxide_mask_collect *c) {
    for (size_t i = 0; i < c->count; i++) {
        if (c->items[i].owns_texture) {
            wlr_texture_destroy(c->items[i].texture);
        }
    }
    free(c->items);
    c->items = NULL;
    c->count = 0;
    c->cap = 0;
}

// Fills a [x,y,u,v]*4 triangle-strip quad (same vertex layout as
// QUAD_VERTICES) placing a `dst_w`x`dst_h` rect at `(dst_x, dst_y)` (pixels,
// top-left origin) within a `canvas_w`x`canvas_h` destination, sampling
// `src_box` of a `tex_w`x`tex_h` texture (the whole texture if `src_box` is
// empty/NULL).
//
// v=1 is pinned to the top of the source image and v=0 to the bottom. This
// mirrors the correspondence the existing fullscreen mask quad already
// relies on (QUAD_VERTICES pairs NDC y=+1 with texcoord v=1), verified
// correct via grim screenshots when this masking pass was first built —
// getting this backwards would render every composited source upside down
// with no error.
static void compute_quad(float dst_x, float dst_y, float dst_w, float dst_h,
        float canvas_w, float canvas_h, const struct wlr_fbox *src_box,
        float tex_w, float tex_h, GLfloat verts_out[16]) {
    float left = -1.0f + 2.0f * dst_x / canvas_w;
    float right = -1.0f + 2.0f * (dst_x + dst_w) / canvas_w;
    float top = 1.0f - 2.0f * dst_y / canvas_h;
    float bottom = 1.0f - 2.0f * (dst_y + dst_h) / canvas_h;

    float sx0 = 0.0f, sy0 = 0.0f, sx1 = tex_w, sy1 = tex_h;
    if (src_box != NULL && !wlr_fbox_empty(src_box)) {
        sx0 = (float)src_box->x;
        sy0 = (float)src_box->y;
        sx1 = sx0 + (float)src_box->width;
        sy1 = sy0 + (float)src_box->height;
    }
    float u0 = sx0 / tex_w;
    float u1 = sx1 / tex_w;
    float v_top = 1.0f - sy0 / tex_h;
    float v_bottom = 1.0f - sy1 / tex_h;

    GLfloat v[16] = {
        left,  bottom, u0, v_bottom,
        right, bottom, u1, v_bottom,
        left,  top,    u0, v_top,
        right, top,    u1, v_top,
    };
    memcpy(verts_out, v, sizeof(v));
}

// Pass A: flattens every source in `collect` into one new/reused
// `composite_swapchain`-backed buffer sized `canvas_w`x`canvas_h`, in the
// order they were collected (already back-to-front). Returns a locked
// `wlr_buffer*` on success (caller unlocks once Pass B is done with it), or
// NULL on any failure.
static struct wlr_buffer *render_composite_pass(struct wlr_renderer *renderer,
        struct wlr_allocator *allocator, struct oxide_corner_program *prog,
        struct oxide_mask_collect *collect, int bbox_x0, int bbox_y0,
        int canvas_w, int canvas_h, float buffer_scale,
        void **swapchain_inout, int *swapchain_w_inout, int *swapchain_h_inout) {
    struct wlr_swapchain *swapchain = *swapchain_inout;
    if (swapchain == NULL || *swapchain_w_inout != canvas_w || *swapchain_h_inout != canvas_h) {
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
                    "0xin: no ARGB8888 render format available for corner-radius compositing");
            *swapchain_inout = NULL;
            return NULL;
        }
        swapchain = wlr_swapchain_create(allocator, canvas_w, canvas_h, format);
        if (swapchain == NULL) {
            wlr_log(WLR_ERROR,
                    "0xin: failed to create corner-radius composite swapchain (%dx%d)",
                    canvas_w, canvas_h);
            *swapchain_inout = NULL;
            return NULL;
        }
        *swapchain_inout = swapchain;
        *swapchain_w_inout = canvas_w;
        *swapchain_h_inout = canvas_h;
    }

    struct wlr_buffer *buffer = wlr_swapchain_acquire(swapchain);
    if (buffer == NULL) {
        wlr_log(WLR_ERROR, "0xin: corner-radius composite swapchain acquire failed");
        return NULL;
    }
    struct wlr_render_pass *pass = wlr_renderer_begin_buffer_pass(renderer, buffer, NULL);
    if (pass == NULL) {
        wlr_buffer_unlock(buffer);
        return NULL;
    }
    GLuint fbo = wlr_gles2_renderer_get_buffer_fbo(renderer, buffer);

    // Save every piece of GL state this draw touches — see the identical
    // block (and its comment) in render_corner_mask below; the same
    // same-frame state-leak concern applies here.
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

    // Block until every source's DMA-BUF import/upload is actually done
    // before sampling any of them — see render_corner_mask's identical
    // glFinish() comment for why this matters on real hardware.
    glFinish();

    glBindFramebuffer(GL_FRAMEBUFFER, fbo);
    GLenum fbo_status = glCheckFramebufferStatus(GL_FRAMEBUFFER);
    if (fbo_status != GL_FRAMEBUFFER_COMPLETE) {
        wlr_log(WLR_ERROR,
                "0xin: corner-radius composite FBO incomplete (0x%x) for %dx%d buffer",
                fbo_status, canvas_w, canvas_h);
    }
    glViewport(0, 0, canvas_w, canvas_h);
    glClearColor(0.0f, 0.0f, 0.0f, 0.0f);
    glClear(GL_COLOR_BUFFER_BIT);
    glEnable(GL_BLEND);
    // Standard premultiplied "over" compositing onto a transparent canvas.
    glBlendFuncSeparate(GL_ONE, GL_ONE_MINUS_SRC_ALPHA, GL_ONE, GL_ONE_MINUS_SRC_ALPHA);
    glBindBuffer(GL_ARRAY_BUFFER, prog->blit_vbo);

    for (size_t i = 0; i < collect->count; i++) {
        struct oxide_mask_source *s = &collect->items[i];
        struct wlr_gles2_texture_attribs attribs;
        wlr_gles2_texture_get_attribs(s->texture, &attribs);
        struct oxide_blit_variant *variant =
                attribs.target == GL_TEXTURE_EXTERNAL_OES ? &prog->blit_tex_oes
                                                            : &prog->blit_tex2d;
        if (variant->program == 0) {
            wlr_log(WLR_DEBUG,
                    "0xin: corner-radius: no blit shader for texture target 0x%x — "
                    "dropping one source from the composite",
                    attribs.target);
            continue;
        }

        float dst_x = ((float)s->x - (float)bbox_x0) * buffer_scale;
        float dst_y = ((float)s->y - (float)bbox_y0) * buffer_scale;
        float dst_w = (float)s->w * buffer_scale;
        float dst_h = (float)s->h * buffer_scale;

        GLfloat verts[16];
        compute_quad(dst_x, dst_y, dst_w, dst_h, (float)canvas_w, (float)canvas_h,
                &s->src_box, (float)s->texture->width, (float)s->texture->height, verts);
        glBufferData(GL_ARRAY_BUFFER, sizeof(verts), verts, GL_DYNAMIC_DRAW);

        glUseProgram(variant->program);
        glActiveTexture(GL_TEXTURE0);
        glBindTexture(attribs.target, attribs.tex);
        // See render_corner_mask's identical call for why this is needed
        // every draw regardless of the texture's own prior state.
        glTexParameteri(attribs.target, GL_TEXTURE_MIN_FILTER, GL_LINEAR);
        glTexParameteri(attribs.target, GL_TEXTURE_MAG_FILTER, GL_LINEAR);
        glTexParameteri(attribs.target, GL_TEXTURE_WRAP_S, GL_CLAMP_TO_EDGE);
        glTexParameteri(attribs.target, GL_TEXTURE_WRAP_T, GL_CLAMP_TO_EDGE);
        glUniform1i(variant->uniform_tex, 0);
        glUniform1f(variant->uniform_opacity, s->opacity);
        glUniform1f(variant->uniform_has_alpha, attribs.has_alpha ? 1.0f : 0.0f);

        glEnableVertexAttribArray(variant->attrib_pos);
        glEnableVertexAttribArray(variant->attrib_texcoord);
        glVertexAttribPointer(variant->attrib_pos, 2, GL_FLOAT, GL_FALSE,
                4 * sizeof(GLfloat), (void *)0);
        glVertexAttribPointer(variant->attrib_texcoord, 2, GL_FLOAT, GL_FALSE,
                4 * sizeof(GLfloat), (void *)(2 * sizeof(GLfloat)));
        glDrawArrays(GL_TRIANGLE_STRIP, 0, 4);
        glDisableVertexAttribArray(variant->attrib_pos);
        glDisableVertexAttribArray(variant->attrib_texcoord);
    }
    log_gl_errors("composite");

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
        wlr_log(WLR_ERROR, "0xin: corner-radius composite render pass submit failed");
        wlr_buffer_unlock(buffer);
        return NULL;
    }
    return buffer;
}

// Pass B: renders `texture` (Pass A's flattened result) through the
// rounded-rect mask shader into a new/reused `swapchain`-backed buffer sized
// `w`x`h` (buffer pixels). `radius_buffer_px` is already converted to buffer
// pixels by the caller. Returns a locked `wlr_buffer*` on success (caller
// sets it on the scene node and unlocks), or NULL on any failure.
static struct wlr_buffer *render_corner_mask(struct wlr_renderer *renderer,
        struct wlr_allocator *allocator, struct oxide_corner_program *prog,
        struct wlr_texture *texture, int w, int h, float radius_buffer_px,
        void **swapchain_inout, int *swapchain_w_inout, int *swapchain_h_inout) {
    if (!wlr_texture_is_gles2(texture)) {
        wlr_log(WLR_ERROR,
                "0xin: corner-radius: composite texture is not GLES2 — masking skipped");
        return NULL;
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
        return NULL;
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
            return NULL;
        }
        swapchain = wlr_swapchain_create(allocator, w, h, format);
        if (swapchain == NULL) {
            wlr_log(WLR_ERROR, "0xin: failed to create corner-radius swapchain (%dx%d)",
                    w, h);
            *swapchain_inout = NULL;
            return NULL;
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
        return NULL;
    }

    struct wlr_render_pass *pass =
            wlr_renderer_begin_buffer_pass(renderer, mask_buffer, NULL);
    if (pass == NULL) {
        wlr_buffer_unlock(mask_buffer);
        return NULL;
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
    // module doc comment) accepts a wait_timeline/explicit-sync point before
    // sampling a texture; our raw GL sampling below has no equivalent, so on
    // a driver where a DMA-BUF import/upload genuinely races our read
    // (observed on real Adreno hardware — not reproduced on the nested Intel
    // path), we could sample before the content is actually there and render
    // black/incomplete. Block until all prior GPU work is actually complete
    // before touching this texture, including the composite pass that just
    // produced it. Costs real time; correctness comes first.
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
    // (GL_NEAREST_MIPMAP_LINEAR) requires a full mipmap chain — single-level
    // textures are "incomplete" without this, and GL silently samples them
    // as opaque black, no error raised. Desktop Mesa/Intel is lenient about
    // this in practice; strict mobile drivers (observed on Adreno) are not.
    glTexParameteri(attribs.target, GL_TEXTURE_MIN_FILTER, GL_LINEAR);
    glTexParameteri(attribs.target, GL_TEXTURE_MAG_FILTER, GL_LINEAR);
    glTexParameteri(attribs.target, GL_TEXTURE_WRAP_S, GL_CLAMP_TO_EDGE);
    glTexParameteri(attribs.target, GL_TEXTURE_WRAP_T, GL_CLAMP_TO_EDGE);
    glUniform1i(variant->uniform_tex, 0);
    glUniform2f(variant->uniform_size, (float)w, (float)h);
    glUniform1f(variant->uniform_radius, radius_buffer_px);
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
    log_gl_errors("mask draw");

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
        return NULL;
    }
    return mask_buffer;
}

void oxide_toplevel_restore_corner_mask(struct wlr_scene_tree *scene_tree,
        struct wlr_surface *root_surface) {
    wlr_scene_node_for_each_buffer(&scene_tree->node, enable_buffer_node, NULL);

    struct oxide_corner_target target = {
        .root_surface = root_surface,
        .found = NULL,
    };
    wlr_scene_node_for_each_buffer(&scene_tree->node, find_root_scene_buffer, &target);
    if (target.found == NULL) {
        return;
    }
    wlr_scene_node_set_position(&target.found->node, 0, 0);
    wlr_scene_buffer_set_buffer(target.found,
            root_surface->buffer != NULL ? &root_surface->buffer->base : NULL);
    wlr_scene_buffer_set_dest_size(target.found, root_surface->current.width,
            root_surface->current.height);
    wlr_scene_buffer_set_source_box(target.found, NULL);
    wlr_scene_buffer_set_opaque_region(target.found, &root_surface->opaque_region);
}

bool oxide_toplevel_apply_corner_mask(struct wlr_renderer *renderer,
        struct wlr_allocator *allocator, void *corner_program,
        struct wlr_scene_tree *scene_tree, struct wlr_surface *root_surface,
        int radius, void **swapchain_inout, int *swapchain_w_inout,
        int *swapchain_h_inout, void **composite_swapchain_inout,
        int *composite_swapchain_w_inout, int *composite_swapchain_h_inout) {
    // Always start from a known-good, fully-visible state. Guarantees a
    // transient failure anywhere below never leaves the window partially
    // hidden — see the function's doc comment in oxide_shim.h.
    oxide_toplevel_restore_corner_mask(scene_tree, root_surface);

    if (corner_program == NULL || radius <= 0) {
        return false;
    }
    struct oxide_corner_program *prog = corner_program;

    int root_buffer_w = root_surface->current.buffer_width;
    int root_buffer_h = root_surface->current.buffer_height;
    int root_logical_w = root_surface->current.width;
    int root_logical_h = root_surface->current.height;
    if (root_buffer_w <= 0 || root_buffer_h <= 0 || root_logical_w <= 0
            || root_logical_h <= 0) {
        return false;
    }
    // `corner_radius` arrives in logical pixels; everything rendered here
    // works in the surface's own buffer-pixel space. At scale 1 those match;
    // at any other scale they don't, so every pixel quantity derived below
    // is scaled by this same ratio — the surface's own, rather than a
    // separately-tracked output scale, so it can never drift out of sync
    // with the buffer actually being masked.
    float buffer_scale = (float)root_buffer_w / (float)root_logical_w;

    int lx = 0, ly = 0;
    wlr_scene_node_coords(&scene_tree->node, &lx, &ly);

    struct oxide_mask_collect collect = {0};
    collect.tree_lx = lx;
    collect.tree_ly = ly;
    collect.root_surface = root_surface;
    collect.renderer = renderer;
    wlr_scene_node_for_each_buffer(&scene_tree->node, collect_mask_source, &collect);

    bool ok = false;
    struct wlr_buffer *composite_buffer = NULL;
    struct wlr_texture *composite_texture = NULL;
    struct wlr_buffer *mask_buffer = NULL;
    int bbox_x0 = 0, bbox_y0 = 0, bbox_w = 0, bbox_h = 0;

    if (collect.overflow || collect.count == 0 || collect.root_buffer == NULL) {
        wlr_log(WLR_ERROR,
                "0xin: corner-radius: collecting buffers for surface %p failed "
                "(overflow=%d count=%zu root_found=%d) — masking skipped",
                (void *)root_surface, collect.overflow, collect.count,
                collect.root_buffer != NULL);
        goto cleanup;
    }

    {
        int bbox_x1 = INT32_MIN, bbox_y1 = INT32_MIN;
        bbox_x0 = INT32_MAX;
        bbox_y0 = INT32_MAX;
        for (size_t i = 0; i < collect.count; i++) {
            struct oxide_mask_source *s = &collect.items[i];
            if (s->x < bbox_x0) bbox_x0 = s->x;
            if (s->y < bbox_y0) bbox_y0 = s->y;
            if (s->x + s->w > bbox_x1) bbox_x1 = s->x + s->w;
            if (s->y + s->h > bbox_y1) bbox_y1 = s->y + s->h;
        }
        bbox_w = bbox_x1 - bbox_x0;
        bbox_h = bbox_y1 - bbox_y0;
    }

    // Guard against a stray/misbehaving subsurface inflating the bounding
    // box arbitrarily (a large negative offset, say) — cap at 4x the root
    // surface's own logical size and leave the window unmasked rather than
    // allocate an unbounded buffer.
    if (bbox_w <= 0 || bbox_h <= 0 || bbox_w > root_logical_w * 4
            || bbox_h > root_logical_h * 4) {
        wlr_log(WLR_ERROR,
                "0xin: corner-radius: bounding box %dx%d for surface %p looks "
                "wrong (root is %dx%d) — masking skipped",
                bbox_w, bbox_h, (void *)root_surface, root_logical_w, root_logical_h);
        goto cleanup;
    }

    {
        int canvas_w = (int)lroundf((float)bbox_w * buffer_scale);
        int canvas_h = (int)lroundf((float)bbox_h * buffer_scale);
        if (canvas_w <= 0 || canvas_h <= 0) {
            goto cleanup;
        }

        composite_buffer = render_composite_pass(renderer, allocator, prog, &collect,
                bbox_x0, bbox_y0, canvas_w, canvas_h, buffer_scale,
                composite_swapchain_inout, composite_swapchain_w_inout,
                composite_swapchain_h_inout);
        if (composite_buffer == NULL) {
            goto cleanup;
        }

        composite_texture = wlr_texture_from_buffer(renderer, composite_buffer);
        if (composite_texture == NULL) {
            wlr_log(WLR_ERROR,
                    "0xin: corner-radius: failed to import composite buffer as a texture");
            goto cleanup;
        }

        mask_buffer = render_corner_mask(renderer, allocator, prog, composite_texture,
                canvas_w, canvas_h, (float)radius * buffer_scale, swapchain_inout,
                swapchain_w_inout, swapchain_h_inout);
        if (mask_buffer == NULL) {
            goto cleanup;
        }
    }

    // Repurpose the root surface's own scene_buffer node to show the
    // composited, masked whole-window image; every other collected node's
    // content is now baked into it, so hide those below.
    wlr_scene_node_set_position(&collect.root_buffer->node, bbox_x0, bbox_y0);
    wlr_scene_buffer_set_buffer(collect.root_buffer, mask_buffer);
    wlr_scene_buffer_set_dest_size(collect.root_buffer, bbox_w, bbox_h);
    wlr_scene_buffer_set_source_box(collect.root_buffer, NULL);

    {
        // Tell the scene which pixels of the composite are still opaque, same
        // reasoning as the single-surface case this generalizes: start from
        // what the root surface itself declared (translated into the bbox's
        // own coordinate space, since the root sits at window-local (0,0)
        // but the bbox's origin can be elsewhere if a subsurface extends
        // above/left of it), not a full rect — a transparent terminal must
        // not claim an opaque background that was never drawn — then
        // subtract the four corners the mask just cut.
        pixman_region32_t opaque;
        pixman_region32_init(&opaque);
        pixman_region32_copy(&opaque, &root_surface->opaque_region);
        pixman_region32_translate(&opaque, -bbox_x0, -bbox_y0);
        pixman_region32_intersect_rect(&opaque, &opaque, 0, 0, bbox_w, bbox_h);
        int corner = radius;
        if (corner > bbox_w / 2) corner = bbox_w / 2;
        if (corner > bbox_h / 2) corner = bbox_h / 2;
        if (corner > 0) {
            pixman_region32_t corners;
            pixman_region32_init_rect(&corners, 0, 0, corner, corner);
            pixman_region32_union_rect(&corners, &corners, bbox_w - corner, 0, corner,
                    corner);
            pixman_region32_union_rect(&corners, &corners, 0, bbox_h - corner, corner,
                    corner);
            pixman_region32_union_rect(&corners, &corners, bbox_w - corner,
                    bbox_h - corner, corner, corner);
            pixman_region32_subtract(&opaque, &opaque, &corners);
            pixman_region32_fini(&corners);
        }
        wlr_scene_buffer_set_opaque_region(collect.root_buffer, &opaque);
        pixman_region32_fini(&opaque);
    }

    for (size_t i = 0; i < collect.count; i++) {
        if (collect.items[i].scene_buffer != collect.root_buffer) {
            wlr_scene_node_set_enabled(&collect.items[i].scene_buffer->node, false);
        }
    }

    ok = true;

cleanup:
    if (composite_texture != NULL) {
        wlr_texture_destroy(composite_texture);
    }
    if (composite_buffer != NULL) {
        wlr_buffer_unlock(composite_buffer);
    }
    if (mask_buffer != NULL) {
        wlr_buffer_unlock(mask_buffer);
    }
    free_collect(&collect);
    return ok;
}

// Unlike the shared corner_program (process-lifetime, see above), each
// toplevel's swapchains must be freed when that window is destroyed — called
// once each for corner_swapchain and composite_swapchain from Rust's
// per-window destroy handler.
void oxide_swapchain_destroy(void *swapchain) {
    if (swapchain != NULL) {
        wlr_swapchain_destroy(swapchain);
    }
}
