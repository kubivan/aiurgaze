use crate::app_settings::MapConfig;
use crate::render_layers::{LayerRegistry, RenderLayerKind, RenderLayerMarker, ViewModeVisibility};
use bevy::asset::{Handle, RenderAssetUsages};
use bevy::image::Image;
use bevy::mesh::{Indices, Mesh, PrimitiveTopology};
use bevy::prelude::*;
use bevy_ecs_tilemap::prelude::*;
use sc2_proto::common::ImageData;

#[derive(Clone)]
pub struct TerrainLayer {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>, // raw bytes from ImageData.data - made public for hashing
}
fn unpack_bits(bytes: &[u8]) -> Vec<bool> {
    let mut bits = Vec::with_capacity(bytes.len() * 8);
    for byte in bytes {
        for i in (0..8).rev() {
            bits.push((byte >> (i as u8)) & 1 == 1);
        }
    }
    bits
}
impl TerrainLayer {
    pub fn from_image_data1(data: &[u8], width: u32, height: u32) -> Self {
        // 1 bit per pixel: unpack each bit
        let bits = unpack_bits(data);
        let pixels: Vec<u8> = bits.iter().map(|&b| if b { 255u8 } else { 0 }).collect();
        TerrainLayer {
            width,
            height,
            data: pixels, // now 8 bits per pixel
        }
    }

    pub fn from_image_data(img: &ImageData) -> Self {
        let img_size = img.size.clone().unwrap();
        let width = img_size.x.unwrap() as u32;
        let height = img_size.y.unwrap() as u32;
        let bits = img.bits_per_pixel.unwrap();
        if bits == 1 {
            Self::from_image_data1(img.data.as_ref().unwrap(), width, height)
        } else {
            Self::from_image_data8(img)
        }
    }

    pub fn from_image_data8(img: &ImageData) -> Self {
        let bits = img.bits_per_pixel.unwrap();
        assert_eq!(
            bits, 8,
            "Only 8 bits per pixel supported for now (got {bits})"
        );
        let img_size = img.size.clone().unwrap();
        let width = img_size.x.unwrap() as u32;
        let height = img_size.y.unwrap() as u32;
        assert_eq!(
            img.data.as_ref().unwrap().len(),
            (width * height) as usize,
            "Image data size mismatch"
        );
        Self {
            width,
            height,
            data: img.data.clone().unwrap(),
        }
    }
    pub fn get_value(&self, x: u32, y: u32) -> u8 {
        let idx = (y * self.width + x) as usize;
        if idx < self.data.len() {
            self.data[idx]
        } else {
            0
        }
    }
}
/// Get tile color based on terrain properties - uses MapConfig from entity system
/// Creep overrides all other colors with purple
pub fn blend_tile_color(
    pathing: u8,
    placement: u8,
    creep: u8,
    energy: u8,
    visibility: u8,
    height: u8,
    map_config: &MapConfig,
    layer_registry: &LayerRegistry,
) -> Color {
    if !layer_registry.is_visible(RenderLayerKind::Terrain) {
        return Color::NONE;
    }

    let pathing = if layer_registry.is_visible(RenderLayerKind::Pathing) {
        pathing
    } else {
        255
    };
    let placement = if layer_registry.is_visible(RenderLayerKind::Placement) {
        placement
    } else {
        255
    };
    let height = if layer_registry.is_visible(RenderLayerKind::HeightMap) {
        height
    } else {
        127
    };
    let creep = if layer_registry.is_visible(RenderLayerKind::Creep) {
        creep
    } else {
        0
    };
    let energy = if layer_registry.is_visible(RenderLayerKind::Energy) {
        energy
    } else {
        0
    };

    // Creep overrides everything with purple
    if creep > 0 {
        let color = map_config.get_creep_color();
        return map_config.apply_height_intensity(color, height);
    }
    // Energy overrides with cyan/blue
    if energy > 0 {
        let color = map_config.get_energy_color();
        return map_config.apply_height_intensity(color, height);
    }
    // Get discrete color for pathable/placeable combination
    let mut color = map_config.get_terrain_color(pathing > 0, placement > 0);
    color = map_config.apply_height_intensity(color, height);

    // Apply fog of war: darken unseen tiles (visibility == 0)
    if visibility == 0 {
        let rgba = color.to_srgba();
        color = Color::srgba(
            rgba.red * 0.3,
            rgba.green * 0.3,
            rgba.blue * 0.3,
            rgba.alpha,
        );
    }

    color
}
#[derive(Clone)]
pub struct TerrainLayers {
    pub pathing: TerrainLayer,
    pub placement: TerrainLayer,
    pub height: TerrainLayer,
    pub creep: Option<TerrainLayer>,
    pub energy: Option<TerrainLayer>,
}

impl TerrainLayers {
    pub fn new(pathing: TerrainLayer, placement: TerrainLayer, height: TerrainLayer) -> Self {
        Self {
            pathing,
            placement,
            height,
            creep: None,
            energy: None,
        }
    }

    pub fn get_dimensions(&self) -> (u32, u32) {
        (self.pathing.width, self.pathing.height)
    }
}

#[derive(Resource, Debug, Clone)]
pub struct Terrain3dSettings {
    pub height_scale: f32,
}

impl Default for Terrain3dSettings {
    fn default() -> Self {
        Self { height_scale: 1.0 }
    }
}

#[derive(Resource, Default)]
pub struct Terrain3dMeshDirty(pub bool);

pub fn terrain_height_to_world_y(value: u8, tile_size: f32, height_scale: f32) -> f32 {
    (value as f32 - 128.0) * tile_size / 8.0 * height_scale
}

pub fn map_position_3d(x: f32, y: f32, z: f32, map_size: (u32, u32), tile_size: f32) -> Vec3 {
    Vec3::new(
        x * tile_size - map_size.0 as f32 * tile_size / 2.0,
        z * tile_size,
        y * tile_size - map_size.1 as f32 * tile_size / 2.0,
    )
}

pub fn build_terrain_mesh(
    layers: &TerrainLayers,
    creep: Option<&TerrainLayer>,
    energy: Option<&TerrainLayer>,
    map_config: &MapConfig,
    layer_registry: &LayerRegistry,
    height_scale: f32,
) -> Mesh {
    let (width, height) = layers.get_dimensions();
    let mut positions = Vec::with_capacity((width * height * 4) as usize);
    let mut colors = Vec::with_capacity((width * height * 4) as usize);
    let mut indices = Vec::with_capacity((width * height * 6) as usize);
    let half_width = width as f32 * map_config.tile_size / 2.0;
    let half_height = height as f32 * map_config.tile_size / 2.0;

    for y in 0..height {
        for x in 0..width {
            let pathing = layers.pathing.get_value(x, y);
            let placement = layers.placement.get_value(x, y);
            let terrain_height = layers.height.get_value(x, y);
            let color = blend_tile_color(
                pathing,
                placement,
                creep.map_or(0, |layer| layer.get_value(x, y)),
                energy.map_or(0, |layer| layer.get_value(x, y)),
                1,
                terrain_height,
                map_config,
                layer_registry,
            );
            let rgba = color.to_srgba();
            let vertex_color = [rgba.red, rgba.green, rgba.blue, rgba.alpha];
            let first = positions.len() as u32;
            let x0 = x as f32 * map_config.tile_size - half_width;
            let x1 = (x + 1) as f32 * map_config.tile_size - half_width;
            let z0 = y as f32 * map_config.tile_size - half_height;
            let z1 = (y + 1) as f32 * map_config.tile_size - half_height;
            let y00 = terrain_height_to_world_y(
                layers.height.get_value(x, y),
                map_config.tile_size,
                height_scale,
            );
            let y10 = terrain_height_to_world_y(
                layers.height.get_value((x + 1).min(width - 1), y),
                map_config.tile_size,
                height_scale,
            );
            let y01 = terrain_height_to_world_y(
                layers.height.get_value(x, (y + 1).min(height - 1)),
                map_config.tile_size,
                height_scale,
            );
            let y11 = terrain_height_to_world_y(
                layers
                    .height
                    .get_value((x + 1).min(width - 1), (y + 1).min(height - 1)),
                map_config.tile_size,
                height_scale,
            );

            positions.extend_from_slice(&[
                [x0, y00, z0],
                [x1, y10, z0],
                [x0, y01, z1],
                [x1, y11, z1],
            ]);
            colors.extend_from_slice(&[vertex_color; 4]);
            indices.extend_from_slice(&[
                first,
                first + 2,
                first + 1,
                first + 1,
                first + 2,
                first + 3,
            ]);
        }
    }

    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
    mesh.insert_indices(Indices::U32(indices));
    mesh.compute_normals();
    mesh
}

#[cfg(test)]
mod tests {
    use super::{map_position_3d, terrain_height_to_world_y};
    use bevy::prelude::Vec3;

    #[test]
    fn map_position_centers_horizontal_axes_and_preserves_elevation() {
        assert_eq!(
            map_position_3d(0.0, 0.0, 3.0, (10, 8), 2.0),
            Vec3::new(-10.0, 6.0, -8.0)
        );
    }

    #[test]
    fn height_values_map_to_scaled_vertical_offsets() {
        assert_eq!(terrain_height_to_world_y(128, 16.0, 1.0), 0.0);
        assert_eq!(terrain_height_to_world_y(136, 16.0, 1.0), 16.0);
        assert_eq!(terrain_height_to_world_y(136, 16.0, 2.0), 32.0);
    }
}

pub fn spawn_tilemap(
    commands: &mut Commands,
    layers: &TerrainLayers,
    asset_server: &mut Res<AssetServer>,
    map_config: &MapConfig,
    layer_registry: &LayerRegistry,
    #[cfg(all(not(feature = "atlas"), feature = "render"))] array_texture_loader: Res<
        ArrayTextureLoader,
    >,
) -> TileStorage {
    let texture_handle: Handle<Image> = asset_server.load("tiles.png");
    let (width, height) = layers.get_dimensions();
    let map_size = TilemapSize {
        x: width,
        y: height,
    };
    let tilemap_entity = commands.spawn_empty().id();
    let mut tile_storage = TileStorage::empty(map_size);
    // Fill map tiles with colors from style config
    for y in 0..height {
        for x in 0..width {
            let tile_pos = TilePos { x, y };
            // Get values from each layer
            let pathing = layers.pathing.get_value(x, y);
            let placement = layers.placement.get_value(x, y);
            let height_val = layers.height.get_value(x, y);
            let creep = layers.creep.as_ref().map_or(0, |l| l.get_value(x, y));
            let energy = layers.energy.as_ref().map_or(0, |l| l.get_value(x, y));
            // Get color based on all layers using map config
            // Default visibility to 1 (visible) - will be updated from observation
            let color = blend_tile_color(
                pathing,
                placement,
                creep,
                energy,
                1,
                height_val,
                map_config,
                layer_registry,
            );
            let tile_entity = commands
                .spawn(TileBundle {
                    position: tile_pos,
                    tilemap_id: TilemapId(tilemap_entity),
                    color: TileColor(color),
                    texture_index: TileTextureIndex(5), // Use a single white tile for coloring
                    ..Default::default()
                })
                .insert((
                    RenderLayerMarker(RenderLayerKind::Terrain),
                    ViewModeVisibility(crate::ui::RenderViewMode::TwoD),
                ))
                .id();
            tile_storage.set(&tile_pos, tile_entity);
        }
    }
    let tile_size = TilemapTileSize {
        x: map_config.tile_size,
        y: map_config.tile_size,
    };
    let grid_size = tile_size.into();
    let map_type = TilemapType::default();
    commands.entity(tilemap_entity).insert(TilemapBundle {
        grid_size,
        map_type,
        size: map_size,
        storage: tile_storage.clone(),
        texture: TilemapTexture::Single(texture_handle),
        tile_size,
        anchor: TilemapAnchor::Center,
        transform: Transform::from_xyz(0.0, 0.0, 0.0), //z = 0.0 (background)
        ..Default::default()
    });
    commands.entity(tilemap_entity).insert((
        RenderLayerMarker(RenderLayerKind::Terrain),
        ViewModeVisibility(crate::ui::RenderViewMode::TwoD),
    ));
    // Add atlas to array texture loader so it's preprocessed before we need to use it.
    // Only used when the atlas feature is off and we are using array textures.
    #[cfg(all(not(feature = "atlas"), feature = "render"))]
    {
        array_texture_loader.add(TilemapArrayTexture {
            texture: TilemapTexture::Single(asset_server.load("tiles.png")),
            tile_size,
            ..Default::default()
        });
    }
    tile_storage
}
