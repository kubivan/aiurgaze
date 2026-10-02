#import bevy_pbr::forward_io::VertexOutput

struct FogUniforms {
    camera_pos: vec3<f32>,
    time: f32,
    light_dir: vec3<f32>,
    _pad0: f32,
    world_size: vec2<f32>,
    world_origin: vec2<f32>,
};

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> fog: FogUniforms;
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var fog_tex: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(2) var fog_sampler: sampler;

@fragment
fn fragment(mesh: VertexOutput) -> @location(0) vec4<f32> {
    let map_position = mesh.world_position.xz;
    let map_min = fog.world_origin - fog.world_size * 0.5;
    let uv = (map_position - map_min) / fog.world_size;
    if any(uv < vec2<f32>(0.0)) || any(uv > vec2<f32>(1.0)) {
        return vec4<f32>(0.0);
    }

    let visibility = textureSample(fog_tex, fog_sampler, uv).r;
    let unexplored = 1.0 - smoothstep(0.10, 0.38, visibility);
    let explored = 1.0 - unexplored - smoothstep(0.76, 0.95, visibility);
    let drift = 0.96 + 0.04 * sin(fog.time * 0.35 + map_position.x * 0.015 + map_position.y * 0.012);
    let alpha = clamp(unexplored * 0.82 * drift + explored * 0.20, 0.0, 0.92);
    let color = mix(vec3<f32>(0.045, 0.055, 0.085), vec3<f32>(0.008, 0.012, 0.028), unexplored);
    return vec4<f32>(color, alpha);
}