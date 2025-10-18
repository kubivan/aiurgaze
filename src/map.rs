use bevy::asset::Handle;
use bevy::image::Image;
use bevy::prelude::*;
use bevy_ecs_tilemap::prelude::*;
use sc2_proto::common::ImageData;
use crate::app_settings::StyleConfig;
#[derive(PartialEq)]
pub enum TerrainLayerKind {
    Pathing,
    Placement,
    Height,
    Creep,
    Energy,
}
pub struct TerrainLayer {
    kind: TerrainLayerKind,
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
    pub fn from_image_data1(data: &[u8], kind: TerrainLayerKind, width: u32, height: u32) -> Self {
        // 1 bit per pixel: unpack each bit
        let bits = unpack_bits(data);
        let pixels: Vec<u8> = bits
            .iter()
            .map(|&b| if b { 255u8 } else { 0 })
            .collect();
        TerrainLayer {
            kind,
            width,
            height,
            data: pixels, // now 8 bits per pixel
        }
    }
    pub fn from_image_data(img: &ImageData, kind: TerrainLayerKind) -> Self {
        let img_size = img.size.clone().unwrap();
        let width = img_size.x.unwrap().clone() as u32;
        let height = img_size.y.unwrap().clone() as u32;
        let bits = img.bits_per_pixel.unwrap();
        if bits == 1 {
            Self::from_image_data1(img.data.as_ref().unwrap(), kind, width, height)
        } else {
            Self::from_image_data8(img, kind)
        }
    }
    pub fn from_image_data8(img: &ImageData, kind: TerrainLayerKind) -> Self {
        let bits = img.bits_per_pixel.unwrap();
        assert_eq!(bits, 8, "Only 8 bits per pixel supported for now (got {bits})");
        let img_size = img.size.clone().unwrap();
        let width = img_size.x.unwrap().clone() as u32;
        let height = img_size.y.unwrap().clone() as u32;
        assert_eq!(
            img.data.as_ref().unwrap().len(),
            (width * height) as usize,
            "Image data size mismatch"
        );
        Self {
            kind,
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
/// Get tile color based on terrain properties - uses StyleConfig from app settings
/// Creep overrides all other colors with purple
pub fn blend_tile_color(
    pathing: u8,
    placement: u8,
    creep: u8,
    energy: u8,
    height: u8,
    style: &StyleConfig,
) -> Color {
    // Creep overrides everything with purple
    if creep > 0 {
        let color = style.get_creep_color();
        return style.apply_height_intensity(color, height);
    }
    // Energy overrides with cyan/blue
    if energy > 0 {
        let color = style.get_energy_color();
        return style.apply_height_intensity(color, height);
    }
    // Get discrete color for pathable/placeable combination
    let base_color = style.get_terrain_color(pathing > 0, placement > 0);
    style.apply_height_intensity(base_color, height)
}
pub struct TerrainLayers {
    pub pathing: Option<TerrainLayer>,
    pub placement: Option<TerrainLayer>,
    pub height: Option<TerrainLayer>,
    pub creep: Option<TerrainLayer>,
    pub energy: Option<TerrainLayer>,
}
impl TerrainLayers {
    pub fn new() -> Self {
        Self {
            pathing: None,
            placement: None,
            height: None,
            creep: None,
            energy: None,
        }
    }
    pub fn add_layer(&mut self, layer: TerrainLayer) {
        match layer.kind {
            TerrainLayerKind::Pathing => self.pathing = Some(layer),
            TerrainLayerKind::Placement => self.placement = Some(layer),
            TerrainLayerKind::Height => self.height = Some(layer),
            TerrainLayerKind::Creep => self.creep = Some(layer),
            TerrainLayerKind::Energy => self.energy = Some(layer),
        }
    }
    pub fn get_dimensions(&self) -> (u32, u32) {
        // Get dimensions from the first available layer
        if let Some(ref layer) = self.pathing {
            return (layer.width, layer.height);
        }
        if let Some(ref layer) = self.placement {
            return (layer.width, layer.height);
        }
        if let Some(ref layer) = self.height {
            return (layer.width, layer.height);
        }
        if let Some(ref layer) = self.creep {
            return (layer.width, layer.height);
        }
        if let Some(ref layer) = self.energy {
            return (layer.width, layer.height);
        }
        (0, 0)
    }
}
pub fn spawn_tilemap(
    commands: &mut Commands,
    layers: &TerrainLayers,
    asset_server: &mut Res<AssetServer>,
    style: &StyleConfig,
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
            let pathing = layers.pathing.as_ref().map_or(0, |l| l.get_value(x, y));
            let placement = layers.placement.as_ref().map_or(0, |l| l.get_value(x, y));
            let height_val = layers.height.as_ref().map_or(128, |l| l.get_value(x, y));
            let creep = layers.creep.as_ref().map_or(0, |l| l.get_value(x, y));
            let energy = layers.energy.as_ref().map_or(0, |l| l.get_value(x, y));
            // Get color based on all layers using style config
            let color = blend_tile_color(pathing, placement, creep, energy, height_val, style);
            let tile_entity = commands
                .spawn(TileBundle {
                    position: tile_pos,
                    tilemap_id: TilemapId(tilemap_entity),
                    color: TileColor(color),
                    texture_index: TileTextureIndex(5), // Use a single white tile for coloring
                    ..Default::default()
                })
                .id();
            tile_storage.set(&tile_pos, tile_entity);
        }
    }
    let tile_size = TilemapTileSize { x: 16.0, y: 16.0 };
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
    return tile_storage;
}
