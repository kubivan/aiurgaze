use bevy::prelude::*;
use std::collections::{HashMap, HashSet};
use bevy::diagnostic::FrameCount;
use bevy::sprite::Anchor;
use sc2_proto::sc2api::{Observation, ResponseObservation};
use tiled::HorizontalAlignment::Justify;

/// === Resources ===

#[derive(Resource, Default)]
pub struct UnitRegistry {
    pub map: HashMap<u64, Entity>, // SC2 unit tag → Bevy entity
}

#[derive(Resource, Default)]
pub struct UnitIconAssets {
    pub icons: HashMap<u32, Handle<Image>>, // unit_type → image handle
}

/// === Components ===

#[derive(Component)]
pub struct UnitTag(pub u64);

#[derive(Component)]
pub struct UnitType(pub u32);

#[derive(Component)]
pub struct UnitHealth(pub f32);

/// === Unit handling logic ===

fn image_path(unit_type: u32) -> &'static str {
    match unit_type {
        59 => "pngs/nexus.png",
        // Add more mappings as needed
        _ => "pngs/probe.png",
    }
}
fn image_size(unit_type: u32) -> f32 {
    match unit_type {
        59 => 16.0 * 9.0,
        _ => 16.0 * 2.0,
    }
}

pub fn handle_observation(
    commands: &mut Commands,
    asset_server: &Res<AssetServer>,
    icon_assets: &Res<UnitIconAssets>,
    registry: &mut ResMut<UnitRegistry>,
    obs_msg: &ResponseObservation,
) {
    let obs = obs_msg.observation.as_ref().unwrap();
    let raw_data = obs.raw_data.as_ref().unwrap();

    let mut seen_tags = HashSet::new();
    let map_size = (200.0 , 176.0);

    for unit in &raw_data.units {
        let tag = unit.tag.unwrap();
        seen_tags.insert(tag);
        let pos = unit.pos.as_ref().unwrap();
        let (x, y, _z ) = (pos.x.unwrap(), pos.y.unwrap(), pos.z.unwrap());
        let health = unit.health.unwrap_or(0.0);
        let unit_type = unit.unit_type.unwrap();
        let tile_size = 16.0;
        let world_x = x * tile_size;
        let world_y = y * tile_size;

        let image = icon_assets.icons.get(&unit_type).cloned().unwrap_or_else(|| asset_server.load(image_path(unit_type)));

        if let Some(&entity) = registry.map.get(&tag) {
            commands.entity(entity).insert((
                Transform::from_xyz(world_x - map_size.0 * tile_size / 2.0, world_y - map_size.1 * tile_size / 2.0, 1.0),
                UnitHealth(health),
            ));
        } else {
            let entity = commands
                .spawn((
                    Sprite {
                        image,
                        custom_size: Some(Vec2::splat(image_size(unit_type))),
                        anchor: Anchor::Center,
                        ..default()
                    },
                    Transform::from_xyz(world_x, world_y , 1.0 ),
                    UnitTag(tag),
                    UnitType(unit_type),
                    UnitHealth(health),
                ))
                .id();
            registry.map.insert(tag, entity);
        }
    }
    // Optional: Despawn units not seen anymore
    /*
    let to_remove: Vec<u64> = registry
        .map
        .keys()
        .filter(|tag| !seen_tags.contains(tag))
        .cloned()
        .collect();

    for tag in to_remove {
        if let Some(entity) = registry.map.remove(&tag) {
            commands.entity(entity).despawn();
        }
    }
    */
}

/// System to preload all unit icons at startup
pub fn preload_unit_icons(asset_server: Res<AssetServer>, mut icons: ResMut<UnitIconAssets>) {
    let unit_types = vec![59, /* add more unit types here */];
    for unit_type in unit_types {
        let path = image_path(unit_type);
        let handle = asset_server.load(path);
        icons.icons.insert(unit_type, handle);
    }
}
