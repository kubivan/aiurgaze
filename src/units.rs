use bevy::prelude::*;
use std::collections::{HashMap, HashSet};
use bevy::sprite::Anchor;
use sc2_proto::sc2api::ResponseObservation;
use protobuf::reflect::ReflectFieldRef;
use protobuf::Message;
use crate::entity_system::EntitySystem;
use bevy_health_bar3d::prelude::*;
use sc2_proto::raw::Alliance;

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

/// === Components ===

#[derive(Component)]
pub struct UnitTag(pub u64);

#[derive(Component)]
pub struct UnitType(pub u32);

#[derive(Component, Reflect)]
#[reflect(Component)]
pub struct UnitHealth {
    pub current: f32,
    pub max: f32,
}
impl Percentage for UnitHealth {
    fn value(&self) -> f32 {
        self.current / self.max
    }
}


#[derive(Component, Reflect)]
#[reflect(Component)]
pub struct UnitShield {
    pub current: f32,
    pub max: f32,
}
impl Percentage for UnitShield {
    fn value(&self) -> f32 {
        if self.max <= 0.0 {
            0.0
        } else {
            self.current / self.max
        }
    }
}

#[derive(Component, Reflect)]
#[reflect(Component)]
pub struct UnitBuildProgress(pub f32);
impl Percentage for UnitBuildProgress {
    fn value(&self) -> f32 {
        self.0
    }
}

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
    registry: &mut ResMut<UnitRegistry>,
    entity_system: &Res<EntitySystem>,
    obs_msg: &ResponseObservation,
    unit_query: Query<&UnitBuildProgress>,
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

        let unit_radius = unit.radius.unwrap_or(1.0);

        let first_order_ability = unit.orders.get(0).and_then(|o| o.ability_id);

        //Apply reddish tint for enemy units
        let sprite_color = match unit.alliance.as_ref().unwrap() {
            Alliance::Enemy => Color::srgb(1.0, 0.5, 0.5),
            _ =>  Color::WHITE
        };

        // Get display info from an entity system
        let size = unit_radius * 2.0 * tile_size;
        let image_handle = entity_system.get_icon_handle(unit_type, asset_server);

        if let Some(&entity) = registry.map.get(&tag) {
            commands.entity(entity).insert((
                Transform::from_xyz(world_x, world_y, 1.0),
                UnitHealth { current: health, max: max_health },
                UnitShield { current: shield, max: max_shield },
                UnitProto(unit.clone()),
                CurrentOrderAbility(first_order_ability),
            ));

            // Prevent flickering: only insert/remove build progress bar if needed
            let has_build_progress = unit_query.get(entity).is_ok();
            if build_progress < 1.0 {
                commands.entity(entity).insert(UnitBuildProgress(build_progress));
            } else if has_build_progress {
                commands.entity(entity).remove::<BarSettings<UnitBuildProgress>>();
                commands.entity(entity).remove::<UnitBuildProgress>();
            }
        } else {
            // Spawn new sprite based on config (without text label)
            // Inside the else block for spawning new units
            let mut entity_commands = commands.spawn((
                Sprite {
                    image: image_handle,
                    custom_size: Some(Vec2::splat(size)),
                    color: sprite_color,
                    anchor: Anchor::Center,
                    ..default()
                },
                Transform::from_xyz(world_x, world_y, 1.0),
                UnitTag(tag),
                UnitType(unit_type),
                UnitHealth { current: health, max: max_health },
                BarSettings::<UnitHealth> {
                    offset: -size / 2.,
                    height: BarHeight::Static(1.),
                    width: size,
                    ..default()
                },
                UnitProto(unit.clone()),
                CurrentOrderAbility(first_order_ability),
            ));

            // Conditionally add shield bar
            if max_shield > 0.0 {
                entity_commands.insert((
                    UnitShield { current: shield, max: max_shield },
                    BarSettings::<UnitShield> {
                        offset: -size / 2. - 2.0,
                        height: BarHeight::Static(1.),
                        width: size,
                        ..default()
                    },
                ));
            }

            // Conditionally add build progress bar
            if build_progress < 1.0 {
                entity_commands.insert((
                    UnitBuildProgress(build_progress),
                    BarSettings::<UnitBuildProgress> {
                        offset: -size / 2. - 4.0,
                        height: BarHeight::Static(1.),
                        width: size,
                        ..default()
                    },
                ));
            }

            let entity = entity_commands.id();
            registry.map.insert(tag, entity);
        }
    }
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
                if distance < 16.0 { // Use unit size threshold //TODO: Make configurable: tile_size
                    selected.tag = Some(tag.0);
                    break;
                }
            }
        }
    }
}

/// System to draw lines from units to their order targets
pub fn draw_unit_orders(
    mut gizmos: Gizmos,
    unit_query: Query<(&Transform, &UnitProto)>,
    registry: Res<UnitRegistry>,
    entity_system: Res<EntitySystem>,
) {
    //TODO: remove hardcode
    let map_size = (200.0, 176.0);
    let tile_size = entity_system.tile_size;

    for (transform, proto) in unit_query.iter().filter(|(_ , proto)| { proto.0.orders.len() > 0}) {
        // Get the first order if it exists
        let order = proto.0.orders.get(0).unwrap();
        let start_pos = Vec2::new(transform.translation.x, transform.translation.y);

        // Check if the order has a target using the oneof enum
        use sc2_proto::raw::UnitOrder_oneof_target;

        match order.target.as_ref() {
            Some(UnitOrder_oneof_target::target_world_space_pos(point)) => {
                // Target is a position
                let target_x = point.x.unwrap_or(0.0);
                let target_y = point.y.unwrap_or(0.0);

                // Convert SC2 coordinates to world coordinates
                let world_x = target_x * tile_size - map_size.0 * tile_size / 2.0;
                let world_y = target_y * tile_size - map_size.1 * tile_size / 2.0;
                let end_pos = Vec2::new(world_x, world_y);

                // Draw dashed line to position target
                draw_dashed_line(&mut gizmos, start_pos, end_pos, Color::srgba(0.8, 0.8, 0.2, 0.6));

                // Draw small circle at target position
                gizmos.circle_2d(end_pos, 4.0, Color::srgba(1.0, 1.0, 0.3, 0.7));
            }
            Some(UnitOrder_oneof_target::target_unit_tag(target_tag)) => {
                // Target is another unit
                if let Some(&target_entity) = registry.map.get(target_tag) {
                    if let Ok((target_transform, _)) = unit_query.get(target_entity) {
                        let end_pos = Vec2::new(target_transform.translation.x, target_transform.translation.y);

                        // Draw solid line to unit target
                        gizmos.line_2d(start_pos, end_pos, Color::srgba(0.2, 0.8, 0.8, 0.7));

                        // Draw a small arrow head
                        draw_arrow_head(&mut gizmos, start_pos, end_pos, Color::srgba(0.2, 0.8, 0.8, 0.7));
                    }
                }
            }
            _ => continue,
        }
    }
}

/// Helper function to draw a dashed line
fn draw_dashed_line(gizmos: &mut Gizmos, start: Vec2, end: Vec2, color: Color) {
    let direction = end - start;
    let distance = direction.length();
    let normalized = direction.normalize_or_zero();

    let dash_length = 8.0;
    let gap_length = 4.0;
    let segment_length = dash_length + gap_length;

    let mut current_distance = 0.0;

    while current_distance < distance {
        let segment_start = start + normalized * current_distance;
        let segment_end_distance = (current_distance + dash_length).min(distance);
        let segment_end = start + normalized * segment_end_distance;

        gizmos.line_2d(segment_start, segment_end, color);

        current_distance += segment_length;
    }
}

/// Helper function to draw an arrow head at the end of a line
fn draw_arrow_head(gizmos: &mut Gizmos, start: Vec2, end: Vec2, color: Color) {
    let direction = (end - start).normalize_or_zero();
    let arrow_size = 8.0;
    let arrow_angle = std::f32::consts::PI / 6.0; // 30 degrees

    // Calculate arrow head points
    let perpendicular = Vec2::new(-direction.y, direction.x);

    let left_point = end - direction * arrow_size + perpendicular * arrow_size * arrow_angle.sin();
    let right_point = end - direction * arrow_size - perpendicular * arrow_size * arrow_angle.sin();

    gizmos.line_2d(end, left_point, color);
    gizmos.line_2d(end, right_point, color);
}
