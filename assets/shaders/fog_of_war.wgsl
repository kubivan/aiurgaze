// ═══════════════════════════════════════════════════════════════════════
// Fog-of-war overlay — RTS-quality atmospheric fog shader
// ═══════════════════════════════════════════════════════════════════════
//
// Tri-state R8Unorm visibility texture:
//   0   = unexplored / hidden   → deep volumetric fog
//   128 = explored, not visible → translucent atmospheric haze
//   255 = currently visible     → clear
//
// Features:
//   • World-space noise (sticks to terrain, not camera)
//   • Camera depth fade (zoom-dependent density)
//   • Height-based falloff (simulated via world Y)
//   • Directional light tinting at fog boundaries
//   • Layered FBM + Voronoi noise (GPU-Fog-Particles inspired)
//   • Subtle time-animated noise for organic life
//   • Gaussian blurred visibility edges (soft penumbra)
//   • Premultiplied-safe straight-alpha output

#import bevy_sprite::mesh2d_vertex_output::VertexOutput

// ─── Bindings ────────────────────────────────────────────────────────

struct FogUniforms {
    camera_pos: vec3<f32>,      // xy = world pan, z = ortho scale
    time: f32,                  // seconds elapsed
    light_dir: vec3<f32>,       // normalized directional light (world space)
    _pad0: f32,
    world_size: vec2<f32>,      // fog quad size in world units
    world_origin: vec2<f32>,    // fog quad center in world space
};

@group(2) @binding(0) var<uniform> fog: FogUniforms;
@group(2) @binding(1) var fog_tex: texture_2d<f32>;
@group(2) @binding(2) var fog_sampler: sampler;

// ─── Hash helpers ────────────────────────────────────────────────────

fn hash21(p: vec2<f32>) -> f32 {
    var p3 = fract(vec3<f32>(p.x, p.y, p.x) * 0.1031);
    p3 += dot(p3, vec3<f32>(p3.y + 33.33, p3.z + 33.33, p3.x + 33.33));
    return fract((p3.x + p3.y) * p3.z);
}

fn hash22(p: vec2<f32>) -> vec2<f32> {
    let q = vec2<f32>(
        dot(p, vec2<f32>(127.1, 311.7)),
        dot(p, vec2<f32>(269.5, 183.3))
    );
    return fract(sin(q) * 43758.5453);
}

// ─── Value noise (hermite-interpolated) ──────────────────────────────

fn vnoise(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    let a = hash21(i);
    let b = hash21(i + vec2<f32>(1.0, 0.0));
    let c = hash21(i + vec2<f32>(0.0, 1.0));
    let d = hash21(i + vec2<f32>(1.0, 1.0));
    return mix(mix(a, b, u.x), mix(c, d, u.x), u.y);
}

// ─── Voronoi cellular noise (F1+F2 blend) ────────────────────────────

fn voronoi(p: vec2<f32>) -> f32 {
    let n = floor(p);
    let f = fract(p);
    var F1 = 8.0;
    var F2 = 8.0;
    for (var j = -1; j <= 1; j++) {
        for (var i = -1; i <= 1; i++) {
            let g = vec2<f32>(f32(i), f32(j));
            let o = hash22(n + g);
            let r = f - g - o;
            let d = 0.5 * dot(r, r);
            if d < F1 { F2 = F1; F1 = d; }
            else if d < F2 { F2 = d; }
        }
    }
    return (F2 + F1) * 0.5;
}

// ─── FBM (4 octaves, domain rotation to break grid) ─────────────────

fn fbm4(p_in: vec2<f32>) -> f32 {
    var p = p_in;
    var v = 0.0;
    var a = 0.5;
    let rot = mat2x2<f32>(0.8, 0.6, -0.6, 0.8);
    for (var i = 0; i < 4; i++) {
        v += a * vnoise(p);
        p = rot * p * 2.0;
        a *= 0.5;
    }
    return v;
}

// ─── Noise remap: [threshold,1] → [0,1] ─────────────────────────────

fn remap01(v: f32, lo: f32) -> f32 {
    return saturate((v - lo) / (1.0 - lo + 0.001));
}

// ─── Gaussian-weighted visibility blur (17-tap, 3-ring) ─────────────
//
// Blurs the hard per-cell visibility into soft penumbra.
// Fixed tap count — no dynamic loops.

fn blurred_vis(uv: vec2<f32>, texel: vec2<f32>) -> f32 {
    var total = textureSample(fog_tex, fog_sampler, uv).r * 4.0;
    var wsum = 4.0;

    // Ring 1: 4 axis (distance 1 texel)
    let ax = array<vec2<f32>, 4>(
        vec2(1.0, 0.0), vec2(-1.0, 0.0),
        vec2(0.0, 1.0), vec2(0.0, -1.0)
    );
    for (var i = 0; i < 4; i++) {
        total += textureSample(fog_tex, fog_sampler, uv + ax[i] * texel).r * 2.0;
        wsum += 2.0;
    }

    // Ring 1: 4 diagonal (distance √2)
    let dg = array<vec2<f32>, 4>(
        vec2(1.0, 1.0), vec2(-1.0, 1.0),
        vec2(1.0, -1.0), vec2(-1.0, -1.0)
    );
    for (var i = 0; i < 4; i++) {
        total += textureSample(fog_tex, fog_sampler, uv + dg[i] * texel).r;
        wsum += 1.0;
    }

    // Ring 2: 4 extended axis (distance 2 texels)
    for (var i = 0; i < 4; i++) {
        total += textureSample(fog_tex, fog_sampler, uv + ax[i] * texel * 2.0).r * 0.5;
        wsum += 0.5;
    }

    return total / wsum;
}

// ─── Fragment ────────────────────────────────────────────────────────

@fragment
fn fragment(mesh: VertexOutput) -> @location(0) vec4<f32> {
    // ── World-space position from vertex output ──
    // Bevy's VertexOutput provides world_position at @location(0).
    let world_pos = mesh.world_position.xy;

    // ── Convert world position → fog texture UV ──
    // The fog quad is centered at world_origin with extent world_size.
    let uv_raw = (world_pos - fog.world_origin + fog.world_size * 0.5) / fog.world_size;
    // Y-flip: tile coordinates are Y-up, texture UVs are Y-down.
    let uv = vec2<f32>(uv_raw.x, 1.0 - uv_raw.y);

    let dims = vec2<f32>(textureDimensions(fog_tex));
    let texel = 1.0 / max(dims, vec2<f32>(1.0));

    // ── World-space noise coordinate (sticks to terrain) ──
    // Use actual world position so noise doesn't shift with camera.
    // Scale into a nice noise-tile range (~0.01-0.05 per world unit).
    let wp = world_pos * 0.03;
    let t = fog.time;

    // ── UV warp (organic boundary distortion) ──
    // Small FBM warp displaces the visibility lookup so edges
    // curl organically instead of following the hard pixel grid.
    let warp_strength = 2.5 * texel;
    let warp = vec2<f32>(
        fbm4(wp * 3.5 + vec2<f32>(0.0, 4.7) + vec2<f32>(t * 0.008, 0.0)) - 0.5,
        fbm4(wp * 3.5 + vec2<f32>(8.3, 0.0) + vec2<f32>(0.0, t * 0.006)) - 0.5
    ) * warp_strength;
    let uv_w = clamp(uv + warp, vec2<f32>(0.0), vec2<f32>(1.0));

    // ── Blurred visibility (soft penumbra) ──
    let vis = blurred_vis(uv_w, texel);

    // ── Tri-state band decomposition ──
    let unexplored = 1.0 - smoothstep(0.10, 0.38, vis);
    let visible    = smoothstep(0.76, 0.95, vis);
    let explored   = saturate(1.0 - unexplored - visible);

    // Early-out: fully visible area → zero fog.
    let fog_mask = unexplored + explored;
    if fog_mask < 0.003 {
        return vec4<f32>(0.0, 0.0, 0.0, 0.0);
    }

    // ── Layered noise (value × voronoi, GPU-Fog-Particles style) ──
    // Time offset gives subtle animated drift.
    let drift = vec2<f32>(t * 0.012, t * -0.008);

    let value_raw = fbm4(wp * 1.5 + drift);
    let value_n = remap01(value_raw, 0.50);
    let value_blend = mix(1.0, value_n, 0.80);

    let voronoi_raw = voronoi(wp * 1.0 + drift * 0.5);
    let voronoi_n = remap01(voronoi_raw, 0.20);
    let voronoi_blend = mix(1.0, voronoi_n, 0.60);

    let density = saturate(value_blend * voronoi_blend);

    // Fine detail noise for micro-texture (faster animation).
    let detail = fbm4(wp * 9.0 + vec2<f32>(50.0, 50.0) + drift * 2.0);

    // ── Camera depth fade ──
    // ortho.scale lives in camera_pos.z. Higher scale = more zoomed out → denser fog.
    let cam_scale = max(fog.camera_pos.z, 0.5);
    // Normalize: scale=1 → factor 1.0, scale=3 → factor ~1.15 (subtle).
    let depth_fade = mix(0.92, 1.08, saturate((cam_scale - 0.5) / 4.0));

    // ── Height-based falloff ──
    // In top-down 2D, simulate height as world Y position.
    // Fog is slightly denser at bottom of map (lower Y), thinner at top.
    let height_norm = saturate(uv_raw.y); // 0 at bottom, 1 at top
    let height_fade = mix(1.05, 0.92, height_norm);

    // ── Per-band alpha ──
    let base_unexplored = 0.72 + density * 0.20;
    let base_explored   = 0.16 + density * 0.20 + detail * 0.06;

    let alpha_unexplored = unexplored * base_unexplored * depth_fade * height_fade;
    let alpha_explored   = explored * base_explored * depth_fade;
    var alpha = clamp(alpha_unexplored + alpha_explored, 0.0, 0.90);

    // ── Per-band color ──
    // Deep desaturated blue-black for unexplored.
    let col_unexplored = vec3<f32>(0.008, 0.012, 0.028);
    // Warmer steel-blue for explored haze.
    let col_explored   = vec3<f32>(0.045, 0.055, 0.085);

    var tint = mix(col_explored, col_unexplored, unexplored);

    // ── Atmospheric color variation from noise ──
    // Voronoi cellular highlights (explored band): cloud-like luminosity.
    tint += vec3<f32>(0.08, 0.10, 0.15) * voronoi_n * explored * 0.35;
    // Dark swirl deepening (unexplored band).
    tint -= vec3<f32>(0.003, 0.005, 0.012) * (1.0 - density) * unexplored * 0.45;
    // Fine detail wisps.
    tint += vec3<f32>(0.05, 0.06, 0.09) * detail * fog_mask * 0.22;

    // ── Directional light tinting ──
    // Simulate faint warm highlight on fog facing the light.
    // In 2D: dot(light_dir.xy, normalize(world_pos - camera_pos.xy)) gives
    // a directional gradient across the visible fog.
    let to_frag = normalize(world_pos - fog.camera_pos.xy + vec2<f32>(0.001));
    let light_dot = dot(fog.light_dir.xy, to_frag) * 0.5 + 0.5; // [0,1]
    let light_tint = vec3<f32>(0.12, 0.10, 0.06) * light_dot * fog_mask * 0.18;
    tint += light_tint;

    // ── Boundary glow (light scatter at fog edge) ──
    let edge = smoothstep(0.32, 0.52, vis) * (1.0 - smoothstep(0.52, 0.78, vis));
    let glow_color = vec3<f32>(0.08, 0.10, 0.16) + light_tint * 0.5;
    tint += glow_color * edge * 0.38;

    // Clamp tint to valid range.
    tint = clamp(tint, vec3<f32>(0.0), vec3<f32>(1.0));

    // ── Output: straight alpha (compatible with Bevy's Blend mode) ──
    return vec4<f32>(tint, alpha);
}
