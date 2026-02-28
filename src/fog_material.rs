use bevy::prelude::*;
use bevy::reflect::TypePath;
use bevy::render::render_resource::AsBindGroup;
use bevy::shader::ShaderRef;
use bevy::sprite_render::{AlphaMode2d, Material2d};

/// Material for the fog-of-war overlay quad.
#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
pub struct FogOfWarMaterial {
    #[texture(0)]
    #[sampler(1)]
    pub fog_texture: Handle<Image>,
}

impl Material2d for FogOfWarMaterial {
    fn fragment_shader() -> ShaderRef {
        "shaders/fog_of_war.wgsl".into()
    }

    fn alpha_mode(&self) -> AlphaMode2d {
        AlphaMode2d::Blend
    }
}
