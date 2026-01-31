use bevy::asset::AssetServer;
use sc2_proto::sc2api::{Response, ResponseGameInfo, ResponseObservation, Response_oneof_response};
use bevy::prelude::{Commands, Res, ResMut, Resource, Query, Event, EventReader, EventWriter, Entity};
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

// Source of observation data (only player proxies now)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObservationSource {
    Player1,
    Player2,
}

impl Default for ObservationSource {
    fn default() -> Self {
        ObservationSource::Player1
    }
}

// Unified event for responses, with source tagging
// Only GameInfoEvent emitted (once after init)
// ObservationEvent emitted only for active source
#[derive(Event)]
pub struct GameInfoEvent {
    pub source: ObservationSource,
    pub game_info: ResponseGameInfo,
}

#[derive(Event)]
pub struct ObservationEvent {
    pub source: ObservationSource,
    pub observation: ResponseObservation,
}

#[derive(Event)]
pub struct SourceSwitchedEvent {
    pub from: ObservationSource,
    pub to: ObservationSource,
}

// Resource to track routing and map initialization
#[derive(Resource)]
pub struct ObservationRouter {
    pub active_source: ObservationSource,
    pub previous_source: Option<ObservationSource>,
    pub map_initialized: bool,
}

impl Default for ObservationRouter {
    fn default() -> Self {
        Self {
            active_source: ObservationSource::Player1,
            previous_source: None,
            map_initialized: false,
        }
    }
}

// Resource to store static terrain layers and tile storage
#[derive(Resource)]
pub struct MapResource {
    pub static_layers: TerrainLayers,
    pub tile_storage: TileStorage,
    pub tilemap_entity: Entity,
    pub last_creep_hash: u64,
    pub last_energy_hash: u64,
    pub last_visibility_hash: u64,
}

pub fn setup_proxy(
    runtime: &TokioTasksRuntime,
    settings: &AppSettings,
    source: ObservationSource,
    listen_port: u16,
) {
    println!("======setup_proxy({:?})====", source);

    let listen_addr = format!("{}:{}", settings.starcraft.listen_url, listen_port);
    let upstream_addr = format!("{}:{}/sc2api", settings.starcraft.upstream_url, settings.starcraft.upstream_port);
    let source_name = format!("{:?}", source);

    // Create proxy with callback that emits Bevy events directly
    runtime.spawn_background_task(move |ctx| async move {
        let ctx_for_callback = ctx.clone();
        let proxy = ProxyWS::new(
            source_name,
            &listen_addr,
            &upstream_addr,
            move |resp| {
                // This callback runs in the async task, so we need to queue the event
                // to be sent on the main thread
                let mut ctx_clone = ctx_for_callback.clone();
                tokio::spawn(async move {
                    ctx_clone.run_on_main_thread(move |ctx| {
                        // Get active source to check if we should emit this response
                        let active_source = ctx.world.resource::<ObservationRouter>().active_source;

                        // Emit GameInfo only once (ProxyWS sends after JoinedResponse)
                        if let Some(gi) = resp.response.as_ref().and_then(|r| {
                            match r {
                                Response_oneof_response::game_info(gi) => Some(gi.clone()),
                                _ => None,
                            }
                        }) {
                            ctx.world.send_event(GameInfoEvent {
                                source,
                                game_info: gi,
                            });
                        }

                        // Emit Observation only if this source is active
                        if source == active_source {
                            if let Some(obs) = resp.response.as_ref().and_then(|r| {
                                match r {
                                    Response_oneof_response::observation(obs) => Some(obs.clone()),
                                    _ => None,
                                }
                            }) {
                                ctx.world.send_event(ObservationEvent {
                                    source,
                                    observation: obs,
                                });
                            }
                        }
                    }).await;
                });
            }
        );

        if let Err(e) = proxy.run().await {
            eprintln!("Proxy task failed: {e}");
        }
    });

    println!("======Proxy task spawned ({:?})====", source);
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
    visibility_layer: Option<&TerrainLayer>,
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
            let pathing = static_layers.pathing.get_value(x, y);
            let placement = static_layers.placement.get_value(x, y);
            let height_val = static_layers.height.get_value(x, y);

            // Get dynamic layer values
            let creep = creep_layer.map_or(0, |l| l.get_value(x, y));
            let energy = energy_layer.map_or(0, |l| l.get_value(x, y));
            let visibility = visibility_layer.map_or(1, |l| l.get_value(x, y));

            // Blend colors using map config from entity system
            let color = blend_tile_color(pathing, placement, creep, energy, visibility, height_val, &entity_system.map_config);

            // Directly mutate the color component
            tile_color.0 = color;
        }
    }
}

pub fn route_proxy_responses(
    mut router: ResMut<ObservationRouter>,
    mut switch_events: EventWriter<SourceSwitchedEvent>,
) {
    // Check if active_source changed from previous_source
    if router.previous_source.is_none() {
        router.previous_source = Some(router.active_source);
    } else if router.previous_source != Some(router.active_source) {
        let from = router.previous_source.unwrap();
        let to = router.active_source;
        switch_events.write(SourceSwitchedEvent { from, to });
        router.previous_source = Some(router.active_source);
        println!("[route] Source switched from {:?} to {:?}", from, to);
    }
}

pub fn handle_source_switch(
    mut switch_events: EventReader<SourceSwitchedEvent>,
    mut commands: Commands,
    mut registry: ResMut<UnitRegistry>,
    mut seen_tags: ResMut<ObservationUnitTags>,
) {
    for event in switch_events.read() {
        println!("Observation source changed from {:?} to {:?}, despawning all entities", event.from, event.to);
        for (_tag, entity) in registry.map.drain() {
            commands.entity(entity).despawn_recursive();
        }
        seen_tags.seen_tags.clear();
    }
}

pub fn apply_game_info(
    mut events: EventReader<GameInfoEvent>,
    mut router: ResMut<ObservationRouter>,
    mut commands: Commands,
    mut asset_server: Res<AssetServer>,
    entity_system: Res<EntitySystem>,
) {
    if router.map_initialized {
        return;  // Only process once
    }

    for event in events.read() {
        if router.map_initialized {
            break;
        }

        let start_raw = match event.game_info.start_raw.as_ref() {
            Some(sr) => sr,
            None => {
                eprintln!("[apply_game_info] Missing start_raw from {:?}", event.source);
                continue;
            }
        };

        eprintln!("[apply_game_info] GameInfo received from {:?}: start_locations.len={}, has pathing={}, has placement={}, has height={}",
            event.source,
            start_raw.start_locations.len(),
            start_raw.pathing_grid.is_some(),
            start_raw.placement_grid.is_some(),
            start_raw.terrain_height.is_some()
        );

        let pathing_grid = match start_raw.pathing_grid.as_ref() {
            Some(pg) => pg,
            None => {
                eprintln!("[apply_game_info] Missing pathing_grid from {:?}", event.source);
                continue;
            }
        };

        let placement_grid = match start_raw.placement_grid.as_ref() {
            Some(pg) => pg,
            None => {
                eprintln!("[apply_game_info] Missing placement_grid from {:?}", event.source);
                continue;
            }
        };

        let terrain_height = match start_raw.terrain_height.as_ref() {
            Some(th) => th,
            None => {
                eprintln!("[apply_game_info] Missing terrain_height from {:?}", event.source);
                continue;
            }
        };

        let path_layer = TerrainLayer::from_image_data(pathing_grid);
        let placement_layer = TerrainLayer::from_image_data(placement_grid);
        let height_layer = TerrainLayer::from_image_data(terrain_height);

        println!("[apply_game_info] Map initialized: {} x {} from {:?}", path_layer.width, path_layer.height, event.source);

        let static_layers = TerrainLayers::new(path_layer, placement_layer, height_layer);

        let (tile_storage, tilemap_entity) = spawn_tilemap(
            &mut commands,
            &static_layers,
            &mut asset_server,
            &entity_system.map_config,
        );

        commands.insert_resource(MapResource {
            static_layers,
            tile_storage,
            tilemap_entity,
            last_creep_hash: 0,
            last_energy_hash: 0,
            last_visibility_hash: 0,
        });

        router.map_initialized = true;
        println!("[apply_game_info] Tilemap spawned from {:?}", event.source);
    }
}

pub fn apply_observation(
    mut events: EventReader<ObservationEvent>,
    mut switch_events: EventReader<SourceSwitchedEvent>,
    router: Res<ObservationRouter>,
    mut map_res: Option<ResMut<MapResource>>,
    mut commands: Commands,
    mut asset_server: Res<AssetServer>,
    mut registry: ResMut<UnitRegistry>,
    entity_system: Res<EntitySystem>,
    mut tile_color_query: Query<&mut TileColor>,
    unit_query: Query<&UnitBuildProgress>,
    mut seen_tags: ResMut<ObservationUnitTags>,
) {
    // Handle source switches (despawn all entities from old source)
    for event in switch_events.read() {
        println!("[apply_obs] Source switched from {:?} to {:?}, despawning entities", event.from, event.to);
        for (_tag, entity) in registry.map.drain() {
            commands.entity(entity).despawn_recursive();
        }
        seen_tags.seen_tags.clear();
    }

    // Process observations from active source
    for event in events.read() {
        let obs = &event.observation;
        println!("[apply_obs] Observation from {:?}", event.source);

        if map_res.is_none() {
            eprintln!("[apply_obs] Map not initialized yet, skipping observation");
            continue;
        }

        if let Some(ref mut map_res) = map_res {
            let raw_data = obs.observation.as_ref().unwrap().raw_data.as_ref().unwrap();
            let map_state = raw_data.map_state.as_ref();

            let creep_layer = map_state.and_then(|ms| ms.creep.as_ref()).map(|creep_data| {
                TerrainLayer::from_image_data(creep_data)
            });

            let visibility_layer = map_state.and_then(|ms| ms.visibility.as_ref()).map(|vis_data| {
                TerrainLayer::from_image_data(vis_data)
            });

            let new_creep_hash = calculate_layer_hash(&creep_layer);
            let new_visibility_hash = calculate_layer_hash(&visibility_layer);
            let new_energy_hash = 0;

            // Update tilemap colors if layers changed
            if new_creep_hash != map_res.last_creep_hash || new_energy_hash != map_res.last_energy_hash || new_visibility_hash != map_res.last_visibility_hash {
                update_tilemap_colors(
                    &map_res.tile_storage,
                    &map_res.static_layers,
                    creep_layer.as_ref(),
                    None,
                    visibility_layer.as_ref(),
                    &mut tile_color_query,
                    &entity_system,
                );

                map_res.last_creep_hash = new_creep_hash;
                map_res.last_energy_hash = new_energy_hash;
                map_res.last_visibility_hash = new_visibility_hash;
            }
        }

        // Update unit entities from observation
        handle_observation(
            &mut commands,
            &asset_server,
            &mut registry,
            &entity_system,
            obs,
            unit_query,
            &mut seen_tags,
            map_res.as_ref().map(|m| {
                let (w, h) = m.static_layers.get_dimensions();
                (w as f32, h as f32)
            }),
        );
    }
}
