use bevy::asset::Handle;
use bevy::image::Image;
use bevy::prelude::*;
use bevy_ecs_tilemap::prelude::*;
use sc2_proto::common::ImageData;
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
/// Get tile color based on terrain properties - Dark IDE theme
/// Creep overrides all other colors with purple
pub fn blend_tile_color(
    pathing: u8,
    placement: u8,
    creep: u8,
    energy: u8,
    height: u8,
) -> Color {
    let intensity = 0.5 + (height as f32 / 255.0) * 0.5;
    // Creep overrides everything with purple (like error highlighting in IDE)
    if creep > 0 {
        return Color::srgb(0.4 * intensity, 0.1 * intensity, 0.5 * intensity);
    }
    // Energy overrides with cyan/blue (like info highlighting)
    if energy > 0 {
        let intensity = 0.5 + (height as f32 / 255.0) * 0.5;
        return Color::srgb(0.1 * intensity, 0.3 * intensity, 0.6 * intensity);
    }
    // Discrete colors for pathable/placeable combinations (dark IDE theme)
    let base_color = match (pathing > 0, placement > 0) {
        (false, false) => Color::srgb(0.05, 0.05, 0.05),  // Non-pathable, non-placeable: Almost black #0D0D0D
        (true, false)  => Color::srgb(0.12, 0.12, 0.13),  // Pathable only: Dark grey #1E1E21 (like IDE background)
        (false, true)  => Color::srgb(0.18, 0.18, 0.20),  // Placeable only (rare): Medium-dark grey #2E2E33
        (true, true)   => Color::srgb(0.22, 0.22, 0.24),  // Both pathable & placeable: Light grey #383840 (like selected line)
    };
    // Apply height as brightness multiplier
    let intensity = 0.6 + (height as f32 / 255.0) * 0.4;
    Color::srgb(
        base_color.to_srgba().red * intensity,
        base_color.to_srgba().green * intensity,
        base_color.to_srgba().blue * intensity,
    )
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
    // Fill map tiles with blended colors
    for y in 0..height {
        for x in 0..width {
            let tile_pos = TilePos { x, y };
            // Get values from each layer
            let pathing = layers.pathing.as_ref().map_or(0, |l| l.get_value(x, y));
            let placement = layers.placement.as_ref().map_or(0, |l| l.get_value(x, y));
            let height_val = layers.height.as_ref().map_or(128, |l| l.get_value(x, y));
            let creep = layers.creep.as_ref().map_or(0, |l| l.get_value(x, y));
            let energy = layers.energy.as_ref().map_or(0, |l| l.get_value(x, y));
            // Get color based on all layers
            let color = blend_tile_color(pathing, placement, creep, energy, height_val);
            let tile_entity = commands
                .spawn(TileBundle {
                    position: tile_pos,
                    visible: TileVisible(true),
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
