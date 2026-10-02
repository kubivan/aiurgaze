use bevy::ecs::system::SystemParam;
use bevy::mesh::{Mesh, Mesh3d};
use bevy::pbr::{MeshMaterial3d, StandardMaterial};
use bevy::prelude::*;
use sc2_proto::sc2api::ResponseObservation;
use std::collections::{HashMap, HashSet};

use crate::controller::MapResource;
use crate::entity_system::EntitySystem;
use crate::map::map_position_3d;
use crate::render_layers::ViewModeVisibility;
use crate::render_view::RenderViewMode;
use bevy_health_bar3d::prelude::*;
use protobuf::reflect::ReflectFieldRef;
use protobuf::Message;
use sc2_proto::raw::Alliance;

/// === Resources ===

#[derive(Resource, Default)]
pub struct UnitRegistry {
    pub map: HashMap<u64, Entity>, // SC2 unit tag → Bevy entity
}

#[derive(Resource, Default)]
pub struct SelectedUnit {
    pub tag: Option<u64>,
}

#[derive(Resource, Debug, Clone)]
pub struct UnitCompositionVisibility {
    pub show_orders: bool,
}

#[derive(Resource, Default)]
pub struct Unit3dMaterialCache {
    card_materials: HashMap<(u32, bool), Handle<StandardMaterial>>,
    bar_background: Option<Handle<StandardMaterial>>,
    health_fill: Option<Handle<StandardMaterial>>,
    shield_fill: Option<Handle<StandardMaterial>>,
    build_fill: Option<Handle<StandardMaterial>>,
}

#[derive(SystemParam)]
pub struct Unit3dRenderAssets<'w> {
    meshes: ResMut<'w, Assets<Mesh>>,
    materials: ResMut<'w, Assets<StandardMaterial>>,
    cache: ResMut<'w, Unit3dMaterialCache>,
}

#[derive(Component, Clone, Copy)]
enum Unit3dPart {
    Card,
    HealthBackground,
    HealthFill,
    ShieldBackground,
    ShieldFill,
    BuildBackground,
    BuildFill,
}

#[derive(Component)]
pub(crate) struct Unit3dBillboard {
    unit_entity: Entity,
    part: Unit3dPart,
    width: f32,
    height: f32,
}

impl Default for UnitCompositionVisibility {
    fn default() -> Self {
        Self { show_orders: true }
    }
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
/// Resource to store unit tags seen in the current observation for cleanup phase
#[derive(Resource, Default)]
pub struct ObservationUnitTags {
    pub seen_tags: HashSet<u64>,
}

/// First phase: Clean up units that are no longer present
/// This runs BEFORE handle_observation to avoid race conditions
pub fn cleanup_dead_units(
    mut commands: Commands,
    mut registry: ResMut<UnitRegistry>,
    seen_tags: Res<ObservationUnitTags>,
) {
    if seen_tags.seen_tags.is_empty() {
        return; // No observation processed yet
    }

    let to_remove: Vec<u64> = registry
        .map
        .keys()
        .filter(|tag| !seen_tags.seen_tags.contains(tag))
        .cloned()
        .collect();

    for tag in to_remove {
        if let Some(entity) = registry.map.remove(&tag) {
            commands.entity(entity).despawn();
        }
    }
}

/// Second phase: Update existing units and spawn new ones
pub fn handle_observation(
    commands: &mut Commands,
    asset_server: &Res<AssetServer>,
    registry: &mut ResMut<UnitRegistry>,
    entity_system: &Res<EntitySystem>,
    obs_msg: &ResponseObservation,
    unit_query: Query<&UnitBuildProgress>,
    seen_tags: &mut ResMut<ObservationUnitTags>,
    map_size: Option<(f32, f32)>,
    visual_assets: &mut Unit3dRenderAssets,
) {
    let obs = obs_msg.observation.as_ref().unwrap();
    let raw_data = obs.raw_data.as_ref().unwrap();

    // NOTE: Do not clear `seen_tags` here.
    // It is cleared once per response_controller_system frame and then
    // accumulated across all ObservationEvents (e.g. VisionMode::All
    // can receive multiple player observations in the same frame).
    // Clearing here causes later observations to drop entities from
    // earlier observations, leading to despawn races with deferred
    // bar-spawn commands.

    // Skip unit processing if map size not yet available
    let Some(map_size) = map_size else {
        return;
    };

    for unit in &raw_data.units {
        let tag = unit.tag.unwrap();
        seen_tags.seen_tags.insert(tag);
        let pos = unit.pos.as_ref().unwrap();
        let (x, y, _z) = (pos.x.unwrap(), pos.y.unwrap(), pos.z.unwrap());
        let health = unit.health.unwrap_or(1.0);
        let max_health = unit.health_max.unwrap_or(1.0); //TODO: check zero division
        let shield = unit.shield.unwrap_or(0.0);
        let max_shield = unit.shield_max.unwrap_or(0.0);
        let build_progress = unit.build_progress.unwrap_or(0.0);
        let unit_type = unit.unit_type.unwrap();
        let tile_size = entity_system.map_config.tile_size;
        let world_x = x * tile_size - map_size.0 * tile_size / 2.0;
        let world_y = y * tile_size - map_size.1 * tile_size / 2.0;

        let unit_radius = unit.radius.unwrap_or(1.0);

        let first_order_ability = unit.orders.first().and_then(|o| o.ability_id);

        //Apply reddish tint for enemy units
        let sprite_color = match unit.alliance.as_ref().unwrap() {
            Alliance::Enemy => Color::srgb(1.0, 0.5, 0.5),
            _ => Color::WHITE,
        };

        // Get display info from an entity system
        // Use custom tile size if specified in config, otherwise use unit radius
        let size = if let Some(custom_size) = entity_system.get_custom_tile_size(unit_type) {
            Vec2::new(custom_size[0] * tile_size, custom_size[1] * tile_size)
        } else {
            Vec2::splat(unit_radius * 2.0 * tile_size)
        };
        let image_handle = entity_system.get_icon_handle(unit_type, asset_server);

        if let Some(&entity) = registry.map.get(&tag) {
            commands.entity(entity).insert((
                Transform::from_xyz(world_x, world_y, 1.0),
                UnitHealth {
                    current: health,
                    max: max_health,
                },
                UnitShield {
                    current: shield,
                    max: max_shield,
                },
                UnitProto(unit.clone()),
                CurrentOrderAbility(first_order_ability),
            ));

            commands.entity(entity).insert(BarSettings::<UnitHealth> {
                offset: -size.y / 2.,
                height: BarHeight::Static(1.),
                width: size.x,
                ..default()
            });

            if max_shield > 0.0 {
                commands.entity(entity).insert(BarSettings::<UnitShield> {
                    offset: -size.y / 2. - 2.0,
                    height: BarHeight::Static(1.),
                    width: size.x,
                    ..default()
                });
            }

            // Prevent flickering: only insert/remove build progress bar if needed
            let has_build_progress = unit_query.get(entity).is_ok();
            if build_progress < 1.0 {
                commands
                    .entity(entity)
                    .insert(UnitBuildProgress(build_progress));
            } else if has_build_progress {
                commands
                    .entity(entity)
                    .remove::<BarSettings<UnitBuildProgress>>();
                commands.entity(entity).remove::<UnitBuildProgress>();
            }
        } else {
            // Spawn new sprite based on config (without text label)
            // Inside the else block for spawning new units
            let entity = commands
                .spawn((
                    Sprite {
                        image: image_handle.clone(),
                        custom_size: Some(size),
                        color: sprite_color,
                        ..default()
                    },
                    Transform::from_xyz(world_x, world_y, 1.0),
                    UnitTag(tag),
                    UnitType(unit_type),
                    UnitHealth {
                        current: health,
                        max: max_health,
                    },
                    BarSettings::<UnitHealth> {
                        offset: -size.y / 2.,
                        height: BarHeight::Static(1.),
                        width: size.x,
                        ..default()
                    },
                    UnitProto(unit.clone()),
                    CurrentOrderAbility(first_order_ability),
                ))
                .id();

            // Conditionally add shield bar
            if max_shield > 0.0 {
                commands.entity(entity).insert((
                    UnitShield {
                        current: shield,
                        max: max_shield,
                    },
                    BarSettings::<UnitShield> {
                        offset: -size.y / 2. - 2.0,
                        height: BarHeight::Static(1.),
                        width: size.x,
                        ..default()
                    },
                ));
            }

            // Conditionally add build progress bar
            if build_progress < 1.0 {
                commands.entity(entity).insert((
                    UnitBuildProgress(build_progress),
                    BarSettings::<UnitBuildProgress> {
                        offset: -size.y / 2. - 4.0,
                        height: BarHeight::Static(1.),
                        width: size.x,
                        ..default()
                    },
                ));
            }

            spawn_unit_3d_visuals(
                commands,
                visual_assets,
                entity,
                unit_type,
                matches!(unit.alliance.as_ref().unwrap(), Alliance::Enemy),
                sprite_color,
                image_handle,
                size,
            );
            registry.map.insert(tag, entity);
        }
    }
}

fn spawn_unit_3d_visuals(
    commands: &mut Commands,
    visual_assets: &mut Unit3dRenderAssets,
    unit_entity: Entity,
    unit_type: u32,
    is_enemy: bool,
    sprite_color: Color,
    image_handle: Handle<Image>,
    size: Vec2,
) {
    let card_material = if let Some(handle) = visual_assets
        .cache
        .card_materials
        .get(&(unit_type, is_enemy))
    {
        handle.clone()
    } else {
        let handle = visual_assets.materials.add(StandardMaterial {
            base_color: sprite_color,
            base_color_texture: Some(image_handle),
            alpha_mode: AlphaMode::Blend,
            cull_mode: None,
            unlit: true,
            ..default()
        });
        visual_assets
            .cache
            .card_materials
            .insert((unit_type, is_enemy), handle.clone());
        handle
    };

    if visual_assets.cache.bar_background.is_none() {
        visual_assets.cache.bar_background = Some(
            visual_assets
                .materials
                .add(bar_material(Color::srgba(0.04, 0.05, 0.06, 0.95))),
        );
        visual_assets.cache.health_fill = Some(
            visual_assets
                .materials
                .add(bar_material(Color::srgb(0.18, 0.85, 0.28))),
        );
        visual_assets.cache.shield_fill = Some(
            visual_assets
                .materials
                .add(bar_material(Color::srgb(0.2, 0.55, 1.0))),
        );
        visual_assets.cache.build_fill = Some(
            visual_assets
                .materials
                .add(bar_material(Color::srgb(1.0, 0.82, 0.12))),
        );
    }

    let card_mesh = visual_assets.meshes.add(Rectangle::new(size.x, size.y));
    spawn_unit_3d_part(
        commands,
        unit_entity,
        card_mesh,
        card_material,
        Unit3dPart::Card,
        size,
    );

    let bar_mesh = visual_assets.meshes.add(Rectangle::new(size.x, 1.5));
    let background = visual_assets.cache.bar_background.as_ref().unwrap().clone();
    let health_fill = visual_assets.cache.health_fill.as_ref().unwrap().clone();
    let shield_fill = visual_assets.cache.shield_fill.as_ref().unwrap().clone();
    let build_fill = visual_assets.cache.build_fill.as_ref().unwrap().clone();
    for (background_part, fill_part, fill_material) in [
        (
            Unit3dPart::HealthBackground,
            Unit3dPart::HealthFill,
            health_fill,
        ),
        (
            Unit3dPart::ShieldBackground,
            Unit3dPart::ShieldFill,
            shield_fill,
        ),
        (
            Unit3dPart::BuildBackground,
            Unit3dPart::BuildFill,
            build_fill,
        ),
    ] {
        spawn_unit_3d_part(
            commands,
            unit_entity,
            bar_mesh.clone(),
            background.clone(),
            background_part,
            size,
        );
        spawn_unit_3d_part(
            commands,
            unit_entity,
            bar_mesh.clone(),
            fill_material,
            fill_part,
            size,
        );
    }
}

fn bar_material(color: Color) -> StandardMaterial {
    StandardMaterial {
        base_color: color,
        alpha_mode: AlphaMode::Blend,
        cull_mode: None,
        unlit: true,
        ..default()
    }
}

fn spawn_unit_3d_part(
    commands: &mut Commands,
    unit_entity: Entity,
    mesh: Handle<Mesh>,
    material: Handle<StandardMaterial>,
    part: Unit3dPart,
    size: Vec2,
) {
    commands.spawn((
        Mesh3d(mesh),
        MeshMaterial3d(material),
        Transform::IDENTITY,
        Visibility::Hidden,
        ViewModeVisibility(RenderViewMode::ThreeD),
        Unit3dBillboard {
            unit_entity,
            part,
            width: size.x,
            height: size.y,
        },
        ChildOf(unit_entity),
    ));
}

pub fn update_unit_3d_billboards(
    camera_query: Query<&GlobalTransform, (With<Camera3d>, Without<Unit3dBillboard>)>,
    unit_query: Query<
        (
            &Transform,
            &UnitProto,
            &UnitHealth,
            Option<&UnitShield>,
            Option<&UnitBuildProgress>,
        ),
        Without<Unit3dBillboard>,
    >,
    mut billboard_query: Query<(&mut Transform, &Unit3dBillboard)>,
    entity_system: Res<EntitySystem>,
) {
    let Ok(camera_transform) = camera_query.single() else {
        return;
    };
    let camera_rotation = camera_transform.to_scale_rotation_translation().1;
    let tile_size = entity_system.map_config.tile_size;

    for (mut transform, billboard) in &mut billboard_query {
        let Ok((unit_transform, proto, health, shield, build_progress)) =
            unit_query.get(billboard.unit_entity)
        else {
            continue;
        };
        let Some(position) = proto.0.pos.as_ref() else {
            continue;
        };
        let elevation = position.z.unwrap_or(0.0) * tile_size;
        let bar_y = elevation + billboard.height / 2.0;
        let (height_offset, fill_ratio, enabled) = match billboard.part {
            Unit3dPart::Card => (0.0, 1.0, true),
            Unit3dPart::HealthBackground => (2.0, 1.0, true),
            Unit3dPart::HealthFill => (2.0, health.current / health.max.max(1.0), true),
            Unit3dPart::ShieldBackground => (4.0, 1.0, shield.is_some()),
            Unit3dPart::ShieldFill => (
                4.0,
                shield.map_or(0.0, |value| value.current / value.max.max(1.0)),
                shield.is_some(),
            ),
            Unit3dPart::BuildBackground => (6.0, 1.0, build_progress.is_some()),
            Unit3dPart::BuildFill => (
                6.0,
                build_progress.map_or(0.0, |value| value.0),
                build_progress.is_some(),
            ),
        };
        let is_fill = matches!(
            billboard.part,
            Unit3dPart::HealthFill | Unit3dPart::ShieldFill | Unit3dPart::BuildFill
        );
        let is_card = matches!(billboard.part, Unit3dPart::Card);
        let active_ratio = fill_ratio.clamp(0.0, 1.0);
        let part_y = if is_card {
            elevation
        } else {
            bar_y + height_offset
        };
        let part_y = if is_card {
            part_y + billboard.height / 2.0
        } else {
            part_y
        };

        transform.translation = Vec3::new(
            if is_fill {
                -billboard.width * (1.0 - active_ratio) / 2.0
            } else {
                0.0
            },
            part_y - unit_transform.translation.y,
            unit_transform.translation.y - unit_transform.translation.z,
        );
        transform.rotation = camera_rotation;
        transform.scale = Vec3::new(
            if is_fill { active_ratio } else { 1.0 },
            if enabled { 1.0 } else { 0.0 },
            1.0,
        );
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
            }
            ReflectFieldRef::Repeated(r) => {
                if r.len() > 0 {
                    let mut items = Vec::new();
                    for i in 0..r.len() {
                        let v = r.get(i).as_ref();
                        items.push(format!("{:?}", v));
                    }
                    result.push((field.name().to_string(), format!("[{}]", items.join(", "))));
                }
            }
            _ => continue,
        }
    }
    result
}

pub fn unit_selection_2d(
    windows: Query<&Window>,
    camera_query: Query<(&Camera, &GlobalTransform), With<Camera2d>>,
    unit_query: Query<(Entity, &Transform, &UnitTag, &UnitProto)>,
    mouse_button_input: Res<ButtonInput<MouseButton>>,
    mut selected: ResMut<SelectedUnit>,
    entity_system: Res<EntitySystem>,
) {
    if !mouse_button_input.just_pressed(MouseButton::Left) {
        return;
    }

    let window = windows.single().unwrap();
    let Ok((camera, camera_transform)) = camera_query.single() else {
        return;
    };

    let Some(cursor_pos) = window.cursor_position() else {
        return;
    };

    let Ok(world_pos) = camera.viewport_to_world(camera_transform, cursor_pos) else {
        return;
    };
    let world_pos = world_pos.origin.truncate();
    for (_entity, transform, tag, _) in &unit_query {
        let unit_pos = transform.translation.truncate();
        if unit_pos.distance(world_pos) < entity_system.map_config.tile_size {
            selected.tag = Some(tag.0);
            break;
        }
    }
}

pub fn unit_selection_3d(
    windows: Query<&Window>,
    camera_query: Query<(&Camera, &GlobalTransform), With<Camera3d>>,
    unit_query: Query<(&UnitTag, &UnitProto)>,
    mouse_button_input: Res<ButtonInput<MouseButton>>,
    mut selected: ResMut<SelectedUnit>,
    entity_system: Res<EntitySystem>,
    map_resource: Option<Res<MapResource>>,
) {
    if !mouse_button_input.just_pressed(MouseButton::Left) {
        return;
    }

    let Ok((camera, camera_transform)) = camera_query.single() else {
        return;
    };
    let Ok(window) = windows.single() else {
        return;
    };
    let Some(cursor_pos) = window.cursor_position() else {
        return;
    };
    let Some(map_resource) = map_resource else {
        return;
    };
    let map_size = map_resource.static_layers.get_dimensions();
    let tile_size = entity_system.map_config.tile_size;

    for (tag, proto) in &unit_query {
        let Some(position) = proto.0.pos.as_ref() else {
            continue;
        };
        let world_position = map_position_3d(
            position.x.unwrap_or(0.0),
            position.y.unwrap_or(0.0),
            position.z.unwrap_or(0.0),
            map_size,
            tile_size,
        );
        let Ok(screen_position) = camera.world_to_viewport(camera_transform, world_position) else {
            continue;
        };
        let radius = proto.0.radius.unwrap_or(1.0) * tile_size;
        let projected_radius = camera
            .world_to_viewport(camera_transform, world_position + Vec3::X * radius)
            .map_or(14.0, |edge| edge.distance(screen_position));
        if cursor_pos.distance(screen_position) <= projected_radius.max(14.0) {
            selected.tag = Some(tag.0);
            break;
        }
    }
}

pub fn draw_unit_orders_2d(
    mut gizmos: Gizmos,
    unit_query: Query<(&Transform, &UnitProto)>,
    registry: Res<UnitRegistry>,
    entity_system: Res<EntitySystem>,
    unit_visibility: Res<UnitCompositionVisibility>,
    map_resource: Option<Res<MapResource>>,
) {
    if !unit_visibility.show_orders {
        return;
    }

    use sc2_proto::raw::UnitOrder_oneof_target;

    let tile_size = entity_system.map_config.tile_size;
    let map_size = map_resource
        .as_ref()
        .map(|map| map.static_layers.get_dimensions())
        .unwrap_or((200, 176));

    for (transform, proto) in unit_query
        .iter()
        .filter(|(_, proto)| !proto.0.orders.is_empty())
    {
        // Get the first order if it exists
        let order = proto.0.orders.first().unwrap();
        let start_pos = Vec2::new(transform.translation.x, transform.translation.y);

        // Check if the order has a target using the oneof enum
        match order.target.as_ref() {
            Some(UnitOrder_oneof_target::target_world_space_pos(point)) => {
                // Target is a position
                let target_x = point.x.unwrap_or(0.0);
                let target_y = point.y.unwrap_or(0.0);

                // Convert SC2 coordinates to world coordinates
                let world_x = target_x * tile_size - map_size.0 as f32 * tile_size / 2.0;
                let world_y = target_y * tile_size - map_size.1 as f32 * tile_size / 2.0;
                let end_pos = Vec2::new(world_x, world_y);

                // Draw dashed line to position target
                draw_dashed_line(
                    &mut gizmos,
                    start_pos,
                    end_pos,
                    Color::srgba(0.8, 0.8, 0.2, 0.6),
                );

                // Draw small circle at target position
                gizmos.circle_2d(end_pos, 4.0, Color::srgba(1.0, 1.0, 0.3, 0.7));
            }
            Some(UnitOrder_oneof_target::target_unit_tag(target_tag)) => {
                // Target is another unit
                let Some(&target_entity) = registry.map.get(target_tag) else {
                    continue;
                };

                let Ok((target_transform, _)) = unit_query.get(target_entity) else {
                    continue;
                };

                let end_pos = Vec2::new(
                    target_transform.translation.x,
                    target_transform.translation.y,
                );

                // Draw solid line to unit target
                gizmos.line_2d(start_pos, end_pos, Color::srgba(0.2, 0.8, 0.8, 0.7));

                // Draw a small arrow head
                draw_arrow_head(
                    &mut gizmos,
                    start_pos,
                    end_pos,
                    Color::srgba(0.2, 0.8, 0.8, 0.7),
                );
            }
            _ => continue,
        }
    }
}

pub fn draw_unit_orders_3d(
    mut gizmos: Gizmos,
    unit_query: Query<&UnitProto>,
    registry: Res<UnitRegistry>,
    entity_system: Res<EntitySystem>,
    unit_visibility: Res<UnitCompositionVisibility>,
    map_resource: Option<Res<MapResource>>,
) {
    if !unit_visibility.show_orders {
        return;
    }
    let Some(map_resource) = map_resource else {
        return;
    };
    use sc2_proto::raw::UnitOrder_oneof_target;

    let map_size = map_resource.static_layers.get_dimensions();
    let tile_size = entity_system.map_config.tile_size;
    for proto in unit_query.iter().filter(|proto| !proto.0.orders.is_empty()) {
        let Some(position) = proto.0.pos.as_ref() else {
            continue;
        };
        let start = map_position_3d(
            position.x.unwrap_or(0.0),
            position.y.unwrap_or(0.0),
            position.z.unwrap_or(0.0),
            map_size,
            tile_size,
        );
        let order = proto.0.orders.first().unwrap();
        match order.target.as_ref() {
            Some(UnitOrder_oneof_target::target_world_space_pos(point)) => {
                let end = map_position_3d(
                    point.x.unwrap_or(0.0),
                    point.y.unwrap_or(0.0),
                    point.z.unwrap_or(0.0),
                    map_size,
                    tile_size,
                );
                gizmos.line(start, end, Color::srgba(0.8, 0.8, 0.2, 0.8));
                gizmos.line(
                    end - Vec3::X * 3.0,
                    end + Vec3::X * 3.0,
                    Color::srgba(1.0, 1.0, 0.3, 0.9),
                );
            }
            Some(UnitOrder_oneof_target::target_unit_tag(target_tag)) => {
                let Some(&target_entity) = registry.map.get(target_tag) else {
                    continue;
                };
                let Ok(target_proto) = unit_query.get(target_entity) else {
                    continue;
                };
                let Some(target_position) = target_proto.0.pos.as_ref() else {
                    continue;
                };
                let end = map_position_3d(
                    target_position.x.unwrap_or(0.0),
                    target_position.y.unwrap_or(0.0),
                    target_position.z.unwrap_or(0.0),
                    map_size,
                    tile_size,
                );
                gizmos.line(start, end, Color::srgba(0.2, 0.8, 0.8, 0.8));
            }
            _ => {}
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
