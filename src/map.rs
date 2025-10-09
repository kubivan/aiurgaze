use bevy::asset::{Handle, RenderAssetUsages};
use bevy::image::Image;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy_ecs_tilemap::prelude::*;
use bevy_sprite::SliceScaleMode::Tile;
use sc2_proto::common::ImageData;
use sc2_proto::sc2api::{ResponseGameInfo, ResponseObservation};

#[derive(PartialEq)]
pub enum TerrainLayerKind {
    Pathing,
    Placement,
    Height,
}

pub struct TerrainLayer {
    kind: TerrainLayerKind,
    pub width: u32,
    pub height: u32,
    data: Vec<u8>, // raw bytes from ImageData.data
}

fn unpack_bits(bytes: &[u8]) -> Vec<bool> {
    let mut bits = Vec::with_capacity(bytes.len() * 8);
    for byte in bytes {
        for i in (0..8).rev() {
            bits.push((byte >> i) & 1 == 1);
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
}

pub fn spawn_tilemap(
    commands: &mut Commands,
    layer: &TerrainLayer,
    asset_server: &mut Res<AssetServer>,
    #[cfg(all(not(feature = "atlas"), feature = "render"))] array_texture_loader: Res<
        ArrayTextureLoader,
    >,
) -> TileStorage {

    let texture_handle: Handle<Image> = asset_server.load("tiles.png");

    let map_size = TilemapSize {
        x: layer.width,
        y: layer.height,
    };
    //let map_size = TilemapSize { x: 32, y: 32 };

    let tilemap_entity = commands.spawn_empty().id();

    let mut tile_storage = TileStorage::empty(map_size);

    // Fill map tiles
    for y in 0..layer.height - 1 {
        for x in 0..layer.width -1 {
            let tile_pos = TilePos { x, y };

            let idx = (y * layer.width + x) as usize;
            let val = layer.data[idx];
            let texture_index = if val == 0 { 4 } else { 5 };

            let tile_entity = commands
                .spawn(TileBundle {
                    position: tile_pos,
                    tilemap_id: TilemapId(tilemap_entity),
                    // color: TileColor(color),
                    texture_index: TileTextureIndex(texture_index),
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

