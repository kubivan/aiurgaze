use bevy::math::{Vec2, Vec3};
use bevy::prelude::*;
use bevy::reflect::TypePath;
use bevy::render::render_resource::{AsBindGroup, ShaderType};
use bevy::shader::ShaderRef;
use bevy::sprite_render::{AlphaMode2d, Material2d};

/// GPU-side uniform block for fog-of-war shader parameters.
///
/// Layout (std140-compatible via `ShaderType`):
///   binding 0: FogUniforms
///   binding 1: fog_texture (R8Unorm)
///   binding 2: fog_sampler
#[derive(Clone, Debug, Default, ShaderType)]
pub struct FogUniforms {
    /// Camera world position (xy = pan, z = ortho scale for depth fade).
    pub camera_pos: Vec3,
    /// Elapsed time in seconds for subtle noise animation.
    pub time: f32,
    /// Directional light direction (normalized, world space). Used for rim tint.
    pub light_dir: Vec3,
    /// Fog overlay world-space size (width, height) in pixels.
    /// Needed to convert world_position → texture UV.
    pub _pad0: f32,
    pub world_size: Vec2,
    /// World-space origin offset of the fog quad (usually 0,0 for centered).
    pub world_origin: Vec2,
}

/// Material for the fog-of-war overlay quad.
///
/// Uses premultiplied alpha blending: fragment outputs `(rgb * a, a)`,
/// blend state is `src_factor=One, dst_factor=OneMinusSrcAlpha`.
#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
pub struct FogOfWarMaterial {
    #[uniform(0)]
    pub uniforms: FogUniforms,
    #[texture(1)]
    #[sampler(2)]
    pub fog_texture: Handle<Image>,
}

impl Material2d for FogOfWarMaterial {
    fn fragment_shader() -> ShaderRef {
        "shaders/fog_of_war.wgsl".into()
    }

    fn alpha_mode(&self) -> AlphaMode2d {
        // Blend mode for premultiplied alpha output.
        // Bevy's AlphaMode2d::Blend uses standard (SrcAlpha, OneMinusSrcAlpha).
        // For true premultiplied we'd need (One, OneMinusSrcAlpha), but Bevy 0.17
        // doesn't expose custom blend states in Material2d. AlphaMode2d::Blend
        // still works correctly if the shader outputs straight alpha (rgb, a),
        // which we do as a fallback-safe path.
        AlphaMode2d::Blend
    }
}
