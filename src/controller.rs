//! Controller module: sets up proxy channels and handles pipeline events.
//!
//! This module orchestrates the reactive observation pipeline:
//! 1. Creates ProxyDataChannels for each bot
//! 2. Merges their response streams with vision mode filtering
//! 3. Emits Bevy events for observations and game info

use bevy::asset::{AssetServer, Assets, RenderAssetUsages};
use bevy::image::{Image, ImageSampler};
use bevy::mesh::Mesh;
use bevy::mesh::Mesh3d;
use bevy::pbr::{MeshMaterial3d, StandardMaterial};
use bevy::prelude::{
    default, Commands, DetectChanges, Handle, Local, Message, MessageReader, Query, Res, ResMut,
    Resource, Transform, Visibility,
};
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy_ecs_tilemap::prelude::{TileColor, TileStorage};
use bevy_ecs_tilemap::tiles::TilePos;
use bevy_tokio_tasks::TokioTasksRuntime;
use sc2_proto::sc2api::{Request, Request_oneof_request};
use tokio_stream::StreamExt;

use crate::app_settings::AppSettings;
use crate::entity_system::EntitySystem;
use crate::frame::{Frame, FrameEvent, FrameHistory, MapSnapshot, PlayerSnapshot, StaticMap};
use crate::map::{
    blend_tile_color, build_terrain_mesh, spawn_tilemap, Terrain3dMeshDirty, Terrain3dSettings,
    TerrainLayer, TerrainLayers,
};
use crate::sources::game::{create_game_info_stream, GameResponseStream, VisionMode};
use crate::proxy_channel::{
    create_observer_channel, observer_response_stream, CreateGameSignal, JoinResponseBarrier,
    MultiplayerPorts, PlayerId, ProxyDataChannel, ProxyReadySignal, TaggedResponse,
};
use crate::render_layers::{LayerRegistry, RenderLayerKind, RenderLayerMarker, ViewModeVisibility};
use crate::render_view::RenderViewMode;
use crate::ui::VisionModeChannel;
use crate::units::{
    apply_unit_snapshots, ObservationUnitTags, Unit3dRenderAssets, UnitBuildProgress, UnitRegistry,
};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[derive(Message, Clone)]
pub struct SaveReplayResponseEvent {
    pub data: Option<Vec<u8>>,
}

#[derive(Message)]
pub struct ReplayExportCompleteEvent {
    pub result: Option<Result<String, String>>,
}

#[derive(Message, Clone)]
pub struct ReplayStatusEvent {
    pub status: String,
}

#[derive(Message)]
pub struct ReplayFileLoadedEvent {
    pub result: Option<Result<Vec<u8>, String>>,
}

#[derive(Resource, Clone, Debug, Default)]
pub struct GameResultState {
    pub player_results: Vec<String>,
    pub status: Option<String>,
    pub replay_data: Option<Vec<u8>>,
}

pub fn save_replay_response_system(
    mut events: MessageReader<SaveReplayResponseEvent>,
    mut game_result: ResMut<GameResultState>,
    mut save_state: ResMut<crate::ui::ReplaySaveState>,
) {
    for event in events.read() {
        let Some(data) = event
            .data
            .as_ref()
            .filter(|data| !data.is_empty())
            .cloned()
        else {
            save_state.status = Some("SC2 could not prepare replay data.".to_string());
            continue;
        };

        game_result.replay_data = Some(data);
        save_state.status = Some("Replay ready to save.".to_string());
    }
}

pub fn replay_export_complete_system(
    mut events: MessageReader<ReplayExportCompleteEvent>,
    mut save_state: ResMut<crate::ui::ReplaySaveState>,
) {
    for event in events.read() {
        save_state.saving = false;
        if let Some(result) = &event.result {
            save_state.status = Some(match result {
                Ok(path) => format!("Replay saved to {path}"),
                Err(error) => format!("Could not save replay: {error}"),
            });
        }
    }
}

/// Event emitted when game info is received.
#[derive(Message, Clone)]
pub struct GameInfoEvent {
    pub player_id: PlayerId,
    pub static_map: StaticMap,
}

/// Event emitted for each proxied SC2 response to track protocol activity.
#[derive(Message, Clone)]
pub struct ProtocolActivityEvent {
    pub player_id: PlayerId,
    pub response_kind: String,
}

/// Playback mode for loop scrubbing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopPlaybackMode {
    Live,
    Paused,
    Scrubbing,
}

impl Default for LoopPlaybackMode {
    fn default() -> Self {
        Self::Live
    }
}

/// The currently selected/rendered frame. This is the view-facing state:
/// the renderer consumes only this single frame, regardless of whether it came
/// from the live proxy stream or from the replay history.
#[derive(Resource, Clone, Debug, Default)]
pub struct CurrentFrame {
    pub frame: Option<Frame>,
}

/// Stateful loop playback and replay history.
#[derive(Resource, Clone, Debug)]
pub struct LoopPlaybackState {
    pub mode: LoopPlaybackMode,
    pub selected_loop: u32,
    pub live_loop: u32,
    pub history: FrameHistory,
}

impl Default for LoopPlaybackState {
    fn default() -> Self {
        Self {
            mode: LoopPlaybackMode::Live,
            selected_loop: 0,
            live_loop: 0,
            history: FrameHistory::new(512),
        }
    }
}

impl LoopPlaybackState {
    pub fn current_loop(&self) -> u32 {
        if self.mode == LoopPlaybackMode::Live {
            self.live_loop
        } else {
            self.selected_loop
        }
    }

    pub fn pause(&mut self) {
        self.mode = LoopPlaybackMode::Paused;
        if self.selected_loop == 0 {
            self.selected_loop = self.live_loop;
        }
    }

    pub fn resume(&mut self) {
        self.mode = LoopPlaybackMode::Live;
        self.selected_loop = self.live_loop;
    }

    pub fn step_back(&mut self) {
        let loops = self.known_loops();
        if loops.is_empty() {
            return;
        }

        let current = if self.selected_loop > 0 {
            self.selected_loop
        } else {
            self.live_loop
        };
        let idx = loops
            .iter()
            .position(|loop_id| *loop_id == current)
            .unwrap_or(loops.len().saturating_sub(1));
        let next_idx = idx.saturating_sub(1).min(loops.len().saturating_sub(1));
        self.mode = LoopPlaybackMode::Paused;
        self.selected_loop = loops[next_idx];
    }

    pub fn step_forward(&mut self) {
        let loops = self.known_loops();
        if loops.is_empty() {
            return;
        }

        let current = if self.selected_loop > 0 {
            self.selected_loop
        } else {
            self.live_loop
        };
        let idx = loops
            .iter()
            .position(|loop_id| *loop_id == current)
            .unwrap_or(loops.len().saturating_sub(1));
        let next_idx = (idx + 1).min(loops.len().saturating_sub(1));
        self.mode = LoopPlaybackMode::Paused;
        self.selected_loop = loops[next_idx];
    }

    fn known_loops(&self) -> Vec<u32> {
        let mut loops = Vec::new();
        for loop_id in self
            .history
            .frames()
            .map(|frame| frame.game_loop)
            .filter(|loop_id| *loop_id > 0)
        {
            if loops.last() != Some(&loop_id) {
                loops.push(loop_id);
            }
        }
        loops
    }

    pub fn push_frame(&mut self, frame: Frame) {
        if frame.game_loop == 0 {
            return;
        }
        self.history.push_frame(frame.clone());
        self.live_loop = frame.game_loop;
        if self.mode == LoopPlaybackMode::Live {
            self.selected_loop = frame.game_loop;
        }
    }

    pub fn frame_for_loop(&self, loop_id: u32) -> Option<&Frame> {
        self.history
            .frames()
            .filter(|frame| frame.game_loop > 0)
            .filter(|frame| frame.game_loop <= loop_id)
            .last()
    }

    pub fn apply_live_frame(&self, frame: Frame, current: &mut CurrentFrame) {
        current.frame = Some(frame.clone());
    }

    pub fn apply_rewind_frame(&self, current: &mut CurrentFrame) {
        let Some(frame) = self.frame_for_loop(self.selected_loop).cloned() else {
            return;
        };
        current.frame = Some(frame);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loop_playback_selects_latest_known_frame() {
        let mut state = LoopPlaybackState::default();
        state
            .history
            .push_frame(Frame::new(crate::frame::FrameSource::Game, PlayerId::Player1, 10));
        state
            .history
            .push_frame(Frame::new(crate::frame::FrameSource::Game, PlayerId::Player1, 20));
        state
            .history
            .push_frame(Frame::new(crate::frame::FrameSource::Game, PlayerId::Player1, 30));

        state.selected_loop = 25;
        let frame = state.frame_for_loop(state.selected_loop).unwrap();
        assert_eq!(frame.game_loop, 20);
    }

    #[test]
    fn paused_replay_frame_keeps_its_debug_text() {
        let mut state = LoopPlaybackState::default();
        let mut frame = Frame::new(crate::frame::FrameSource::Replay, PlayerId::Player1, 20);
        frame
            .debug_draws
            .push(crate::debug_draw::DebugDrawPrimitive::Line {
                start: bevy::prelude::Vec3::ZERO,
                end: bevy::prelude::Vec3::X,
                color: bevy::prelude::Color::WHITE,
            });
        frame
            .debug_texts
            .push(crate::debug_draw::DebugTextPrimitive {
                text: "frame text".to_owned(),
                position: crate::debug_draw::DebugTextPosition::Virtual(bevy::prelude::Vec2::new(
                    0.5, 0.5,
                )),
                color: bevy::prelude::Color::WHITE,
                size: 8.0,
            });
        state.history.push_frame(frame);
        state.mode = LoopPlaybackMode::Paused;
        state.selected_loop = 20;

        let mut current = CurrentFrame::default();
        state.apply_rewind_frame(&mut current);

        assert_eq!(
            current.frame.as_ref().unwrap().debug_texts[0].text,
            "frame text"
        );
        assert!(matches!(
            &current.frame.as_ref().unwrap().debug_draws[0],
            crate::debug_draw::DebugDrawPrimitive::Line { start, end, .. }
                if *start == bevy::prelude::Vec3::ZERO && *end == bevy::prelude::Vec3::X
        ));
    }

    #[test]
    fn new_frame_keeps_latest_debug_draws_and_texts() {
        let mut previous = Frame::new(crate::frame::FrameSource::Game, PlayerId::Player1, 10);
        previous
            .debug_draws
            .push(crate::debug_draw::DebugDrawPrimitive::Line {
                start: bevy::prelude::Vec3::ZERO,
                end: bevy::prelude::Vec3::X,
                color: bevy::prelude::Color::WHITE,
            });
        previous
            .debug_texts
            .push(crate::debug_draw::DebugTextPrimitive {
                text: "persistent".to_owned(),
                position: crate::debug_draw::DebugTextPosition::Virtual(
                    bevy::prelude::Vec2::new(0.5, 0.5),
                ),
                color: bevy::prelude::Color::WHITE,
                size: 8.0,
            });

        let mut next = Frame::new(crate::frame::FrameSource::Game, PlayerId::Player1, 11);
        carry_forward_debug_draws(&mut next, Some(&previous));

        assert_eq!(next.debug_draws.len(), 1);
        assert_eq!(next.debug_texts.len(), 1);
        assert_eq!(next.debug_texts[0].text, "persistent");
    }

    #[test]
    fn loop_playback_navigates_unique_loops_and_resumes() {
        let mut state = LoopPlaybackState::default();
        for loop_id in [10, 10, 20, 20, 30, 30] {
            state.history.push_frame(Frame::new(
                crate::frame::FrameSource::Replay,
                PlayerId::Player1,
                loop_id,
            ));
        }
        state.live_loop = 30;
        state.selected_loop = 30;

        state.step_back();
        assert_eq!(state.selected_loop, 20);
        state.step_back();
        assert_eq!(state.selected_loop, 10);
        state.step_forward();
        assert_eq!(state.selected_loop, 20);

        state.resume();
        assert_eq!(state.mode, LoopPlaybackMode::Live);
        assert_eq!(state.selected_loop, 30);
    }
}

/// Live player stats extracted from each observation (minerals, vespene, supply, etc.).
#[derive(Resource, Default, Clone, Debug)]
pub struct PlayerResources {
    pub minerals: u32,
    pub vespene: u32,
    pub food_used: u32,
    pub food_cap: u32,
    pub army_count: u32,
    pub worker_count: u32,
    pub idle_workers: u32,
    pub game_loop: u32,
}

/// Latest protocol activity summary for status UI.
#[derive(Resource, Debug, Clone)]
pub struct ProtocolActivityState {
    pub player1_last: String,
    pub player2_last: String,
}

impl Default for ProtocolActivityState {
    fn default() -> Self {
        Self {
            player1_last: "idle".to_string(),
            player2_last: "idle".to_string(),
        }
    }
}

/// Resource to store static terrain layers and tile storage.
#[derive(Resource)]
pub struct MapResource {
    pub static_layers: TerrainLayers,
    pub tile_storage: TileStorage,
    pub terrain_mesh_handle: Handle<Mesh>,
    pub terrain_mesh_entity: bevy::prelude::Entity,
    pub last_creep_hash: u64,
    pub last_energy_hash: u64,
    pub last_visibility_hash: u64,
    pub latest_creep_layer: Option<TerrainLayer>,
    pub latest_energy_layer: Option<TerrainLayer>,
    pub latest_visibility_layer: Option<TerrainLayer>,
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

#[derive(Resource)]
pub struct Fog3dMaterialHandle {
    pub handle: Handle<crate::fog_material::FogOfWarMaterial3d>,
}

/// Start the configured game or replay source.
///
/// Creates one or two proxy channels depending on game mode,
/// Returns a ProxyReadySignal that will be signaled when all proxies are ready.
fn forward_messages<M>(
    runtime: &TokioTasksRuntime,
    mut messages: tokio::sync::mpsc::Receiver<M>,
) where
    M: Message + Send + Sync + 'static,
{
    runtime.spawn_background_task(move |mut ctx| async move {
        while let Some(message) = messages.recv().await {
            ctx.run_on_main_thread(move |ctx| {
                ctx.world.write_message(message);
            })
            .await;
        }
    });
}

fn forward_game_info(
    runtime: &TokioTasksRuntime,
    p1_stream: GameResponseStream,
    p2_stream: Option<GameResponseStream>,
) {
    runtime.spawn_background_task(move |mut ctx| async move {
        let mut game_info_stream = Box::pin(create_game_info_stream(p1_stream, p2_stream));
        while let Some(tagged_info) = game_info_stream.next().await {
            let Some(static_map) =
                crate::sources::game::static_map_from_info(&tagged_info.game_info)
            else {
                continue;
            };
            ctx.run_on_main_thread(move |ctx| {
                ctx.world.write_message(GameInfoEvent {
                    player_id: tagged_info.player_id,
                    static_map,
                });
            })
            .await;
        }
    });
}

fn forward_protocol_activity(
    runtime: &TokioTasksRuntime,
    mut responses: GameResponseStream,
    include_replay_events: bool,
) {
    runtime.spawn_background_task(move |mut ctx| async move {
        while let Some(tagged) = responses.next().await {
            let activity = ProtocolActivityEvent {
                player_id: tagged.player_id,
                response_kind: response_kind_name(&tagged.response),
            };
            let save_replay = (include_replay_events && tagged.response.has_save_replay())
                .then(|| SaveReplayResponseEvent {
                    data: tagged.response.get_save_replay().data.as_ref().cloned(),
                });
            let replay_status = (include_replay_events && tagged.response.has_start_replay())
                .then(|| tagged.response.get_start_replay())
                .filter(|response| response.has_error())
                .map(|response| ReplayStatusEvent {
                    status: format!(
                        "Could not start replay: {:?} - {}",
                        response.get_error(),
                        response.get_error_details()
                    ),
                });
            ctx.run_on_main_thread(move |ctx| {
                ctx.world.write_message(activity);
                if let Some(event) = save_replay {
                    ctx.world.write_message(event);
                }
                if let Some(event) = replay_status {
                    ctx.world.write_message(event);
                }
            })
            .await;
        }
    });
}

fn start_replay_proxy(
    runtime: &TokioTasksRuntime,
    channel: ProxyDataChannel,
    ready_signal: ProxyReadySignal,
    request: Option<Request>,
) {
    let Some(request) = request else {
        eprintln!("[Player1] No StartReplay request for replay mode");
        return;
    };
    runtime.spawn_background_task(move |_ctx| async move {
        if let Err(error) = crate::sources::replay::run(channel, ready_signal, request).await {
            eprintln!("[Player1] Replay failed: {error}");
        }
    });
}

fn start_multiplayer_proxies(
    runtime: &TokioTasksRuntime,
    host: ProxyDataChannel,
    guest: ProxyDataChannel,
    ready_host: ProxyReadySignal,
    ready_guest: ProxyReadySignal,
    create_game_request: Option<Request>,
    upstream_port: u16,
    debug_tx: tokio::sync::mpsc::Sender<crate::debug_draw::DebugDrawEvent>,
    chat_tx: tokio::sync::mpsc::Sender<crate::chat_overlay::ChatMessageEvent>,
    pause_state: Arc<AtomicBool>,
) {
    let create_game_signal = CreateGameSignal::new();
    let guest_signal = create_game_signal.clone();
    let join_barrier = JoinResponseBarrier::new(2);
    let host_barrier = Some(join_barrier.clone());
    let guest_barrier = Some(join_barrier);
    let multiplayer_ports = MultiplayerPorts::from_base(upstream_port + 2, 2);
    println!("[setup_proxies] Multiplayer ports: {:?}", multiplayer_ports);
    let host_ports = Some(multiplayer_ports.clone());
    let guest_ports = Some(multiplayer_ports);
    let host_debug_tx = debug_tx.clone();
    let host_chat_tx = chat_tx.clone();
    let host_pause = pause_state.clone();
    runtime.spawn_background_task(move |_ctx| async move {
        let Some(request) = create_game_request else {
            eprintln!("[Player1] No CreateGame request for host mode");
            return;
        };
        if let Err(error) = host
            .run_host(
                ready_host,
                create_game_signal,
                host_barrier,
                request,
                host_ports,
                Some(host_debug_tx),
                Some(host_chat_tx),
                Some(host_pause),
            )
            .await
        {
            eprintln!("[Player1] Proxy failed: {error}");
        }
    });

    runtime.spawn_background_task(move |_ctx| async move {
        if let Err(error) = guest
            .run_guest(
                ready_guest,
                guest_signal,
                guest_barrier,
                guest_ports,
                Some(debug_tx),
                Some(chat_tx),
                Some(pause_state),
            )
            .await
        {
            eprintln!("[Player2] Proxy failed: {error}");
        }
    });
}

fn start_solo_proxy(
    runtime: &TokioTasksRuntime,
    channel: ProxyDataChannel,
    ready_signal: ProxyReadySignal,
    request: Option<Request>,
    observer_sender: Option<tokio::sync::broadcast::Sender<TaggedResponse>>,
    debug_tx: tokio::sync::mpsc::Sender<crate::debug_draw::DebugDrawEvent>,
    chat_tx: tokio::sync::mpsc::Sender<crate::chat_overlay::ChatMessageEvent>,
    pause_state: Arc<AtomicBool>,
) {
    runtime.spawn_background_task(move |_ctx| async move {
        let Some(request) = request else {
            eprintln!("[Player1] No CreateGame request for solo mode");
            return;
        };
        if let Err(error) = channel
            .run_solo(
                ready_signal,
                request,
                observer_sender,
                Some(debug_tx),
                Some(chat_tx),
                Some(pause_state),
            )
            .await
        {
            eprintln!("[Player1] Proxy failed: {error}");
        }
    });
}

pub fn setup_proxies(
    runtime: &TokioTasksRuntime,
    settings: &AppSettings,
    is_vs_bot: bool,
    create_game_request: Option<Request>,
    pause_state: Arc<AtomicBool>,
) -> ProxyReadySignal {
    println!("====== setup_proxies (is_vs_bot={}) ======", is_vs_bot);
    let is_replay = create_game_request.as_ref().is_some_and(|request| {
        matches!(
            request.request,
            Some(Request_oneof_request::start_replay(_))
        )
    });

    // Create ready signal - expect 1 proxy for VsAI, 2 for VsBot
    let expected_proxies = if is_vs_bot { 2 } else { 1 };
    let ready_signal = ProxyReadySignal::new(expected_proxies);
    let (debug_tx, debug_rx) = tokio::sync::mpsc::channel(64);
    let (chat_tx, chat_rx) = tokio::sync::mpsc::channel(64);

    let base_port = settings.starcraft.listen_port;
    let listen_url = settings.starcraft.listen_url.clone();
    let upstream_url = settings.starcraft.upstream_url.clone();
    let upstream_port = settings.starcraft.upstream_port;
    let upstream_addr1 = format!("{}:{}/sc2api", upstream_url, upstream_port);
    // For multiplayer, Player2 connects to a SEPARATE SC2 instance on upstream_port+1
    let upstream_addr2 = format!("{}:{}/sc2api", upstream_url, upstream_port + 1);

    // Create Player1 proxy channel (each gets its own upstream URL)
    let listen_addr1 = format!("{}:{}", listen_url, base_port);
    let (channel1, _response_rx1, p1_frame_responses) = ProxyDataChannel::new(
        PlayerId::Player1,
        listen_addr1.clone(),
        upstream_addr1.clone(),
    );

    // Create Player2 proxy channel if VsBot mode — connects to the 2nd SC2 instance
    let (channel2, p2_gi_stream, p2_replay_responses) = if is_vs_bot {
        let listen_addr2 = format!("{}:{}", listen_url, base_port + 1);
        let (ch, _response_rx, replay_responses) =
            ProxyDataChannel::new(PlayerId::Player2, listen_addr2, upstream_addr2.clone());
        let gi: GameResponseStream = Box::pin(ch.response_stream());
        (Some(ch), Some(gi), Some(replay_responses))
    } else {
        (None, None, None)
    };

    // In VsAI mode, create an observer broadcast channel. The proxy will
    // interleave `disable_fog` observation requests in its bridge loop and
    // publish responses here, providing full-visibility data on the same
    // upstream WS connection (SC2 only allows one WS per port).
    let (observer_sender, obs_gi_stream) = if !is_vs_bot {
        let (sender, _rx) = create_observer_channel();
        let gi: GameResponseStream = Box::pin(observer_response_stream(&sender));
        (Some(sender), Some(gi))
    } else {
        (None, None)
    };

    if is_replay {
        crate::sources::replay::spawn_frames(runtime, p1_frame_responses);
    } else {
        crate::sources::game::spawn_frames(runtime, p1_frame_responses);
    }
    if let Some(p2_replay_responses) = p2_replay_responses {
        crate::sources::game::spawn_frames(runtime, p2_replay_responses);
    }

    // Merge P2 streams: either from VsBot channel2 or from VsAI observer
    let merged_p2_gi = p2_gi_stream.or(obs_gi_stream);
    forward_messages(runtime, debug_rx);
    forward_messages(runtime, chat_rx);

    // GameInfo supplies static map geometry to the renderer.
    let p1_gi: GameResponseStream = Box::pin(channel1.response_stream());
    forward_game_info(runtime, p1_gi, merged_p2_gi);

    // Spawn protocol activity consumer (all responses, independent from observation/game_info pipelines).
    let p1_activity: GameResponseStream = Box::pin(channel1.response_stream());
    forward_protocol_activity(runtime, p1_activity, true);

    if let Some(ref channel2_for_activity) = channel2 {
        let p2_activity: GameResponseStream = Box::pin(channel2_for_activity.response_stream());
        forward_protocol_activity(runtime, p2_activity, false);
    }

    if let Some(ref observer_sender_for_activity) = observer_sender {
        let obs_activity: GameResponseStream =
            Box::pin(observer_response_stream(observer_sender_for_activity));
        forward_protocol_activity(runtime, obs_activity, false);
    }

    // Spawn independent proxy tasks — each with its own upstream WS to SC2.
    // Only coordination: host sends CreateGame first, then signals guest.
    if is_replay {
        start_replay_proxy(runtime, channel1, ready_signal.clone(), create_game_request);
    } else if is_vs_bot {
        let channel2 = channel2.unwrap();
        start_multiplayer_proxies(
            runtime,
            channel1,
            channel2,
            ready_signal.clone(),
            ready_signal.clone(),
            create_game_request,
            upstream_port,
            debug_tx,
            chat_tx,
            pause_state,
        );
    } else {
        start_solo_proxy(
            runtime,
            channel1,
            ready_signal.clone(),
            create_game_request,
            observer_sender,
            debug_tx,
            chat_tx,
            pause_state,
        );
    }

    println!("====== Proxy tasks spawned ======");
    ready_signal
}

fn response_kind_name(response: &sc2_proto::sc2api::Response) -> String {
    let Some(oneof) = response.response.as_ref() else {
        return "unknown".to_string();
    };

    let full = format!("{oneof:?}");
    full.split('(').next().unwrap_or("unknown").to_string()
}

pub fn protocol_activity_system(
    mut events: MessageReader<ProtocolActivityEvent>,
    mut state: ResMut<ProtocolActivityState>,
) {
    for event in events.read() {
        match event.player_id {
            PlayerId::Player1 => state.player1_last = event.response_kind.clone(),
            PlayerId::Player2 => state.player2_last = event.response_kind.clone(),
        }
    }
}

/// Copies normalized source data into the HUD resources.
pub fn update_player_resources(
    mut events: MessageReader<FrameEvent>,
    mut res: ResMut<PlayerResources>,
    mut game_result: ResMut<GameResultState>,
    mut replay_save_state: ResMut<crate::ui::ReplaySaveState>,
    mut playback: ResMut<LoopPlaybackState>,
    vision_mode: Res<VisionModeChannel>,
) {
    for event in events.read() {
        if vision_mode.current != VisionMode::All
            && !matches!(
                (vision_mode.current, event.frame.player_id),
                (VisionMode::Player1, PlayerId::Player1)
                    | (VisionMode::Player2, PlayerId::Player2)
            )
        {
            continue;
        }
        if game_result.player_results.is_empty() && !event.frame.player_results.is_empty() {
            replay_save_state.status = Some("Preparing replay export...".to_string());
            game_result
                .player_results
                .clone_from(&event.frame.player_results);
        }

        let Some(player) = event.frame.player.as_ref() else {
            continue;
        };
        let loop_id = event.frame.game_loop;
        if playback.mode == LoopPlaybackMode::Live {
            res.game_loop = loop_id;
            playback.selected_loop = loop_id;
        } else {
            res.game_loop = playback.selected_loop;
        }

        apply_player_snapshot(player, &mut res);
    }
}

fn apply_player_snapshot(player: &PlayerSnapshot, resources: &mut PlayerResources) {
    resources.minerals = player.minerals;
    resources.vespene = player.vespene;
    resources.food_used = player.food_used;
    resources.food_cap = player.food_cap;
    resources.army_count = player.army_count;
    resources.worker_count = player.worker_count;
    resources.idle_workers = player.idle_workers;
}

pub fn replay_status_system(
    mut events: MessageReader<ReplayStatusEvent>,
    mut game_result: ResMut<GameResultState>,
) {
    for event in events.read() {
        game_result.status = Some(event.status.clone());
    }
}

/// Selects the single frame consumed by rendering.
pub fn current_frame_system(
    mut frame_events: MessageReader<FrameEvent>,
    mut playback: ResMut<LoopPlaybackState>,
    mut current_frame: ResMut<CurrentFrame>,
    vision_mode: Res<VisionModeChannel>,
) {
    for event in frame_events.read() {
        if vision_mode.current != VisionMode::All
            && !matches!(
                (vision_mode.current, event.frame.player_id),
                (VisionMode::Player1, PlayerId::Player1)
                    | (VisionMode::Player2, PlayerId::Player2)
            )
        {
            continue;
        }
        let mut frame = event.frame.clone();
        carry_forward_debug_draws(&mut frame, playback.history.latest_frame());
        playback.push_frame(frame.clone());
        if playback.mode == LoopPlaybackMode::Live {
            playback.apply_live_frame(frame, &mut current_frame);
        }
    }

    if playback.mode != LoopPlaybackMode::Live {
        playback.apply_rewind_frame(&mut current_frame);
    }
}

fn carry_forward_debug_draws(frame: &mut Frame, previous: Option<&Frame>) {
    if !frame.debug_draws.is_empty() || !frame.debug_texts.is_empty() {
        return;
    }

    let Some(previous) = previous else {
        return;
    };
    frame.debug_draws.clone_from(&previous.debug_draws);
    frame.debug_texts.clone_from(&previous.debug_texts);
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
    layer_registry: &LayerRegistry,
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
                layer_registry,
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

fn update_map_from_frame(
    map_snapshot: &MapSnapshot,
    map_res: &mut Option<ResMut<MapResource>>,
    tile_color_query: &mut Query<&mut TileColor>,
    entity_system: &Res<EntitySystem>,
    layer_registry: &Res<LayerRegistry>,
    fog_data: &mut Option<ResMut<FogOfWarData>>,
    terrain_mesh_dirty: &mut ResMut<Terrain3dMeshDirty>,
) -> Option<()> {
    let map_res = map_res.as_mut()?;
    let creep_layer = map_snapshot.creep.as_ref().map(TerrainLayer::from_frame_layer);
    let energy_layer = map_snapshot
        .energy
        .as_ref()
        .map(TerrainLayer::from_frame_layer);
    let visibility_layer = map_snapshot
        .visibility
        .as_ref()
        .map(TerrainLayer::from_frame_layer);

    let new_creep_hash = calculate_layer_hash(&creep_layer);
    let new_visibility_hash = calculate_layer_hash(&visibility_layer);
    let new_energy_hash = calculate_layer_hash(&energy_layer);

    map_res.latest_creep_layer = creep_layer.clone();
    map_res.latest_energy_layer = energy_layer.clone();
    map_res.latest_visibility_layer = visibility_layer.clone();

    if new_creep_hash != map_res.last_creep_hash
        || new_energy_hash != map_res.last_energy_hash
        || new_visibility_hash != map_res.last_visibility_hash
    {
        if new_creep_hash != map_res.last_creep_hash || new_energy_hash != map_res.last_energy_hash
        {
            terrain_mesh_dirty.0 = true;
        }
        update_tilemap_colors(
            &map_res.tile_storage,
            &map_res.static_layers,
            creep_layer.as_ref(),
            energy_layer.as_ref(),
            visibility_layer.as_ref(),
            tile_color_query,
            entity_system,
            layer_registry,
        );

        map_res.last_creep_hash = new_creep_hash;
        map_res.last_energy_hash = new_energy_hash;
        map_res.last_visibility_hash = new_visibility_hash;
    }

    // Always write latest visibility to fog texture (independent of hash gate)
    // so fog updates reliably each observation tick.
    //
    // MVP tri-state encoding (stored in R8 texture):
    //   0   = unexplored / hidden
    //   128 = explored but currently not visible
    //   255 = currently visible
    if let Some(fog) = fog_data {
        if let Some(vis_layer) = &visibility_layer {
            let (w, h) = (vis_layer.width, vis_layer.height);
            let mut data = Vec::with_capacity((w * h) as usize);
            let mut n_unexplored = 0u32;
            let mut n_explored = 0u32;
            let mut n_visible = 0u32;
            let mut n_other = 0u32;
            for y in 0..h {
                for x in 0..w {
                    let v = vis_layer.get_value(x, y);
                    let encoded = match v {
                        0 => {
                            n_unexplored += 1;
                            0u8
                        }
                        1 => {
                            n_explored += 1;
                            128u8
                        }
                        2 => {
                            n_visible += 1;
                            255u8
                        }
                        _ => {
                            // Treat unknown values as visible but track them.
                            n_other += 1;
                            255u8
                        }
                    };
                    data.push(encoded);
                }
            }

            // 1bpp fallback: values may arrive as 0/255 only (no explored state).
            if n_visible == 0 && n_explored == 0 && n_unexplored > 0 {
                for px in &mut data {
                    if *px > 0 {
                        *px = 255;
                    }
                }
            }

            // One-time summary so we can verify data has variation.
            static LOGGED_FOG_SUMMARY: AtomicBool = AtomicBool::new(false);
            if !LOGGED_FOG_SUMMARY.swap(true, Ordering::Relaxed) {
                eprintln!(
                    "[fog] first fog write: {}x{}, unexplored={n_unexplored}, explored={n_explored}, visible={n_visible}, other={n_other}, total={}",
                    w,
                    h,
                    data.len()
                );
            }
            fog.data = data;
            fog.dirty = true;
        }
    }

    Some(())
}

/// Repaint the map immediately when map layer toggles change.
pub fn refresh_map_colors_on_layer_change(
    layer_registry: Res<LayerRegistry>,
    map_res: Option<Res<MapResource>>,
    entity_system: Res<EntitySystem>,
    mut tile_color_query: Query<&mut TileColor>,
    mut terrain_mesh_dirty: ResMut<Terrain3dMeshDirty>,
) {
    if !layer_registry.is_changed() {
        return;
    }

    let Some(map_res) = map_res else {
        return;
    };

    update_tilemap_colors(
        &map_res.tile_storage,
        &map_res.static_layers,
        map_res.latest_creep_layer.as_ref(),
        map_res.latest_energy_layer.as_ref(),
        map_res.latest_visibility_layer.as_ref(),
        &mut tile_color_query,
        &entity_system,
        &layer_registry,
    );
    terrain_mesh_dirty.0 = true;
}

pub fn refresh_3d_terrain_mesh(
    map_res: Option<Res<MapResource>>,
    layer_registry: Res<LayerRegistry>,
    settings: Res<Terrain3dSettings>,
    entity_system: Res<EntitySystem>,
    mut dirty: ResMut<Terrain3dMeshDirty>,
    mut meshes: ResMut<Assets<Mesh>>,
) {
    if !dirty.0 && !layer_registry.is_changed() && !settings.is_changed() {
        return;
    }
    let Some(map_res) = map_res else {
        return;
    };
    let Some(mut mesh) = meshes.get_mut(&map_res.terrain_mesh_handle) else {
        return;
    };
    *mesh = build_terrain_mesh(
        &map_res.static_layers,
        map_res.latest_creep_layer.as_ref(),
        map_res.latest_energy_layer.as_ref(),
        &entity_system.map_config,
        &layer_registry,
        settings.height_scale,
    );
    dirty.0 = false;
}

fn handle_units_for_frame(
    commands: &mut Commands,
    asset_server: &Res<AssetServer>,
    registry: &mut ResMut<UnitRegistry>,
    entity_system: &Res<EntitySystem>,
    units: &[crate::frame::UnitSnapshot],
    unit_query: Query<&UnitBuildProgress>,
    seen_tags: &mut ResMut<ObservationUnitTags>,
    map_res: &Option<ResMut<MapResource>>,
    visual_assets: &mut Unit3dRenderAssets,
) {
    let map_size = map_res.as_ref().map(|m| {
        let (width, height) = m.static_layers.get_dimensions();
        (width as f32, height as f32)
    });

    apply_unit_snapshots(
        commands,
        asset_server,
        registry,
        entity_system,
        units,
        unit_query,
        seen_tags,
        map_size,
        visual_assets,
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
    layer_registry: Res<LayerRegistry>,
    mut images: ResMut<Assets<Image>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    terrain_settings: Res<Terrain3dSettings>,
) {
    // Drain all events in this batch; only process the first valid one.
    let events: Vec<_> = gi_events.read().collect();
    let Some(event) = events
        .iter()
        .find(|event| !event.static_map.pathing.data.is_empty())
    else {
        return;
    };

    let path_layer = TerrainLayer::from_frame_layer(&event.static_map.pathing);
    let placement_layer = TerrainLayer::from_frame_layer(&event.static_map.placement);
    let height_layer = TerrainLayer::from_frame_layer(&event.static_map.height);

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
        &layer_registry,
    );
    let terrain_mesh_handle = meshes.add(build_terrain_mesh(
        &static_layers,
        None,
        None,
        &entity_system.map_config,
        &layer_registry,
        terrain_settings.height_scale,
    ));
    let terrain_material = materials.add(StandardMaterial {
        perceptual_roughness: 1.0,
        cull_mode: None,
        ..default()
    });
    let terrain_mesh_entity = commands
        .spawn((
            Mesh3d(terrain_mesh_handle.clone()),
            MeshMaterial3d(terrain_material),
            Transform::IDENTITY,
            Visibility::Hidden,
            RenderLayerMarker(RenderLayerKind::Terrain),
            ViewModeVisibility(RenderViewMode::ThreeD),
        ))
        .id();
    commands.insert_resource(MapResource {
        static_layers: static_layers.clone(),
        tile_storage,
        terrain_mesh_handle,
        terrain_mesh_entity,
        last_creep_hash: 0,
        last_energy_hash: 0,
        last_visibility_hash: 0,
        latest_creep_layer: None,
        latest_energy_layer: None,
        latest_visibility_layer: None,
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
        Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        &data,
        TextureFormat::R8Unorm,
        RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
    );
    image.sampler = ImageSampler::nearest();
    image
}

/// System to handle observation events from the pipeline.
pub fn render_current_frame(
    mut last_vision_mode: ResMut<LastVisionMode>,
    mut map_res: Option<ResMut<MapResource>>,
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mut registry: ResMut<UnitRegistry>,
    entity_system: Res<EntitySystem>,
    mut tile_color_query: Query<&mut TileColor>,
    unit_query: Query<&UnitBuildProgress>,
    mut seen_tags: ResMut<ObservationUnitTags>,
    layer_registry: Res<LayerRegistry>,
    mut fog_data: Option<ResMut<FogOfWarData>>,
    mut terrain_mesh_dirty: ResMut<Terrain3dMeshDirty>,
    mut logged_first_obs: Local<bool>,
    current_frame: Res<CurrentFrame>,
    vision_mode: Res<VisionModeChannel>,
    mut visual_assets: Unit3dRenderAssets,
) {
    seen_tags.seen_tags.clear();

    let Some(frame) = current_frame.frame.as_ref() else {
        return;
    };
    handle_vision_mode_change(
        vision_mode.current,
        &mut last_vision_mode,
        &mut commands,
        &mut registry,
        &mut seen_tags,
    );

    if let Some(map_snapshot) = frame.map.as_ref() {
        if !*logged_first_obs {
            eprintln!(
                "[fog] first frame received. visibility layer present: {}.",
                map_snapshot.visibility.is_some()
            );
            *logged_first_obs = true;
        }
        let _ = update_map_from_frame(
            map_snapshot,
            &mut map_res,
            &mut tile_color_query,
            &entity_system,
            &layer_registry,
            &mut fog_data,
            &mut terrain_mesh_dirty,
        );
    }

    handle_units_for_frame(
        &mut commands,
        &asset_server,
        &mut registry,
        &entity_system,
        &frame.units,
        unit_query,
        &mut seen_tags,
        &map_res,
        &mut visual_assets,
    );
}
