// Fog-of-war overlay shader — photorealistic variant.
//
// Tri-state R8Unorm texture:
//   0   = unexplored / hidden   → deep dark fog
//   128 = explored, not visible → translucent haze
//   255 = currently visible     → clear
//
// Techniques: 5-octave FBM noise, Gaussian-weighted blur, per-band coloring
// with hue shift, wispy boundary distortion, volumetric density falloff.

#import bevy_sprite::mesh2d_vertex_output::VertexOutput

@group(2) @binding(0) var fog_tex: texture_2d<f32>;
@group(2) @binding(1) var fog_sampler: sampler;

// ─── Noise primitives ────────────────────────────────────────────────

fn hash21(p: vec2<f32>) -> f32 {
    var p3 = fract(vec3<f32>(p.x, p.y, p.x) * 0.1031);
    p3 = p3 + dot(p3, vec3<f32>(p3.y + 33.33, p3.z + 33.33, p3.x + 33.33));
    return fract((p3.x + p3.y) * p3.z);
}

fn hash22(p: vec2<f32>) -> vec2<f32> {
    let n = vec2<f32>(
        dot(p, vec2<f32>(127.1, 311.7)),
        dot(p, vec2<f32>(269.5, 183.3))
    );
    return fract(sin(n) * 43758.5453);
}

// Value noise with smooth hermite interpolation.
fn vnoise(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);

    let a = hash21(i + vec2<f32>(0.0, 0.0));
    let b = hash21(i + vec2<f32>(1.0, 0.0));
    let c = hash21(i + vec2<f32>(0.0, 1.0));
    let d = hash21(i + vec2<f32>(1.0, 1.0));

    return mix(mix(a, b, u.x), mix(c, d, u.x), u.y);
}

// 5-octave fractional Brownian motion for organic cloud shapes.
fn fbm5(p_in: vec2<f32>) -> f32 {
    var p = p_in;
    var v = 0.0;
    var a = 0.5;
    let rot = mat2x2<f32>(0.8, 0.6, -0.6, 0.8); // domain rotation to break grid
    for (var i = 0; i < 5; i = i + 1) {
        v = v + a * vnoise(p);
        p = rot * p * 2.0;
        a = a * 0.5;
    }
    return v;
}

// ─── Gaussian-weighted visibility blur ───────────────────────────────

fn blurred_vis(uv: vec2<f32>, texel: vec2<f32>) -> f32 {
    // 5×5 Gaussian kernel (sigma ≈ 1.0), weights from standard table.
    // Using separable-style sampling on a 2D kernel for quality.
    let w0 = 0.0625;  // corner
    let w1 = 0.125;   // edge
    let w2 = 0.25;    // center (extra weight)

    // Hand-unrolled 3-ring weighted sample (13-tap pattern for perf).
    var total = 0.0;
    var wsum  = 0.0;

    // Center
    total = total + textureSample(fog_tex, fog_sampler, uv).r * 4.0;
    wsum  = wsum + 4.0;

    // 4 axis neighbours (distance 1)
    let offsets4 = array<vec2<f32>, 4>(
        vec2<f32>( 1.0,  0.0), vec2<f32>(-1.0,  0.0),
        vec2<f32>( 0.0,  1.0), vec2<f32>( 0.0, -1.0)
    );
    for (var i = 0; i < 4; i = i + 1) {
        total = total + textureSample(fog_tex, fog_sampler, uv + offsets4[i] * texel).r * 2.0;
        wsum  = wsum + 2.0;
    }

    // 4 diagonal neighbours (distance √2)
    let offsets4d = array<vec2<f32>, 4>(
        vec2<f32>( 1.0,  1.0), vec2<f32>(-1.0,  1.0),
        vec2<f32>( 1.0, -1.0), vec2<f32>(-1.0, -1.0)
    );
    for (var i = 0; i < 4; i = i + 1) {
        total = total + textureSample(fog_tex, fog_sampler, uv + offsets4d[i] * texel).r * 1.0;
        wsum  = wsum + 1.0;
    }

    // 4 extended axis (distance 2) for wider penumbra
    for (var i = 0; i < 4; i = i + 1) {
        total = total + textureSample(fog_tex, fog_sampler, uv + offsets4[i] * texel * 2.0).r * 0.5;
        wsum  = wsum + 0.5;
    }

    return total / wsum;
}

// ─── Fragment ────────────────────────────────────────────────────────

@fragment
fn fragment(mesh: VertexOutput) -> @location(0) vec4<f32> {
    let dims = vec2<f32>(textureDimensions(fog_tex));
    let texel = 1.0 / max(dims, vec2<f32>(1.0, 1.0));

    // Y-flip: tile coordinates are Y-up, texture UVs are Y-down.
    let uv = vec2<f32>(mesh.uv.x, 1.0 - mesh.uv.y);

    // ── Wispy boundary distortion ──
    // Offset the UV lookup with FBM noise so fog edges curl organically
    // instead of following the hard pixel grid.
    let world_p = uv * dims;
    let warp_strength = 1.8 * texel;  // ~1.8 texels displacement
    let warp = vec2<f32>(
        fbm5(world_p * 0.12 + vec2<f32>(0.0, 3.7)) - 0.5,
        fbm5(world_p * 0.12 + vec2<f32>(7.3, 0.0)) - 0.5
    ) * warp_strength;

    let uv_warped = uv + warp;

    // ── Visibility (Gaussian-blurred, warped) ──
    let vis = blurred_vis(uv_warped, texel);

    // ── Tri-state bands ──
    let unexplored = 1.0 - smoothstep(0.12, 0.40, vis);
    let visible    = smoothstep(0.78, 0.96, vis);
    let explored   = clamp(1.0 - unexplored - visible, 0.0, 1.0);

    // ── FBM cloud texture (creates volumetric density look) ──
    let cloud_large = fbm5(world_p * 0.06);
    let cloud_small = fbm5(world_p * 0.22 + vec2<f32>(50.0, 50.0));
    let cloud = mix(cloud_large, cloud_small, 0.35);

    // ── Per-band alpha ──
    // Unexplored: deep, dense fog with cloud modulation.
    let alpha_unexplored = unexplored * (0.72 + cloud * 0.20);
    // Explored: lighter haze, more cloud variation shows through.
    let alpha_explored = explored * (0.18 + cloud * 0.16);
    // Visible: fully transparent.
    let alpha = clamp(alpha_unexplored + alpha_explored, 0.0, 0.88);

    // Early out for fully visible areas (skip color math).
    if alpha < 0.005 {
        return vec4<f32>(0.0, 0.0, 0.0, 0.0);
    }

    // ── Per-band color with subtle hue shift ──
    // Unexplored: deep midnight blue-black.
    let col_unexplored = vec3<f32>(0.012, 0.018, 0.035);
    // Explored: slightly warmer steel-grey with blue tinge.
    let col_explored = vec3<f32>(0.06, 0.07, 0.10);

    // Blend base tint by band weight.
    var tint = mix(col_explored, col_unexplored, unexplored);

    // ── Cloud-driven color variation ──
    // Lighter wisps in the explored band, darker swirls in unexplored.
    let wisp_highlight = vec3<f32>(0.10, 0.12, 0.18) * cloud_small * explored * 0.5;
    let deep_shadow    = vec3<f32>(0.005, 0.008, 0.02) * (1.0 - cloud_large) * unexplored * 0.4;
    tint = tint + wisp_highlight - deep_shadow;

    // ── Subtle boundary glow ──
    // Thin bright-ish rim where fog meets clear to simulate light scatter.
    let boundary = smoothstep(0.35, 0.55, vis) * (1.0 - smoothstep(0.55, 0.80, vis));
    let glow = vec3<f32>(0.08, 0.10, 0.16) * boundary * 0.35;
    tint = tint + glow;

    // Clamp to valid range.
    tint = clamp(tint, vec3<f32>(0.0), vec3<f32>(1.0));

    return vec4<f32>(tint, alpha);
}
