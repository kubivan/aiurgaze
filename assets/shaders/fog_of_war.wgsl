// Fog-of-war overlay shader — photorealistic variant.
//
// Tri-state R8Unorm texture:
//   0   = unexplored / hidden   → deep volumetric fog
//   128 = explored, not visible → translucent atmospheric haze
//   255 = currently visible     → clear
//
// Inspired by MirzaBeig/GPU-Fog-Particles (Unlicense):
//   • Three noise types multiplied: value × simplex × Voronoi
//   • Per-noise remap for contrast control
//   • Voronoi cellular structure for cloud-like organic shapes
//   • Gaussian visibility blur + UV warp for wispy boundaries

#import bevy_sprite::mesh2d_vertex_output::VertexOutput

@group(2) @binding(0) var fog_tex: texture_2d<f32>;
@group(2) @binding(1) var fog_sampler: sampler;

// ─── Hash helpers ────────────────────────────────────────────────────

fn hash21(p: vec2<f32>) -> f32 {
    var p3 = fract(vec3<f32>(p.x, p.y, p.x) * 0.1031);
    p3 = p3 + dot(p3, vec3<f32>(p3.y + 33.33, p3.z + 33.33, p3.x + 33.33));
    return fract((p3.x + p3.y) * p3.z);
}

fn hash22(p: vec2<f32>) -> vec2<f32> {
    var q = vec2<f32>(
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

// ─── 3D Simplex noise (ported from Ashima/webgl-noise) ──────────────

fn mod289_3(x: vec3<f32>) -> vec3<f32> { return x - floor(x / 289.0) * 289.0; }
fn mod289_4(x: vec4<f32>) -> vec4<f32> { return x - floor(x / 289.0) * 289.0; }
fn permute4(x: vec4<f32>) -> vec4<f32> { return mod289_4((x * 34.0 + 1.0) * x); }
fn taylorInvSqrt4(r: vec4<f32>) -> vec4<f32> { return 1.79284291400159 - r * 0.85373472095314; }

fn snoise3(v: vec3<f32>) -> f32 {
    let C = vec2<f32>(1.0 / 6.0, 1.0 / 3.0);
    let i = floor(v + dot(v, vec3<f32>(C.y, C.y, C.y)));
    let x0 = v - i + dot(i, vec3<f32>(C.x, C.x, C.x));

    let g = step(x0.yzx, x0.xyz);
    let l = 1.0 - g;
    let i1 = min(g, l.zxy);
    let i2 = max(g, l.zxy);

    let x1 = x0 - i1 + vec3<f32>(C.x, C.x, C.x);
    let x2 = x0 - i2 + vec3<f32>(C.y, C.y, C.y);
    let x3 = x0 - 0.5;

    let ii = mod289_3(i);
    let p = permute4(permute4(permute4(
        ii.z + vec4<f32>(0.0, i1.z, i2.z, 1.0)) +
        ii.y + vec4<f32>(0.0, i1.y, i2.y, 1.0)) +
        ii.x + vec4<f32>(0.0, i1.x, i2.x, 1.0));

    let j = p - 49.0 * floor(p / 49.0);
    let x_ = floor(j / 7.0);
    let y_ = floor(j - 7.0 * x_);
    let x = (x_ * 2.0 + 0.5) / 7.0 - 1.0;
    let y = (y_ * 2.0 + 0.5) / 7.0 - 1.0;
    let h = 1.0 - abs(x) - abs(y);

    let b0 = vec4<f32>(x.x, x.y, y.x, y.y);
    let b1 = vec4<f32>(x.z, x.w, y.z, y.w);
    let s0 = floor(b0) * 2.0 + 1.0;
    let s1 = floor(b1) * 2.0 + 1.0;
    let sh = -step(h, vec4<f32>(0.0));

    let a0 = vec4<f32>(b0.x, b0.z, b0.y, b0.w) + vec4<f32>(s0.x, s0.z, s0.y, s0.w) * vec4<f32>(sh.x, sh.x, sh.y, sh.y);
    let a1 = vec4<f32>(b1.x, b1.z, b1.y, b1.w) + vec4<f32>(s1.x, s1.z, s1.y, s1.w) * vec4<f32>(sh.z, sh.z, sh.w, sh.w);

    var g0 = vec3<f32>(a0.x, a0.y, h.x);
    var g1 = vec3<f32>(a0.z, a0.w, h.y);
    var g2 = vec3<f32>(a1.x, a1.y, h.z);
    var g3 = vec3<f32>(a1.z, a1.w, h.w);

    let norm = taylorInvSqrt4(vec4<f32>(dot(g0, g0), dot(g1, g1), dot(g2, g2), dot(g3, g3)));
    g0 = g0 * norm.x;
    g1 = g1 * norm.y;
    g2 = g2 * norm.z;
    g3 = g3 * norm.w;

    var m = max(vec4<f32>(0.6) - vec4<f32>(dot(x0, x0), dot(x1, x1), dot(x2, x2), dot(x3, x3)), vec4<f32>(0.0));
    m = m * m;
    m = m * m;

    return 42.0 * dot(m, vec4<f32>(dot(x0, g0), dot(x1, g1), dot(x2, g2), dot(x3, g3)));
}

// ─── Voronoi cellular noise ──────────────────────────────────────────

fn voronoi(p: vec2<f32>) -> f32 {
    let n = floor(p);
    let f = fract(p);
    var F1 = 8.0;
    var F2 = 8.0;

    for (var j = -1; j <= 1; j = j + 1) {
        for (var i = -1; i <= 1; i = i + 1) {
            let g = vec2<f32>(f32(i), f32(j));
            let o = hash22(n + g);
            let r = f - g - o;
            let d = 0.5 * dot(r, r);
            if d < F1 {
                F2 = F1;
                F1 = d;
            } else if d < F2 {
                F2 = d;
            }
        }
    }
    return (F2 + F1) * 0.5;
}

// ─── FBM with domain rotation ────────────────────────────────────────

fn fbm4(p_in: vec2<f32>) -> f32 {
    var p = p_in;
    var v = 0.0;
    var a = 0.5;
    let rot = mat2x2<f32>(0.8, 0.6, -0.6, 0.8);
    for (var i = 0; i < 4; i = i + 1) {
        v = v + a * vnoise(p);
        p = rot * p * 2.0;
        a = a * 0.5;
    }
    return v;
}

// ─── Noise remap (GPU-Fog-Particles style) ───────────────────────────
// Remap from [threshold, 1] → [0, 1], saturate. Higher threshold = more contrast.
fn remap_noise(v: f32, threshold: f32) -> f32 {
    return saturate((v - threshold) / (1.0 - threshold + 0.001));
}

// ─── Gaussian-weighted visibility blur (17-tap, 3-ring) ─────────────

fn blurred_vis(uv: vec2<f32>, texel: vec2<f32>) -> f32 {
    var total = 0.0;
    var wsum = 0.0;

    // Center
    total = total + textureSample(fog_tex, fog_sampler, uv).r * 4.0;
    wsum = wsum + 4.0;

    // 4 axis neighbours (distance 1)
    let ax = array<vec2<f32>, 4>(
        vec2<f32>(1.0, 0.0), vec2<f32>(-1.0, 0.0),
        vec2<f32>(0.0, 1.0), vec2<f32>(0.0, -1.0)
    );
    for (var i = 0; i < 4; i = i + 1) {
        total = total + textureSample(fog_tex, fog_sampler, uv + ax[i] * texel).r * 2.0;
        wsum = wsum + 2.0;
    }

    // 4 diagonal neighbours
    let dg = array<vec2<f32>, 4>(
        vec2<f32>(1.0, 1.0), vec2<f32>(-1.0, 1.0),
        vec2<f32>(1.0, -1.0), vec2<f32>(-1.0, -1.0)
    );
    for (var i = 0; i < 4; i = i + 1) {
        total = total + textureSample(fog_tex, fog_sampler, uv + dg[i] * texel).r;
        wsum = wsum + 1.0;
    }

    // 4 extended axis (distance 2) for wider penumbra
    for (var i = 0; i < 4; i = i + 1) {
        total = total + textureSample(fog_tex, fog_sampler, uv + ax[i] * texel * 2.0).r * 0.5;
        wsum = wsum + 0.5;
    }

    return total / wsum;
}

// ─── Fragment ────────────────────────────────────────────────────────

@fragment
fn fragment(mesh: VertexOutput) -> @location(0) vec4<f32> {
    let dims = vec2<f32>(textureDimensions(fog_tex));
    let texel = 1.0 / max(dims, vec2<f32>(1.0));

    // Y-flip: tile coords Y-up, texture UVs Y-down.
    let uv = vec2<f32>(mesh.uv.x, 1.0 - mesh.uv.y);
    let world_p = uv * dims;

    // ── UV warp (wispy boundary distortion) ──
    let warp_strength = 2.2 * texel;
    let warp = vec2<f32>(
        fbm4(world_p * 0.10 + vec2<f32>(0.0, 4.7)) - 0.5,
        fbm4(world_p * 0.10 + vec2<f32>(8.3, 0.0)) - 0.5
    ) * warp_strength;
    let uv_w = uv + warp;

    // ── Blurred visibility ──
    let vis = blurred_vis(uv_w, texel);

    // ── Tri-state band decomposition ──
    let unexplored = 1.0 - smoothstep(0.10, 0.38, vis);
    let visible    = smoothstep(0.76, 0.95, vis);
    let explored   = clamp(1.0 - unexplored - visible, 0.0, 1.0);

    // ── Combined noise: value × simplex × voronoi (GPU-Fog-Particles style) ──
    // Three independent noise types at different scales, each remapped
    // for contrast, then multiplied for rich organic variation.

    // Tuned to match GPU-Fog-Particles "Fog Large" material:
    //   value: scale=20, amount=0.80, remap=0.50
    //   simplex: amount=0 (disabled in Fog Large preset)
    //   voronoi: scale=5, amount=0.60, remap=0.20
    //   combined remap=0

    let value_raw = fbm4(world_p * 0.045);   // ~20 world-scale equivalent
    let value_noise = remap_noise(value_raw, 0.50);

    let voronoi_raw = voronoi(world_p * 0.030);  // ~5 world-scale
    let voronoi_noise = remap_noise(voronoi_raw, 0.20);

    // Weighted blend: lerp(1, noise, amount) per channel.
    let value_blend   = mix(1.0, value_noise, 0.80);
    let voronoi_blend = mix(1.0, voronoi_noise, 0.60);

    // Multiply value × voronoi → organic cloud density.
    let combined = value_blend * voronoi_blend;
    let density = saturate(combined);   // combined remap = 0 → no threshold

    // Fine detail noise for micro-texture.
    let detail = fbm4(world_p * 0.28 + vec2<f32>(50.0, 50.0));

    // ── Per-band alpha ──
    // Unexplored: thick volumetric fog, modulated by combined noise.
    let alpha_unexplored = unexplored * (0.68 + density * 0.24);
    // Explored: lighter atmospheric haze with more noise variation.
    let alpha_explored = explored * (0.12 + density * 0.22 + detail * 0.06);
    let alpha = clamp(alpha_unexplored + alpha_explored, 0.0, 0.88);

    // Early-out for fully visible.
    if alpha < 0.004 {
        return vec4<f32>(0.0, 0.0, 0.0, 0.0);
    }

    // ── Per-band color ──
    // Deep midnight blue-black for unexplored.
    let col_unexplored = vec3<f32>(0.010, 0.015, 0.032);
    // Warmer steel-blue for explored haze.
    let col_explored = vec3<f32>(0.055, 0.065, 0.095);

    var tint = mix(col_explored, col_unexplored, unexplored);

    // ── Atmospheric color variation from noise ──
    // Voronoi creates cellular highlights in the explored band.
    let cell_highlight = vec3<f32>(0.09, 0.11, 0.16) * voronoi_noise * explored * 0.40;
    // Dark swirls deepen the unexplored band.
    let deep_swirl = vec3<f32>(0.004, 0.006, 0.015) * (1.0 - density) * unexplored * 0.50;
    // Fine detail wisps.
    let wisps = vec3<f32>(0.06, 0.07, 0.10) * detail * (explored + unexplored * 0.3) * 0.25;

    tint = tint + cell_highlight - deep_swirl + wisps;

    // ── Boundary glow (light scatter at fog edge) ──
    let edge = smoothstep(0.32, 0.52, vis) * (1.0 - smoothstep(0.52, 0.78, vis));
    let glow = vec3<f32>(0.07, 0.09, 0.14) * edge * 0.40;
    tint = tint + glow;

    tint = clamp(tint, vec3<f32>(0.0), vec3<f32>(1.0));

    return vec4<f32>(tint, alpha);
}
