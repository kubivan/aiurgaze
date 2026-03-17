# Fog of War

The fog of war is rendered as a full-map GPU overlay quad sitting above the tilemap (z = 1000). It reads SC2 visibility data each observation tick and renders atmospheric fog via a WGSL fragment shader.

## Data Flow

```
SC2 Protobuf Observation
  └─ visibility ImageData
       └─ TerrainLayer::from_image_data()
            └─ update_map_from_observation()  [controller.rs]
                 encodes tri-state bytes → FogOfWarData (CPU Vec<u8>)
                 └─ update_fog_texture()  [main.rs]
                      writes to Assets<Image> + invalidates FogOfWarMaterial
                      └─ Bevy re-prepares bind group → GpuImage re-uploaded
                           └─ fog_of_war.wgsl samples the updated texture
```

## Visibility Encoding

SC2 reports three visibility states per map cell. These are encoded into the R8Unorm fog texture as:

| SC2 value | Meaning | Texture byte |
|-----------|---------|--------------|
| 0 | Never seen | 0 |
| 1 | Explored, not currently visible | 128 |
| 2 | Currently visible | 255 |

The texture starts as all-255 (fully visible) so the map appears clear before the first observation arrives.

## Bevy Resources

| Resource | Purpose |
|---|---|
| `FogOfWarHandle` | `Handle<Image>` for the R8Unorm fog texture |
| `FogOfWarData` | CPU-side `Vec<u8>` + `dirty` flag written each observation tick |
| `FogMaterialHandle` | `Handle<FogOfWarMaterial>` used to invalidate the bind group after texture updates |

## Systems

| System | When | What it does |
|---|---|---|
| `map_init_system` | Once on `GameInfoEvent` | Creates the fog texture, inserts `FogOfWarHandle` |
| `spawn_fog_overlay` | Once when `FogOfWarHandle` is added | Spawns the quad mesh + `FogOfWarMaterial`, stores `FogMaterialHandle` |
| `update_fog_texture` | Every frame, after `response_controller_system` | Flushes `FogOfWarData` → `Image`; calls `materials.get_mut()` to force bind group rebuild |
| `update_fog_uniforms` | Every frame when `FogMaterialHandle` exists | Updates time and camera position in the uniform block |

**Why `materials.get_mut()` is required after texture upload:** Bevy re-creates the `GpuImage` when an `Image` asset is modified, but the `Material2d` bind group — which holds the GPU texture handle the shader actually samples — is only rebuilt when the *material* asset is also marked modified. Without touching the material, the shader keeps sampling the original texture forever.

## Shader (`fog_of_war.wgsl`)

The fragment shader is responsible for all visual rendering. It receives the R8Unorm texture and a uniform block (`FogUniforms`) containing camera position, elapsed time, light direction, and the quad's world-space size and origin.

### UV Mapping

World position is converted to fog texture UV by subtracting the quad's world-space minimum corner and dividing by its size. UVs are clamped to texel centers to avoid edge bleeding.

### Visibility Blur

Before any band decomposition, the raw visibility value is softened by a weighted 17-tap sample (`blurred_vis`): center (weight 4), 4 axis neighbours at 1 texel (weight 2 each), 4 diagonals (weight 1 each), 4 extended axis at 2 texels (weight 0.5 each). This turns the hard per-cell grid boundary into a smooth penumbra.

A small FBM warp is also applied to the UV before sampling, making fog edges curl organically rather than following the pixel grid.

### Band Decomposition

The blurred visibility value is split into three soft bands via smoothstep:

- **unexplored** — `1 - smoothstep(0.10, 0.38, vis)`
- **visible** — `smoothstep(0.76, 0.95, vis)`
- **explored** — `saturate(1 - unexplored - visible)`

### Noise and Density (unexplored band only)

Procedural fog density is built from several layered noise terms, all anchored in world space so they stick to terrain rather than the camera:

- **FBM value noise** (`fbm4`, 4 octaves with domain rotation) — base density
- **Voronoi** (F1+F2 blend) — adds cellular cloud-body highlights
- **Cloud mass** (low-frequency FBM) — large-scale density modulation
- **Billowy puffs** (inverted ridge FBM at two scales, blended) — soft puff shaping
- **Macro density** (very low-frequency value noise) — slow large-area breathing
- **Detail** (high-frequency FBM) — fine wisps added to tint
- **Micro** (small-scale value noise) — tiny surface texture to prevent flatness

All noise layers are time-animated via a `drift` vector (`t * 0.15, t * 0.08`).

### Alpha Computation

- **Unexplored**: base alpha `0.70 + density * 0.26`, modulated by macro density, camera depth fade (`1 - exp(-dist * 0.01)`), height fade (world Y), and fog presence factor. Micro adds a small additive contribution. Maximum clamped to 0.92.
- **Explored**: flat `0.20` — static darkening with no animation.
- **Visible**: 0 — no overlay.

Final alpha is passed through a Beer-Lambert transmittance approximation: `final_alpha = 1 - exp(-alpha * 1.55)`.

### Color

- **Unexplored**: deep desaturated blue-black `(0.008, 0.012, 0.028)`.
- **Explored**: warmer steel-blue `(0.045, 0.055, 0.085)`.

Additional tint contributions (unexplored only):
- Voronoi highlights add subtle cloud luminosity.
- FBM dark swirls deepen density troughs.
- Fine detail wisps add blue-grey wisps.
- Directional light wrap tinting and a forward-scatter phase term simulate light scattering at fog volume boundaries.
- A boundary glow is added at the vis transition edge where fog meets clear.

The final RGB is a mix of the tint with a cool blue-white `(0.85, 0.9, 1.0)` at 10%, multiplied by `final_alpha` for straight-alpha output.

## Diagnostic Logging

One-time `eprintln!` messages are emitted to stderr on startup to confirm the pipeline is working:

```
[fog] visibility format: bpp=8, bytes=35200, size=176x200
[fog] first observation received. visibility layer present: true
[fog] first fog write: 176x200, unexplored=28345, explored=0, visible=6855, other=0, total=35200
[fog] spawned fog overlay: 2816x3200 px, texture 176x200
```

If `visibility layer present: false`, the bot's `JoinGameRequest` is missing the raw interface option.
