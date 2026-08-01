//! Rounded window corners.
//!
//! Real per-pixel masking, not a decoration drawn on top: a window's whole
//! surface tree is composited into an offscreen texture, and that texture is
//! then drawn through a rounded-rect shader. So the corners are genuinely
//! transparent — correct over a wallpaper image, another window, or a reduced
//! `window_opacity` — and subsurfaces are masked along with everything else,
//! because they are part of the texture by the time the mask runs.
//!
//! This is the Smithay equivalent of the wlroots build's GLES2 masking pass,
//! and it costs the same thing: one extra offscreen pass per masked window per
//! frame. Fullscreen windows are excluded — rounding a window's edges against
//! the bare screen looks wrong, and skipping the pass on exactly the "video
//! playing, committing every frame" case is a real win.

use smithay::backend::renderer::element::texture::TextureRenderElement;
use smithay::backend::renderer::element::{Element, Id, Kind, RenderElement, UnderlyingStorage};
use smithay::backend::renderer::gles::{
    GlesError, GlesFrame, GlesRenderer, GlesTexProgram, GlesTexture, Uniform, UniformName,
    UniformType,
};
use smithay::backend::renderer::utils::{CommitCounter, DamageSet};
use smithay::utils::{Buffer as BufferCoords, Physical, Point, Rectangle, Scale, Transform};

/// The masking shader. Smithay substitutes `//_DEFINES_` and requires the
/// `EXTERNAL` / `NO_ALPHA` / `DEBUG_FLAGS` variants to be handled.
///
/// `v_coords` runs 0..1 across our offscreen texture, which is exactly the
/// window rect, so the fragment's position inside the window is `v_coords *
/// size` — no screen-space or output-transform maths needed.
const ROUNDED_SHADER: &str = r#"#version 100

//_DEFINES_

#if defined(EXTERNAL)
#extension GL_OES_EGL_image_external : require
#endif

precision mediump float;
#if defined(EXTERNAL)
uniform samplerExternalOES tex;
#else
uniform sampler2D tex;
#endif

uniform float alpha;
varying vec2 v_coords;

#if defined(DEBUG_FLAGS)
uniform float tint;
#endif

uniform float corner_radius;
uniform vec2 size;

void main() {
    vec4 color = texture2D(tex, v_coords);

#if defined(NO_ALPHA)
    color = vec4(color.rgb, 1.0) * alpha;
#else
    color = color * alpha;
#endif

#if defined(DEBUG_FLAGS)
    if (tint == 1.0)
        color = vec4(0.0, 0.2, 0.0, 0.2) + color * 0.8;
#endif

    // Signed distance to a rounded rectangle: positive outside the shape.
    vec2 half_size = size * 0.5;
    vec2 corner = half_size - vec2(corner_radius);
    vec2 offset = abs(v_coords * size - half_size) - corner;
    float distance = length(max(offset, 0.0)) + min(max(offset.x, offset.y), 0.0) - corner_radius;

    // One pixel of feathering, so the curve is not stair-stepped.
    gl_FragColor = color * (1.0 - smoothstep(-0.5, 0.5, distance));
}
"#;

/// The compiled masking program, built once per renderer.
#[derive(Clone)]
pub struct Corners {
    program: GlesTexProgram,
}

impl Corners {
    pub fn new(renderer: &mut GlesRenderer) -> Result<Corners, GlesError> {
        let program = renderer.compile_custom_texture_shader(
            ROUNDED_SHADER,
            &[
                UniformName::new("corner_radius", UniformType::_1f),
                UniformName::new("size", UniformType::_2f),
            ],
        )?;
        Ok(Corners { program })
    }

    /// Wrap a window's composited texture so it is drawn through the mask.
    pub fn mask(
        &self,
        texture: TextureRenderElement<GlesTexture>,
        radius: f32,
        size: (f32, f32),
    ) -> RoundedElement {
        RoundedElement {
            inner: texture,
            program: self.program.clone(),
            radius,
            size,
        }
    }
}

/// A texture element drawn through the rounded-corner program.
///
/// Everything except `draw` is the wrapped element's behaviour: the mask
/// changes which pixels survive, not where the window is or what it damages.
pub struct RoundedElement {
    inner: TextureRenderElement<GlesTexture>,
    program: GlesTexProgram,
    radius: f32,
    size: (f32, f32),
}

impl Element for RoundedElement {
    fn id(&self) -> &Id {
        self.inner.id()
    }

    fn current_commit(&self) -> CommitCounter {
        self.inner.current_commit()
    }

    fn src(&self) -> Rectangle<f64, BufferCoords> {
        self.inner.src()
    }

    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
        self.inner.geometry(scale)
    }

    fn location(&self, scale: Scale<f64>) -> Point<i32, Physical> {
        self.inner.location(scale)
    }

    fn transform(&self) -> Transform {
        self.inner.transform()
    }

    fn damage_since(
        &self,
        scale: Scale<f64>,
        commit: Option<CommitCounter>,
    ) -> DamageSet<i32, Physical> {
        self.inner.damage_since(scale, commit)
    }

    // Deliberately no opaque regions: the corners are transparent, so whatever
    // is behind the window has to be drawn there.
    fn alpha(&self) -> f32 {
        self.inner.alpha()
    }

    fn kind(&self) -> Kind {
        self.inner.kind()
    }
}

impl RenderElement<GlesRenderer> for RoundedElement {
    fn draw(
        &self,
        frame: &mut GlesFrame<'_, '_>,
        src: Rectangle<f64, BufferCoords>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
    ) -> Result<(), GlesError> {
        frame.override_default_tex_program(
            self.program.clone(),
            vec![
                Uniform::new("corner_radius", self.radius),
                Uniform::new("size", self.size),
            ],
        );
        let result = RenderElement::<GlesRenderer>::draw(
            &self.inner,
            frame,
            src,
            dst,
            damage,
            opaque_regions,
        );
        frame.clear_tex_program_override();
        result
    }

    fn underlying_storage(&self, _renderer: &mut GlesRenderer) -> Option<UnderlyingStorage<'_>> {
        // Never hand the masked texture to a DRM plane: direct scan-out would
        // bypass the shader and show square corners.
        None
    }
}
