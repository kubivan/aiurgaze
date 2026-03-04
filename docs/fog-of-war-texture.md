t a# Fog of War Texture Overlay — Implementation Notes

## Goal

Replace the old per-tile color-darkening fog of war with a GPU shader-based overlay using Bevy's `Material2d` system. The overlay is a single full-map quad with an R8Unorm texture that is updated every observation tick from the SC2 visibility layer.

## Architecture

```
SC2 Protobuf Observation
  └─ visibility ImageData (1bpp or 8bpp)
       └─ TerrainLayer::from_image_data()
            └─ update_map_from_observation() writes binary 0/255 into FogOfWarData
                 └─ update_fog_texture() flushes to Assets<Image> + invalidates material
                      └─ Bevy extract/prepare pipeline re-uploads GpuImage
                           └─ WGSL shader samples updated texture
```

### Key Resources

| Resource | Purpose |
|---|---|
| `FogOfWarHandle` | `Handle<Image>` for the R8Unorm fog texture |
| `FogOfWarData` | CPU-side `Vec<u8>` + `dirty` flag, written each observation tick |
| `FogMaterialHandle` | `Handle<FogOfWarMaterial>` — needed to invalidate the material bind group |

### Key Systems

| System | Schedule | Description |
|---|---|---|
| `map_init_system` | `Update` (once) | Creates fog texture via `create_fog_of_war_texture()`, inserts `FogOfWarHandle` |
| `spawn_fog_overlay` | `Update` (once, on `resource_added::<FogOfWarHandle>`) | Spawns the mesh quad + material, stores `FogMaterialHandle` |
| `update_fog_texture` | `Update` (every frame, after `response_controller_system`) | Flushes `FogOfWarData` → `Image` → invalidates material |

### Files Changed

| File | Changes |
|---|---|
| `src/fog_material.rs` | New file — `FogOfWarMaterial` with `AsBindGroup`, `Material2d`, `AlphaMode2d::Blend` |
| `src/controller.rs` | `FogOfWarHandle`, `FogOfWarData`, `FogMaterialHandle` resources; fog texture creation; visibility→fog writes in `update_map_from_observation` |
| `src/main.rs` | `spawn_fog_overlay`, `update_fog_texture` systems; `Material2dPlugin` registration |
| `assets/shaders/fog_of_war.wgsl` | WGSL fragment shader with 3×3 blur, smoothstep, noise |

## Problems Encountered & Solutions

### 1. Bevy 0.17 API Changes

Many import paths changed from 0.16:

- `Material2d`, `Material2dPlugin`, `MeshMaterial2d`, `AlphaMode2d` → `bevy::sprite_render`
- `ShaderRef` → `bevy::shader`
- `RenderAssetUsages` → `bevy::asset`
- `Mesh2d` → `bevy::mesh`
- `AsBindGroup` stays in `bevy::render::render_resource`

### 2. Shader Bind Group Index

`#{MATERIAL_BIND_GROUP}` is a preprocessor macro that only works in Bevy's internal shaders loaded via `load_shader_library!`. External `.wgsl` files must hardcode `@group(2)` for Material2d bind groups.

### 3. AlphaMode Defaults to Opaque

Without `fn alpha_mode() -> AlphaMode2d { AlphaMode2d::Blend }`, the overlay quad is fully opaque even if the shader outputs `alpha < 1.0`. The material is routed to the opaque pass, which ignores alpha.

### 4. Fog Quad Misaligned

The tilemap uses `TilemapAnchor::Center` at `Transform(0, 0, 0)`, meaning its center is at world origin. The fog overlay quad must also be centered at origin: `Transform::from_xyz(0.0, 0.0, 1.0)` (z=1 to render above tiles).

### 5. Fog Texture Never Updating (Root Cause)

**This was the critical bug.** When `update_fog_texture` modified the `Image` asset via `images.get_mut()`, Bevy re-created the `GpuImage` on the render world. However, the `FogOfWarMaterial`'s bind group — which holds the actual GPU texture/sampler references the shader samples — was **never invalidated**. The material was prepared once at spawn, so the shader kept sampling the original all-255 texture forever.

**Fix:** After updating the `Image`, also call `materials.get_mut(&mat_handle.handle)` on the `FogOfWarMaterial` asset. This marks it as `AssetEvent::Modified`, causing Bevy to re-extract and re-prepare the material, which calls `as_bind_group()` again and picks up the new `GpuImage`.

```rust
// Invalidate the material so its bind group is rebuilt with the new GpuImage.
if let Some(ref mat_handle) = mat_handle {
    let _ = materials.get_mut(&mat_handle.handle);
}
```

### 6. Initial Texture Value

Initial fog texture must be `vec![255u8; size]` (fully visible), not `vec![0u8; size]` (fully fogged). Otherwise, before the first observation arrives, the entire map is covered by fog permanently if no visibility data ever comes.

## Shader Details (`fog_of_war.wgsl`)

- **Input**: R8Unorm texture where 0 = fogged, 255 = visible
- **3×3 box blur**: Softens the binary edges of visibility boundaries
- **Smoothstep**: `fog = 1.0 - smoothstep(0.22, 0.78, vis)` creates a gradual transition
- **Hash noise**: Subtle per-pixel noise to avoid flat uniform look
- **Tint**: Dark blue-grey `vec3(0.03, 0.035, 0.045)` for production; orange `vec3(1.0, 0.45, 0.0)` for debug

Debug mode (current state — revert for production):
```wgsl
let tint = vec3<f32>(1.0, 0.45, 0.0);  // orange debug
let alpha = clamp(0.25 + fog * (0.90 + n * 0.10), 0.0, 1.0);  // high alpha debug
```

Production values:
```wgsl
let tint = vec3<f32>(0.03, 0.035, 0.045);
let alpha = clamp(fog * (0.55 + n * 0.25), 0.0, 0.9);
```

## Diagnostic Logging

One-time logs on stderr to verify the pipeline:

```
[fog] visibility format: bpp=8, bytes=35200, size=176x200
[fog] first observation received. visibility layer present: true
[fog] first fog write: 176x200, fogged=28345, visible=6855, total=35200
[fog] spawned fog overlay: 2816x3200 px, texture 176x200
```

If `visibility layer present: false`, the bot's `JoinGameRequest` is missing the raw interface option.
