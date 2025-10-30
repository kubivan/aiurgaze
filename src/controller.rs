use bevy::asset::AssetServer;
use sc2_proto::sc2api::{Response, Response_oneof_response::{game_info, observation}};
use bevy::prelude::{Commands, Res, ResMut, Resource, Query, Event, EventReader};
use bevy_ecs_tilemap::prelude::{TileColor, TileStorage};
use bevy_ecs_tilemap::tiles::TilePos;
use bevy_tokio_tasks::TokioTasksRuntime;
use crate::proxy_ws::ProxyWS;
use crate::map::{spawn_tilemap, TerrainLayers, TerrainLayer, blend_tile_color};
use crate::entity_system::EntitySystem;
use crate::units::{handle_observation, UnitBuildProgress, UnitRegistry, ObservationUnitTags};
use crate::app_settings::AppSettings;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

// Event for proxy responses
#[derive(Event)]
pub struct ProxyResponseEvent(pub Response);

// Resource to store static terrain layers and tile storage
#[derive(Resource)]
pub struct MapResource {
    pub static_layers: TerrainLayers,
    pub tile_storage: TileStorage,
    pub last_creep_hash: u64,
    pub last_energy_hash: u64,
}

pub fn setup_proxy(runtime: Res<TokioTasksRuntime>, settings: Res<AppSettings>) {
    println!("======setup_proxy====");

    let listen_addr = format!("{}:{}", settings.starcraft.listen_url, settings.starcraft.listen_port);
    let upstream_addr = format!("{}:{}/sc2api", settings.starcraft.upstream_url, settings.starcraft.upstream_port);

    // Create proxy with callback that emits Bevy events directly
    runtime.spawn_background_task(|ctx| async move {
        let proxy = ProxyWS::new(
            &listen_addr,
            &upstream_addr,
            move |resp| {
                // This callback runs in the async task, so we need to queue the event
                // to be sent on the main thread
                let mut ctx_clone = ctx.clone();
                tokio::spawn(async move {
                    ctx_clone.run_on_main_thread(move |ctx| {
                        ctx.world.send_event(ProxyResponseEvent(resp));
                    }).await;
                });
            }
        );

        if let Err(e) = proxy.run().await {
            eprintln!("Proxy task failed: {e}");
        }
    });

    println!("======Proxy task spawned====");
}


fn calculate_layer_hash(layer: &Option<TerrainLayer>) -> u64 {
    let mut hasher = DefaultHasher::new();
    if let Some(layer) = layer {
        layer.data.hash(&mut hasher);
    }
    hasher.finish()
}

fn update_tilemap_colors(
    tile_storage: &TileStorage,
    static_layers: &TerrainLayers,
    creep_layer: Option<&TerrainLayer>,
    energy_layer: Option<&TerrainLayer>,
    tile_color_query: &mut Query<&mut TileColor>,
    entity_system: &EntitySystem,
) {
    let (width, height) = static_layers.get_dimensions();

    for y in 0..height {
        for x in 0..width {
            let tile_pos = TilePos { x, y };

            let Some(tile_entity) = tile_storage.get(&tile_pos) else {
                continue;
            };
            
            let Ok(mut tile_color) = tile_color_query.get_mut(tile_entity) else {
                continue;
            };
            
            // Get static layer values
            let pathing = static_layers.pathing.as_ref().map_or(0, |l| l.get_value(x, y));
            let placement = static_layers.placement.as_ref().map_or(0, |l| l.get_value(x, y));
            let height_val = static_layers.height.as_ref().map_or(128, |l| l.get_value(x, y));

            // Get dynamic layer values
            let creep = creep_layer.map_or(0, |l| l.get_value(x, y));
            let energy = energy_layer.map_or(0, |l| l.get_value(x, y));

            // Blend colors using map config from entity system
            let color = blend_tile_color(pathing, placement, creep, energy, height_val, &entity_system.map_config);

            // Directly mutate the color component
            tile_color.0 = color;
        }
    }
}

pub fn response_controller_system(
    mut events: EventReader<ProxyResponseEvent>,
    mut map_res: Option<ResMut<MapResource>>,
    mut commands: Commands,
    mut asset_server: Res<AssetServer>,
    mut registry: ResMut<UnitRegistry>,
    entity_system: Res<EntitySystem>,
    mut tile_color_query: Query<&mut TileColor>,
    unit_query: Query<&UnitBuildProgress>,
    mut seen_tags: ResMut<ObservationUnitTags>,
) {
    for event in events.read() {
        match event.0.response.as_ref().unwrap() {
            observation (obs)  => {
                // Update dynamic layers (creep, energy, visibility) only if changed
                if let Some(ref mut map_res) = map_res {
                    let obs_data = obs.observation.as_ref().unwrap();
                    let raw_data = obs_data.raw_data.as_ref().unwrap();
                    let map_state = raw_data.map_state.as_ref();

                    let creep_layer = map_state.and_then(|ms| ms.creep.as_ref()).map(|creep_data| {
                        TerrainLayer::from_image_data(creep_data, crate::map::TerrainLayerKind::Creep)
                    });

                    // TODO: Extract energy layer when available
                    // let energy_layer = ...

                    // Calculate hashes to check if layers changed
                    let new_creep_hash = calculate_layer_hash(&creep_layer);
                    let new_energy_hash = 0; // TODO: calculate when energy layer is available

                    // Only update if something changed
                    if new_creep_hash != map_res.last_creep_hash || new_energy_hash != map_res.last_energy_hash {
                        // Update all tile colors with new dynamic data
                        update_tilemap_colors(
                            &map_res.tile_storage,
                            &map_res.static_layers,
                            creep_layer.as_ref(),
                            None, // energy_layer
                            &mut tile_color_query,
                            &entity_system,
                        );

                        // Update hashes
                        map_res.last_creep_hash = new_creep_hash;
                        map_res.last_energy_hash = new_energy_hash;
                    }
                }

                handle_observation(
                    &mut commands,
                    &asset_server,
                    &mut registry,
                    &entity_system,
                    &obs,
                    unit_query,
                    &mut seen_tags,
                    map_res.as_ref().map(|m| {
                        let (w, h) = m.static_layers.get_dimensions();
                        (w as f32, h as f32)
                    }).unwrap(),
                );

            }
            game_info (gi) =>  {
                let start_raw = gi.start_raw.as_ref().unwrap();
                let start_pos = start_raw.start_locations.get(0).unwrap();

                // Create static layers
                let path_layer = TerrainLayer::from_image_data(
                    start_raw.pathing_grid.as_ref().unwrap(),
                    crate::map::TerrainLayerKind::Pathing);
                let placement_layer = TerrainLayer::from_image_data(
                    start_raw.placement_grid.as_ref().unwrap(),
                    crate::map::TerrainLayerKind::Placement);
                let height_layer = TerrainLayer::from_image_data(
                    start_raw.terrain_height.as_ref().unwrap(),
                    crate::map::TerrainLayerKind::Height);

                println!("Got game info: map size {} x {}", path_layer.width, path_layer.height);

                // Build static layers container
                let mut static_layers = TerrainLayers::new();
                static_layers.add_layer(path_layer);
                static_layers.add_layer(placement_layer);
                static_layers.add_layer(height_layer);

                // Spawn the tilemap with initial static layers only
                let tile_storage = spawn_tilemap(
                    &mut commands,
                    &static_layers,
                    &mut asset_server,
                    &entity_system.map_config,
                );

                // Store the static layers and tile storage as a resource
                commands.insert_resource(MapResource {
                    static_layers,
                    tile_storage,
                    last_creep_hash: 0,
                    last_energy_hash: 0,
                });

                println!("Spawned tilemap, start pos: {:?}", start_pos);
            }
            _ => ()
        }
    }
}
