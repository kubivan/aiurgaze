//! Controller module: sets up proxy channels and handles pipeline events.
//!
//! This module orchestrates the reactive observation pipeline:
//! 1. Creates ProxyDataChannels for each bot
//! 2. Merges their response streams with vision mode filtering
//! 3. Emits Bevy events for observations and game info

use bevy::asset::AssetServer;
use bevy::prelude::{Commands, Res, ResMut, Resource, Query, Event, EventReader};
use bevy_ecs_tilemap::prelude::{TileColor, TileStorage};
use bevy_ecs_tilemap::tiles::TilePos;
use bevy_tokio_tasks::TokioTasksRuntime;
use sc2_proto::sc2api::{Request, ResponseObservation, ResponseGameInfo};
use tokio::sync::watch;
use tokio_stream::StreamExt;

use crate::proxy_channel::{ProxyDataChannel, PlayerId, ProxyReadySignal, SharedUpstream, JoinBarrier};
use crate::observation_pipeline::{VisionMode, create_observation_pipeline, PipelineEvent};
use crate::map::{spawn_tilemap, TerrainLayers, TerrainLayer, blend_tile_color};
use crate::entity_system::EntitySystem;
use crate::units::{handle_observation, UnitBuildProgress, UnitRegistry, ObservationUnitTags};
use crate::app_settings::AppSettings;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

/// Event emitted when an observation is received from the pipeline.
#[derive(Event, Clone)]
pub struct ObservationEvent {
    pub player_id: PlayerId,
    pub observation: ResponseObservation,
    pub vision_mode: VisionMode,
}

/// Event emitted when game info is received.
#[derive(Event, Clone)]
pub struct GameInfoEvent {
    pub player_id: PlayerId,
    pub game_info: ResponseGameInfo,
}

/// Resource to store static terrain layers and tile storage.
#[derive(Resource)]
pub struct MapResource {
    pub static_layers: TerrainLayers,
    pub tile_storage: TileStorage,
    pub last_creep_hash: u64,
    pub last_energy_hash: u64,
    pub last_visibility_hash: u64,
}

/// Resource to track the last vision mode for detecting changes.
#[derive(Resource, Default)]
pub struct LastVisionMode(pub Option<VisionMode>);

/// Set up proxy channels and the observation pipeline.
///
/// Creates one or two proxy channels depending on game mode,
/// merges their streams with vision filtering, and emits Bevy events.
/// Returns a ProxyReadySignal that will be signaled when all proxies are ready.
pub fn setup_proxies(
    runtime: &TokioTasksRuntime,
    settings: &AppSettings,
    is_vs_bot: bool,
    vision_mode_rx: watch::Receiver<VisionMode>,
    create_game_request: Option<Request>,
) -> ProxyReadySignal {
    println!("====== setup_proxies (is_vs_bot={}) ======", is_vs_bot);

    // Create ready signal - expect 1 proxy for VsAI, 2 for VsBot
    let expected_proxies = if is_vs_bot { 2 } else { 1 };
    let ready_signal = ProxyReadySignal::new(expected_proxies);

    let base_port = settings.starcraft.listen_port;
    let listen_url = settings.starcraft.listen_url.clone();
    let upstream_url = settings.starcraft.upstream_url.clone();
    let upstream_port = settings.starcraft.upstream_port;
    let upstream_addr = format!("{}:{}/sc2api", upstream_url, upstream_port);

    // Create Player1 proxy channel
    let listen_addr1 = format!("{}:{}", listen_url, base_port);

    let (channel1, rx1) = ProxyDataChannel::new(
        PlayerId::Player1,
        listen_addr1.clone(),
    );

    // Create Player2 proxy channel if VsBot mode
    let (channel2, rx2) = if is_vs_bot {
        let listen_addr2 = format!("{}:{}", listen_url, base_port + 1);
        let (ch, rx) = ProxyDataChannel::new(
            PlayerId::Player2,
            listen_addr2,
        );
        (Some(ch), Some(rx))
    } else {
        (None, None)
    };

    // Spawn the pipeline consumer task
    runtime.spawn_background_task(move |ctx| async move {
        // Create the merged observation pipeline
        let pipeline = create_observation_pipeline(rx1, rx2, vision_mode_rx);
        tokio::pin!(pipeline);

        // Consume pipeline events and emit Bevy events
        while let Some(event) = pipeline.next().await {
            let mut ctx_clone = ctx.clone();
            match event {
                PipelineEvent::Observation(tagged_obs) => {
                    let obs_event = ObservationEvent {
                        player_id: tagged_obs.player_id,
                        observation: tagged_obs.observation,
                        vision_mode: tagged_obs.vision_mode,
                    };
                    tokio::spawn(async move {
                        ctx_clone.run_on_main_thread(move |ctx| {
                            ctx.world.send_event(obs_event);
                        }).await;
                    });
                }
                PipelineEvent::GameInfo(tagged_gi) => {
                    let gi_event = GameInfoEvent {
                        player_id: tagged_gi.player_id,
                        game_info: tagged_gi.game_info,
                    };
                    tokio::spawn(async move {
                        ctx_clone.run_on_main_thread(move |ctx| {
                            ctx.world.send_event(gi_event);
                        }).await;
                    });
                }
            }
        }
        println!("Pipeline consumer finished");
    });

    // Get the publish sender from Player1's channel for SharedUpstream to use
    let publish_sender = channel1.sender();

    // Create join barrier for bot-vs-bot: holds all communication until
    // every proxy has received its JoinGame response + silent GameInfo
    let join_barrier = if is_vs_bot {
        Some(JoinBarrier::new(2))
    } else {
        None
    };

    // Spawn a single task that creates the SharedUpstream and then runs both proxies
    let ready_signal1 = ready_signal.clone();
    let ready_signal2 = ready_signal.clone();
    let barrier1 = join_barrier.clone();
    let barrier2 = join_barrier;
    runtime.spawn_background_task(move |_ctx| async move {
        // Create SharedUpstream: single WS to SC2, sends CreateGame first
        // expected_joins: 1 for VsAI (only player joins), 2 for VsBot (both join)
        let expected_joins: u8 = if barrier1.is_some() { 2 } else { 1 };
        let upstream = match SharedUpstream::connect(
            &upstream_addr,
            create_game_request,
            publish_sender,
            expected_joins,
        ).await {
            Ok(u) => u,
            Err(e) => {
                eprintln!("[setup_proxies] Failed to connect SharedUpstream: {}", e);
                return;
            }
        };

        println!("[setup_proxies] SharedUpstream connected, spawning proxy tasks");

        // Spawn Player1 proxy task
        let upstream1 = upstream.clone();
        let p1_handle = tokio::spawn(async move {
            println!("[Player1] Starting proxy on {}", listen_addr1);
            if let Err(e) = channel1.run(Some(ready_signal1), upstream1, barrier1).await {
                eprintln!("[Player1] Proxy task failed: {}", e);
            }
        });

        // Spawn Player2 proxy task if VsBot
        let p2_handle = if let Some(channel2) = channel2 {
            let upstream2 = upstream.clone();
            Some(tokio::spawn(async move {
                println!("[Player2] Starting proxy");
                if let Err(e) = channel2.run(Some(ready_signal2), upstream2, barrier2).await {
                    eprintln!("[Player2] Proxy task failed: {}", e);
                }
            }))
        } else {
            None
        };

        // Wait for proxy tasks to finish
        let _ = p1_handle.await;
        if let Some(h) = p2_handle {
            let _ = h.await;
        }
    });

    println!("====== Proxy tasks spawned ======");
    ready_signal
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

/// System to handle observation events from the pipeline.
pub fn response_controller_system(
    mut obs_events: EventReader<ObservationEvent>,
    mut gi_events: EventReader<GameInfoEvent>,
    mut last_vision_mode: ResMut<LastVisionMode>,
    mut map_res: Option<ResMut<MapResource>>,
    mut commands: Commands,
    mut asset_server: Res<AssetServer>,
    mut registry: ResMut<UnitRegistry>,
    entity_system: Res<EntitySystem>,
    mut tile_color_query: Query<&mut TileColor>,
    unit_query: Query<&UnitBuildProgress>,
    mut seen_tags: ResMut<ObservationUnitTags>,
) {
    // Process game info events first (map initialization)
    for event in gi_events.read() {
        let gi = &event.game_info;
        let Some(start_raw) = gi.start_raw.as_ref() else {
            eprintln!("GameInfo missing start_raw");
            continue;
        };

        let start_pos = start_raw.start_locations.get(0);

        // Create static layers
        let path_layer = TerrainLayer::from_image_data(
            start_raw.pathing_grid.as_ref().unwrap());
        let placement_layer = TerrainLayer::from_image_data(
            start_raw.placement_grid.as_ref().unwrap());
        let height_layer = TerrainLayer::from_image_data(
            start_raw.terrain_height.as_ref().unwrap());

        println!("[{}] Got game info: map size {} x {}", 
                 event.player_id, path_layer.width, path_layer.height);

        // Build static layers container
        let static_layers = TerrainLayers::new(path_layer, placement_layer, height_layer);

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
            last_visibility_hash: 0,
        });

        println!("Spawned tilemap, start pos: {:?}", start_pos);
    }

    // Process observation events
    for event in obs_events.read() {
        let current_mode = event.vision_mode;
        
        // Check if vision mode changed - despawn all entities for fresh respawn
        if let Some(prev_mode) = last_vision_mode.0 {
            if prev_mode != current_mode {
                println!("Vision mode changed from {:?} to {:?}, despawning all entities", 
                         prev_mode, current_mode);
                
                // Despawn all entities
                for (_tag, entity) in registry.map.drain() {
                    commands.entity(entity).despawn_recursive();
                }
                
                // Reset seen tags
                seen_tags.seen_tags.clear();
            }
        }
        last_vision_mode.0 = Some(current_mode);

        let obs = &event.observation;

        // Update dynamic layers (creep, energy, visibility) only if changed
        if let Some(ref mut map_res) = map_res {
            if let Some(obs_data) = obs.observation.as_ref() {
                if let Some(raw_data) = obs_data.raw_data.as_ref() {
                    let map_state = raw_data.map_state.as_ref();

                    let creep_layer = map_state.and_then(|ms| ms.creep.as_ref()).map(|creep_data| {
                        TerrainLayer::from_image_data(creep_data)
                    });

                    let visibility_layer = map_state.and_then(|ms| ms.visibility.as_ref()).map(|vis_data| {
                        TerrainLayer::from_image_data(vis_data)
                    });

                    // Calculate hashes to check if layers changed
                    let new_creep_hash = calculate_layer_hash(&creep_layer);
                    let new_visibility_hash = calculate_layer_hash(&visibility_layer);
                    let new_energy_hash = 0;

                    // Only update if something changed
                    if new_creep_hash != map_res.last_creep_hash 
                        || new_energy_hash != map_res.last_energy_hash 
                        || new_visibility_hash != map_res.last_visibility_hash 
                    {
                        update_tilemap_colors(
                            &map_res.tile_storage,
                            &map_res.static_layers,
                            creep_layer.as_ref(),
                            None, // energy_layer
                            visibility_layer.as_ref(),
                            &mut tile_color_query,
                            &entity_system,
                        );

                        map_res.last_creep_hash = new_creep_hash;
                        map_res.last_energy_hash = new_energy_hash;
                        map_res.last_visibility_hash = new_visibility_hash;
                    }
                }
            }
        }

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
