use bevy::prelude::*;
use std::collections::{HashMap, HashSet};
use bevy::sprite::Anchor;
use sc2_proto::sc2api::ResponseObservation;
use protobuf::reflect::ReflectFieldRef;
use protobuf::Message;
use crate::entity_system::EntitySystem;

/// === Resources ===

#[derive(Resource, Default)]
pub struct UnitRegistry {
    pub map: HashMap<u64, Entity>, // SC2 unit tag → Bevy entity
}

#[derive(Resource, Default)]
pub struct UnitIconAssets {
    pub icons: HashMap<u32, Handle<Image>>, // unit_type → image handle
}

#[derive(Resource, Default)]
pub struct SelectedUnit {
    pub tag: Option<u64>,
}

#[derive(Resource, Default)]
pub struct UnitTypeIndex {
    pub by_type: HashMap<u32, Vec<Entity>>, // unit_type id -> spawned bevy entities
}

/// === Components ===

#[derive(Component)]
pub struct UnitTag(pub u64);

#[derive(Component)]
pub struct UnitType(pub u32);

#[derive(Component)]
pub struct UnitHealth {
    pub current: f32,
    pub max: f32,
}

#[derive(Component)]
pub struct UnitShield {
    pub current: f32,
    pub max: f32,
}

#[derive(Component)]
pub struct UnitBuildProgress(pub f32);

#[derive(Component)]
pub struct UnitAlliance(pub i32); // 1=Self, 2=Ally, 3=Neutral, 4=Enemy

#[derive(Component)]
pub struct UnitProto(pub sc2_proto::raw::Unit);

#[derive(Component, Default)]
pub struct CurrentOrderAbility(pub Option<u32>);

#[derive(Component)]
pub struct HealthBar;

#[derive(Component)]
pub struct ShieldBar;

#[derive(Component)]
pub struct BuildProgressBar;

/// === Unit handling logic ===

pub fn handle_observation(
    commands: &mut Commands,
    asset_server: &Res<AssetServer>,
    _icon_assets: &Res<UnitIconAssets>,
    registry: &mut ResMut<UnitRegistry>,
    type_index: &mut ResMut<UnitTypeIndex>,
    entity_system: &Res<EntitySystem>,
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
        let max_health = unit.health_max.unwrap_or(0.0);
        let shield = unit.shield.unwrap_or(0.0);
        let max_shield = unit.shield_max.unwrap_or(0.0);
        let build_progress = unit.build_progress.unwrap_or(0.0);
        let unit_type = unit.unit_type.unwrap();
        let tile_size = entity_system.tile_size;
        let world_x = x * tile_size - map_size.0 * tile_size / 2.0;
        let world_y = y * tile_size - map_size.1 * tile_size / 2.0;

        let first_order_ability = unit.orders.get(0).and_then(|o| o.ability_id);

        // Get display info from entity system
        let display = entity_system.get_display_info(unit_type);
        let size = entity_system.unit_size(unit_type);
        let image_handle = entity_system.get_icon_handle(unit_type, asset_server);

        if let Some(&entity) = registry.map.get(&tag) {
            // Update existing unit components
            commands.entity(entity).insert((
                Transform::from_xyz(world_x, world_y, 1.0),
                UnitHealth { current: health, max: max_health },
                UnitShield { current: shield, max: max_shield },
                UnitBuildProgress(build_progress),
                UnitProto(unit.clone()),
                CurrentOrderAbility(first_order_ability),
            ));
        } else {
            // Spawn new sprite based on config (without text label)
            let entity = commands
                .spawn((
                    Sprite {
                        image: image_handle,
                        custom_size: Some(Vec2::splat(size)),
                        anchor: Anchor::Center,
                        ..default()
                    },
                    Transform::from_xyz(world_x, world_y, 1.0),
                    UnitTag(tag),
                    UnitType(unit_type),
                    UnitHealth { current: health, max: max_health },
                    UnitShield { current: shield, max: max_shield },
                    UnitBuildProgress(build_progress),
                    UnitProto(unit.clone()),
                    CurrentOrderAbility(first_order_ability),
                ))
                .id();
            registry.map.insert(tag, entity);
            type_index.by_type.entry(unit_type).or_default().push(entity);
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

pub fn get_set_fields(unit: &sc2_proto::raw::Unit) -> Vec<(String, String)> {
    let descriptor = unit.descriptor();
    let mut result = Vec::new();
    for field in descriptor.fields() {
        match field.get_reflect(unit) {
            ReflectFieldRef::Optional(s) => {
                if field.has_field(unit) {
                    if let Some(val) = s {
                        result.push((field.name().to_string(), format!("{:?}", val)));
                    }
                }
            },
            ReflectFieldRef::Repeated(r) => {
                if r.len() > 0 {
                    let mut items = Vec::new();
                    for i in 0..r.len() {
                        let v = r.get(i).as_ref();
                        items.push(format!("{:?}", v));
                    }
                    result.push((field.name().to_string(), format!("[{}]", items.join(", "))));
                }
            },
            _ => continue,
        }
    }
    result
}

/// System to select unit on mouse click
pub fn unit_selection_system(
    windows: Query<&Window>,
    camera_query: Query<(&Camera, &GlobalTransform)>,
    _registry: Res<UnitRegistry>,
    unit_query: Query<(Entity, &Transform, &UnitTag)>,
    mouse_button_input: Res<ButtonInput<MouseButton>>,
    mut selected: ResMut<SelectedUnit>,
) {
    if !mouse_button_input.just_pressed(MouseButton::Left) {
        return;
    }
    let window = windows.single().unwrap();
    let (camera, camera_transform) = camera_query.single().unwrap();
    if let Some(cursor_pos) = window.cursor_position() {
        // Convert cursor position to world coordinates
        let world_pos = camera.viewport_to_world(camera_transform, cursor_pos);
        if let Ok(world_pos) = world_pos {
            let world_pos = world_pos.origin.truncate();
            // Check for unit under cursor
            for (_entity, transform, tag) in unit_query.iter() {
                let unit_pos = transform.translation.truncate();
                let distance = unit_pos.distance(world_pos);
                if distance < 16.0 { // Use unit size threshold
                    selected.tag = Some(tag.0);
                    break;
                }
            }
        }
    }
}

/// System to preload all unit icons at startup
#[allow(unused_variables)]
pub fn preload_unit_icons(asset_server: Res<AssetServer>, mut icons: ResMut<UnitIconAssets>) {
    // This function is now deprecated in favor of EntitySystem
    // Icons are pre-loaded in EntitySystem::load()
}

/// System to update unit visuals: alliance tint, health/shield/build progress bars
pub fn update_unit_visuals(
    mut commands: Commands,
    mut unit_query: Query<(
        Entity,
        &UnitProto,
        &UnitType,
        &UnitHealth,
        &UnitShield,
        &UnitBuildProgress,
        &mut Sprite,
        Option<&Children>,
    )>,
    entity_system: Res<EntitySystem>,
    child_query: Query<Entity>,
) {
    for (entity, proto, unit_type, health, shield, build_progress, mut sprite, children) in unit_query.iter_mut() {
        use sc2_proto::raw::Alliance;
        let alliance = proto.0.alliance.unwrap_or(Alliance::Neutral); // Default to neutral
        let size = entity_system.unit_size(unit_type.0);

        // 1. Apply reddish tint for enemy units
        if alliance == Alliance::Enemy {
            sprite.color = Color::srgb(1.5, 0.7, 0.7); // Reddish tint
        } else {
            sprite.color = Color::WHITE;
        }

        // 2. Remove old children (bars and labels)
        if let Some(children) = children {
            for child in children.iter() {
                if child_query.contains(child) {
                    commands.entity(child).despawn();
                }
            }
        }

        let bar_width = size;
        let bar_height = 3.0;
        let bar_y_offset = -(size / 2.0) - 2.0; // Directly below icon (2px gap)

        // 3. Show build progress bar if building is not complete
        if build_progress.0 < 1.0 && build_progress.0 > 0.0 {
            let progress_width = bar_width * build_progress.0;

            // Background for build progress
            let bg_entity = commands.spawn((
                Sprite {
                    color: Color::srgb(0.2, 0.2, 0.2), // Dark background
                    custom_size: Some(Vec2::new(bar_width, bar_height)),
                    ..default()
                },
                Transform::from_xyz(0.0, bar_y_offset, 0.0),
            )).id();
            commands.entity(entity).add_child(bg_entity);

            // Progress bar
            let progress_entity = commands.spawn((
                Sprite {
                    color: Color::srgb(1.0, 1.0, 0.0), // Yellow for build progress
                    custom_size: Some(Vec2::new(progress_width, bar_height)),
                    ..default()
                },
                Transform::from_xyz(-(bar_width - progress_width) / 2.0, bar_y_offset, 0.1),
                BuildProgressBar,
            )).id();
            commands.entity(entity).add_child(progress_entity);
        } else {
            // 4. Show health bar (green)
            if health.max > 0.0 {
                let health_percent = (health.current / health.max).clamp(0.0, 1.0);
                let health_width = bar_width * health_percent;

                // Health bar background
                let health_bg = commands.spawn((
                    Sprite {
                        color: Color::srgb(0.2, 0.2, 0.2), // Dark background
                        custom_size: Some(Vec2::new(bar_width, bar_height)),
                        ..default()
                    },
                    Transform::from_xyz(0.0, bar_y_offset, 0.0),
                )).id();
                commands.entity(entity).add_child(health_bg);

                // Health bar foreground
                let health_fg = commands.spawn((
                    Sprite {
                        color: Color::srgb(0.0, 1.0, 0.0), // Green
                        custom_size: Some(Vec2::new(health_width, bar_height)),
                        ..default()
                    },
                    Transform::from_xyz(-(bar_width - health_width) / 2.0, bar_y_offset, 0.1),
                    HealthBar,
                )).id();
                commands.entity(entity).add_child(health_fg);
            }

            // 5. Show shield bar (blue) if present
            if shield.max > 0.0 {
                let shield_y_offset = bar_y_offset - bar_height - 1.0; // Below health bar
                let shield_percent = (shield.current / shield.max).clamp(0.0, 1.0);
                let shield_width = bar_width * shield_percent;

                // Shield bar background
                let shield_bg = commands.spawn((
                    Sprite {
                        color: Color::srgb(0.2, 0.2, 0.2), // Dark background
                        custom_size: Some(Vec2::new(bar_width, bar_height)),
                        ..default()
                    },
                    Transform::from_xyz(0.0, shield_y_offset, 0.0),
                )).id();
                commands.entity(entity).add_child(shield_bg);

                // Shield bar foreground
                let shield_fg = commands.spawn((
                    Sprite {
                        color: Color::srgb(0.0, 0.5, 1.0), // Blue
                        custom_size: Some(Vec2::new(shield_width, bar_height)),
                        ..default()
                    },
                    Transform::from_xyz(-(bar_width - shield_width) / 2.0, shield_y_offset, 0.1),
                    ShieldBar,
                )).id();
                commands.entity(entity).add_child(shield_fg);
            }
        }

        // // Re-add text label
        // let text_label = if let Some(aid) = proto.0.orders.get(0).and_then(|o| o.ability_id) {
        //     entity_system.ability_name(aid).unwrap_or("").to_string()
        // } else {
        //     let display = entity_system.get_display_info(unit_type.0);
        //     display.label.unwrap_or_default()
        // };
        //
        // let label_entity = commands.spawn((
        //     Text2d::new(text_label),
        //     TextLayout::new_with_justify(JustifyText::Center),
        //     TextFont::from_font_size(14.),
        //     Transform::from_xyz(0., -(size / 2.0) - 6.0, 0.),
        //     bevy::sprite::Anchor::TopCenter,
        // )).id();
        // commands.entity(entity).add_child(label_entity);
    }
}
