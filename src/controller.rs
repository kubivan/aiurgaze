//! Controller module: sets up proxy channels and handles pipeline events.
//!
//! This module orchestrates the reactive observation pipeline:
//! 1. Creates ProxyDataChannels for each bot
//! 2. Merges their response streams with vision mode filtering
//! 3. Emits Bevy events for observations and game info

use bevy::asset::{AssetServer, Assets, RenderAssetUsages};
use bevy::image::{Image, ImageSampler};
use bevy::prelude::{Commands, Handle, Local, Message, MessageReader, Query, Res, ResMut, Resource};
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy_ecs_tilemap::prelude::{TileColor, TileStorage};
use bevy_ecs_tilemap::tiles::TilePos;
use bevy_tokio_tasks::TokioTasksRuntime;
use sc2_proto::sc2api::{Request, ResponseGameInfo, ResponseObservation};
use tokio::sync::watch;
use tokio_stream::StreamExt;

use crate::app_settings::AppSettings;
use crate::entity_system::EntitySystem;
use crate::map::{blend_tile_color, spawn_tilemap, TerrainLayer, TerrainLayers};
use crate::observation_pipeline::{
    create_game_info_stream, create_observation_stream, TaggedResponseStream, VisionMode,
};
use crate::proxy_channel::{
    CreateGameSignal, JoinResponseBarrier, MultiplayerPorts, PlayerId, ProxyDataChannel,
    ProxyReadySignal,
};
use crate::units::{handle_observation, ObservationUnitTags, UnitBuildProgress, UnitRegistry};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicBool, Ordering};

static LOGGED_VIS_FORMAT: AtomicBool = AtomicBool::new(false);

/// Event emitted when an observation is received from the pipeline.
#[derive(Message, Clone)]
pub struct ObservationEvent {
    pub player_id: PlayerId,
    pub observation: ResponseObservation,
    pub vision_mode: VisionMode,
}

/// Event emitted when game info is received.
#[derive(Message, Clone)]
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

/// Handle to the fog-of-war GPU texture kept alive as a Bevy asset.
#[derive(Resource)]
pub struct FogOfWarHandle {
    pub handle: Handle<Image>,
}

/// CPU-side fog-of-war visibility data written each observation tick.
/// A separate system flushes this to `Assets<Image>` when `dirty`.
#[derive(Resource, Default)]
pub struct FogOfWarData {
    pub data: Vec<u8>,
    pub dirty: bool,
}

/// Handle to the `FogOfWarMaterial` asset so we can invalidate its bind group
/// whenever the underlying fog texture is re-uploaded.
#[derive(Resource)]
pub struct FogMaterialHandle {
    pub handle: Handle<crate::fog_material::FogOfWarMaterial>,
}

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
    let upstream_addr1 = format!("{}:{}/sc2api", upstream_url, upstream_port);
    // For multiplayer, Player2 connects to a SEPARATE SC2 instance on upstream_port+1
    let upstream_addr2 = format!("{}:{}/sc2api", upstream_url, upstream_port + 1);

    // Create Player1 proxy channel (each gets its own upstream URL)
    let listen_addr1 = format!("{}:{}", listen_url, base_port);
    let (channel1, _rx1) = ProxyDataChannel::new(
        PlayerId::Player1,
        listen_addr1.clone(),
        upstream_addr1.clone(),
    );

    // Create Player2 proxy channel if VsBot mode — connects to the 2nd SC2 instance
    let (channel2, p2_gi_stream, p2_obs_stream) = if is_vs_bot {
        let listen_addr2 = format!("{}:{}", listen_url, base_port + 1);
        let (ch, _rx) =
            ProxyDataChannel::new(PlayerId::Player2, listen_addr2, upstream_addr2.clone());
        let gi: TaggedResponseStream = Box::pin(ch.response_stream());
        let obs: TaggedResponseStream = Box::pin(ch.response_stream());
        (Some(ch), Some(gi), Some(obs))
    } else {
        (None, None, None)
    };

    // Spawn game_info consumer (cold — fires once per player at startup)
    let p1_gi: TaggedResponseStream = Box::pin(channel1.response_stream());
    runtime.spawn_background_task(move |ctx| async move {
        let gi_stream = create_game_info_stream(p1_gi, p2_gi_stream);
        tokio::pin!(gi_stream);

        while let Some(tagged_gi) = gi_stream.next().await {
            let mut ctx_clone = ctx.clone();
            let gi_event = GameInfoEvent {
                player_id: tagged_gi.player_id,
                game_info: tagged_gi.game_info,
            };
            tokio::spawn(async move {
                ctx_clone
                    .run_on_main_thread(move |ctx| {
                        ctx.world.send_event(gi_event);
                    })
                    .await;
            });
        }
        println!("GameInfo stream finished");
    });

    // Spawn observation consumer (hot — continuous during game)
    let p1_obs: TaggedResponseStream = Box::pin(channel1.response_stream());
    runtime.spawn_background_task(move |ctx| async move {
        let obs_stream = create_observation_stream(p1_obs, p2_obs_stream, vision_mode_rx);
        tokio::pin!(obs_stream);

        while let Some(tagged_obs) = obs_stream.next().await {
            let mut ctx_clone = ctx.clone();
            let obs_event = ObservationEvent {
                player_id: tagged_obs.player_id,
                observation: tagged_obs.observation,
                vision_mode: tagged_obs.vision_mode,
            };
            tokio::spawn(async move {
                ctx_clone
                    .run_on_main_thread(move |ctx| {
                        ctx.world.send_event(obs_event);
                    })
                    .await;
            });
        }
        println!("Observation stream finished");
    });

    // Spawn independent proxy tasks — each with its own upstream WS to SC2.
    // Only coordination: host sends CreateGame first, then signals guest.
    let ready_signal1 = ready_signal.clone();
    let ready_signal2 = ready_signal.clone();

    if is_vs_bot {
        let create_game_signal = CreateGameSignal::new();
        let cg_signal1 = create_game_signal.clone();
        let cg_signal2 = create_game_signal;
        let channel2 = channel2.unwrap();
        let join_barrier = JoinResponseBarrier::new(2);
        let barrier1 = Some(join_barrier.clone());
        let barrier2 = Some(join_barrier);

        // Compute multiplayer ports for SC2 internal game sync.
        // These are TCP ports that the two SC2 instances use for their internal
        // LAN protocol. They are separate from both the WebSocket ports and the
        // proxy listen ports. We pick upstream_port+2..+7 and map them through
        // Docker so both SC2 processes can reach them.
        // Layout: server=(up+2, up+3), client1=(up+4, up+5), client2=(up+6, up+7)
        let mp = MultiplayerPorts::from_base(upstream_port + 2, 2);
        println!("[setup_proxies] Multiplayer ports: {:?}", mp);
        let mp1 = Some(mp.clone());
        let mp2 = Some(mp);

        // Host (Player1): CreateGame → signal → JoinGame → bridge
        runtime.spawn_background_task(move |_ctx| async move {
            let Some(cg_req) = create_game_request else {
                eprintln!("[Player1] No CreateGame request for host mode");
                return;
            };
            if let Err(e) = channel1
                .run_host(ready_signal1, cg_signal1, barrier1, cg_req, mp1)
                .await
            {
                eprintln!("[Player1] Proxy failed: {e}");
            }
        });

        // Guest (Player2): wait signal → JoinGame → bridge
        runtime.spawn_background_task(move |_ctx| async move {
            if let Err(e) = channel2
                .run_guest(ready_signal2, cg_signal2, barrier2, mp2)
                .await
            {
                eprintln!("[Player2] Proxy failed: {e}");
            }
        });
    } else {
        // Solo (VsAI): CreateGame → JoinGame → bridge
        runtime.spawn_background_task(move |_ctx| async move {
            let Some(cg_req) = create_game_request else {
                eprintln!("[Player1] No CreateGame request for solo mode");
                return;
            };
            if let Err(e) = channel1.run_solo(ready_signal1, cg_req).await {
                eprintln!("[Player1] Proxy failed: {e}");
            }
        });
    }

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

/// Update tilemap colors for static layers only. Fog of war is now handled by a Bevy overlay.
fn update_tilemap_colors(
    tile_storage: &TileStorage,
    static_layers: &TerrainLayers,
    creep_layer: Option<&TerrainLayer>,
    energy_layer: Option<&TerrainLayer>,
    _visibility_layer: Option<&TerrainLayer>,
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

            // Get dynamic layer values (creep/energy only)
            let creep = creep_layer.map_or(0, |l| l.get_value(x, y));
            let energy = energy_layer.map_or(0, |l| l.get_value(x, y));

            // Blend colors using map config from entity system (no fog/visibility blending)
            let color = blend_tile_color(
                pathing,
                placement,
                creep,
                energy,
                1, // Always visible for base tile color; fog is handled by overlay
                height_val,
                &entity_system.map_config,
            );

            tile_color.0 = color;
        }
    }
}

fn handle_vision_mode_change(
    current_mode: VisionMode,
    last_vision_mode: &mut ResMut<LastVisionMode>,
    commands: &mut Commands,
    registry: &mut ResMut<UnitRegistry>,
    seen_tags: &mut ResMut<ObservationUnitTags>,
) {
    if let Some(prev_mode) = last_vision_mode.0 {
        if prev_mode != current_mode {
            println!(
                "Vision mode changed from {:?} to {:?}, despawning all entities",
                prev_mode, current_mode
            );

            for (_tag, entity) in registry.map.drain() {
                commands.entity(entity).despawn();
            }

            seen_tags.seen_tags.clear();
        }
    }

    last_vision_mode.0 = Some(current_mode);
}

fn update_map_from_observation(
    obs: &ResponseObservation,
    map_res: &mut Option<ResMut<MapResource>>,
    tile_color_query: &mut Query<&mut TileColor>,
    entity_system: &Res<EntitySystem>,
    fog_data: &mut Option<ResMut<FogOfWarData>>,
) -> Option<()> {
    let map_res = map_res.as_mut()?;
    let map_state = obs
        .observation
        .as_ref()?
        .raw_data
        .as_ref()?
        .map_state
        .as_ref()?;

    let creep_layer = map_state
        .creep
        .as_ref()
        .map(|creep_data| TerrainLayer::from_image_data(creep_data));

    if let Some(vis_data) = map_state.visibility.as_ref() {
        if !LOGGED_VIS_FORMAT.swap(true, Ordering::Relaxed) {
            let bits = vis_data.bits_per_pixel.unwrap_or_default();
            let data_len = vis_data.data.as_ref().map(|v| v.len()).unwrap_or_default();
            let size = vis_data.size.as_ref();
            let w = size.and_then(|s| s.x).unwrap_or_default();
            let h = size.and_then(|s| s.y).unwrap_or_default();
            eprintln!(
                "[fog] visibility format: bpp={bits}, bytes={data_len}, size={}x{}",
                w, h
            );
        }
    }

    let visibility_layer = map_state
        .visibility
        .as_ref()
        .map(|vis_data| TerrainLayer::from_image_data(vis_data));

    let new_creep_hash = calculate_layer_hash(&creep_layer);
    let new_visibility_hash = calculate_layer_hash(&visibility_layer);
    let new_energy_hash = 0;

    if new_creep_hash != map_res.last_creep_hash
        || new_energy_hash != map_res.last_energy_hash
        || new_visibility_hash != map_res.last_visibility_hash
    {
        update_tilemap_colors(
            &map_res.tile_storage,
            &map_res.static_layers,
            creep_layer.as_ref(),
            None,
            visibility_layer.as_ref(),
            tile_color_query,
            entity_system,
        );

        map_res.last_creep_hash = new_creep_hash;
        map_res.last_energy_hash = new_energy_hash;
        map_res.last_visibility_hash = new_visibility_hash;
    }

    // Always write latest visibility to fog texture (independent of hash gate)
    // so fog updates reliably each observation tick.
    if let Some(fog) = fog_data {
        match &visibility_layer {
            Some(vis_layer) => {
                let (w, h) = (vis_layer.width, vis_layer.height);
                let mut data = Vec::with_capacity((w * h) as usize);
                let mut n_fogged = 0u32;
                let mut n_visible = 0u32;
                for y in 0..h {
                    for x in 0..w {
                        let v = vis_layer.get_value(x, y);
                        if v > 0 {
                            data.push(255u8);
                            n_visible += 1;
                        } else {
                            data.push(0u8);
                            n_fogged += 1;
                        }
                    }
                }
                // One-time summary so we can verify data has variation.
                static LOGGED_FOG_SUMMARY: AtomicBool = AtomicBool::new(false);
                if !LOGGED_FOG_SUMMARY.swap(true, Ordering::Relaxed) {
                    eprintln!(
                        "[fog] first fog write: {}x{}, fogged={n_fogged}, visible={n_visible}, total={}",
                        w, h, data.len()
                    );
                }
                fog.data = data;
                fog.dirty = true;
            }
            None => {}
        }
    }

    Some(())
}

fn handle_units_for_observation(
    commands: &mut Commands,
    asset_server: &Res<AssetServer>,
    registry: &mut ResMut<UnitRegistry>,
    entity_system: &Res<EntitySystem>,
    obs: &ResponseObservation,
    unit_query: Query<&UnitBuildProgress>,
    seen_tags: &mut ResMut<ObservationUnitTags>,
    map_res: &Option<ResMut<MapResource>>,
) {
    let map_size = map_res.as_ref().map(|m| {
        let (width, height) = m.static_layers.get_dimensions();
        (width as f32, height as f32)
    });

    handle_observation(
        commands,
        asset_server,
        registry,
        entity_system,
        obs,
        unit_query,
        seen_tags,
        map_size,
    );
}

/// System to initialise the tilemap from the first valid GameInfoEvent.
///
/// Runs every frame until `MapResource` exists, then is skipped by the
/// `run_if` condition.  In vsBot mode two GameInfoEvents arrive in the same
/// tick; we drain them all but only act on the first one with `start_raw`.
pub fn map_init_system(
    mut gi_events: MessageReader<GameInfoEvent>,
    mut commands: Commands,
    mut asset_server: Res<AssetServer>,
    entity_system: Res<EntitySystem>,
    mut images: ResMut<Assets<Image>>,
) {
    // Drain all events in this batch; only process the first valid one.
    let events: Vec<_> = gi_events.read().collect();
    let Some(event) = events.iter().find(|e| e.game_info.start_raw.is_some()) else {
        return;
    };

    let gi = &event.game_info;
    let start_raw = gi.start_raw.as_ref().unwrap();
    let _start_pos = start_raw.start_locations.get(0);

    let path_layer = TerrainLayer::from_image_data(start_raw.pathing_grid.as_ref().unwrap());
    let placement_layer = TerrainLayer::from_image_data(start_raw.placement_grid.as_ref().unwrap());
    let height_layer = TerrainLayer::from_image_data(start_raw.terrain_height.as_ref().unwrap());

    println!(
        "[{}] Got game info: map size {} x {}",
        event.player_id, path_layer.width, path_layer.height
    );

    let static_layers = TerrainLayers::new(path_layer, placement_layer, height_layer);
    let tile_storage = spawn_tilemap(
        &mut commands,
        &static_layers,
        &mut asset_server,
        &entity_system.map_config,
    );
    commands.insert_resource(MapResource {
        static_layers: static_layers.clone(),
        tile_storage,
        last_creep_hash: 0,
        last_energy_hash: 0,
        last_visibility_hash: 0,
    });

    // Create the fog texture, add it to Assets<Image>, store the handle.
    let (width, height) = static_layers.get_dimensions();
    let fog_image = create_fog_of_war_texture(width, height);
    let handle = images.add(fog_image);
    commands.insert_resource(FogOfWarHandle { handle });
}

/// Utility: create a fog-of-war image (all pixels initially visible).
fn create_fog_of_war_texture(width: u32, height: u32) -> Image {
    let size = (width * height) as usize;
    let data = vec![255u8; size]; // 255 = fully visible; observations will carve fog
    let mut image = Image::new_fill(
        Extent3d { width, height, depth_or_array_layers: 1 },
        TextureDimension::D2,
        &data,
        TextureFormat::R8Unorm,
        RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
    );
    image.sampler = ImageSampler::nearest();
    image
}

/// System to handle observation events from the pipeline.
pub fn response_controller_system(
    mut obs_events: MessageReader<ObservationEvent>,
    mut last_vision_mode: ResMut<LastVisionMode>,
    mut map_res: Option<ResMut<MapResource>>,
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mut registry: ResMut<UnitRegistry>,
    entity_system: Res<EntitySystem>,
    mut tile_color_query: Query<&mut TileColor>,
    unit_query: Query<&UnitBuildProgress>,
    mut seen_tags: ResMut<ObservationUnitTags>,
    mut fog_data: Option<ResMut<FogOfWarData>>,
    mut logged_first_obs: Local<bool>,
) {
    // Rebuild seen tags once per frame across all observation events.
    // This prevents cross-event despawn races in VisionMode::All.
    seen_tags.seen_tags.clear();

    for event in obs_events.read() {
        let obs = &event.observation;

        // One-time diagnostic on first observation.
        if !*logged_first_obs {
            let has_vis = obs
                .observation.as_ref()
                .and_then(|o| o.raw_data.as_ref())
                .and_then(|r| r.map_state.as_ref())
                .and_then(|m| m.visibility.as_ref())
                .is_some();
            eprintln!(
                "[fog] first observation received. visibility layer present: {has_vis}. \
                 If false, enable raw interface in your bot's JoinGameRequest."
            );
            *logged_first_obs = true;
        }

        handle_vision_mode_change(
            event.vision_mode,
            &mut last_vision_mode,
            &mut commands,
            &mut registry,
            &mut seen_tags,
        );

        if update_map_from_observation(obs, &mut map_res, &mut tile_color_query, &entity_system, &mut fog_data)
            .is_none()
        {
            eprintln!(
                "[response_controller_system] Skipped map update: missing map resource or observation raw map_state"
            );
        }

        handle_units_for_observation(
            &mut commands,
            &asset_server,
            &mut registry,
            &entity_system,
            obs,
            unit_query,
            &mut seen_tags,
            &map_res,
        );
    }
}
