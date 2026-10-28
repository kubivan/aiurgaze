//! Reactive proxy data channel for SC2 bot communication.
//!
//! Each bot gets its own `ProxyDataChannel` with an **independent** upstream
//! WS connection to SC2. SC2 headless accepts one WS per player — no sharing.
//!
//! Coordination between proxies is minimal:
//! - Host sends CreateGame, then signals via `CreateGameSignal`.
//! - Both proxies independently send JoinGame on their own WS.
//! - After JoinGame, each proxy fetches a silent GameInfo for map data.
//! - Then the proxy enters the bridge loop forwarding traffic in both directions.

pub(crate) mod bridge;
mod coordination;

use bridge::{accept_proxy_client, connect_upstream, create_game, BridgeOptions};
pub use coordination::{CreateGameSignal, JoinResponseBarrier, MultiplayerPorts, ProxyReadySignal};
pub use crate::frame::PlayerId;

use bevy::prelude::Resource;
use futures_util::StreamExt;
use sc2_proto::sc2api::{Request, Response};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc};
use tokio_stream::wrappers::BroadcastStream;
use tokio_tungstenite::WebSocketStream;

type WsStream = WebSocketStream<tokio::net::TcpStream>;
pub(crate) type UpstreamWs =
    WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// Size of the broadcast channel buffer.
const CHANNEL_BUFFER_SIZE: usize = 64;

#[derive(Clone)]
pub(crate) struct ProxyPublisher {
    response_sender: broadcast::Sender<TaggedResponse>,
    frame_response_sender: mpsc::Sender<TaggedResponse>,
}

impl ProxyPublisher {
    pub(crate) async fn publish(&self, player_id: PlayerId, response: Response) {
        self.publish_to(&self.response_sender, player_id, response)
            .await;
    }

    async fn publish_to(
        &self,
        sender: &broadcast::Sender<TaggedResponse>,
        player_id: PlayerId,
        response: Response,
    ) {
        let tagged = TaggedResponse {
            player_id,
            response,
        };
        let _ = sender.send(tagged.clone());
        if self.frame_response_sender.send(tagged).await.is_err() {
            eprintln!("[ProxyPublisher] Frame consumer has stopped");
        }
    }
}

// ─── Types ──────────────────────────────────────────────────────────────────

/// Shared pause state for live bot/server communication.
/// When paused, the proxy stops reading client requests until resumed.
#[derive(Resource, Clone, Default, Debug)]
pub struct ProxyStreamPause {
    pub paused: Arc<AtomicBool>,
}

impl ProxyStreamPause {
    pub fn new() -> Self {
        Self {
            paused: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn set_paused(&self, paused: bool) {
        self.paused.store(paused, Ordering::SeqCst);
    }

    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::SeqCst)
    }
}

/// Response tagged with its source player.
#[derive(Debug, Clone)]
pub struct TaggedResponse {
    pub player_id: PlayerId,
    pub response: Response,
}

// ─── ProxyDataChannel ───────────────────────────────────────────────────────

/// A reactive proxy channel for a single bot.
///
/// Each channel owns its own upstream WS to SC2.
/// Three run modes: `run_host`, `run_guest`, `run_solo`.
pub struct ProxyDataChannel {
    pub player_id: PlayerId,
    pub listen_addr: String,
    pub upstream_url: String,
    publisher: ProxyPublisher,
}

impl ProxyDataChannel {
    /// Create a new proxy data channel.
    /// Returns the channel, its response receiver, and an ordered replay input.
    pub fn new(
        player_id: PlayerId,
        listen_addr: impl Into<String>,
        upstream_url: impl Into<String>,
    ) -> (
        Self,
        broadcast::Receiver<TaggedResponse>,
        mpsc::Receiver<TaggedResponse>,
    ) {
        let (response_sender, response_receiver) = broadcast::channel(CHANNEL_BUFFER_SIZE);
        let (replay_response_sender, replay_response_receiver) = mpsc::channel(CHANNEL_BUFFER_SIZE);
        (
            Self {
                player_id,
                listen_addr: listen_addr.into(),
                upstream_url: upstream_url.into(),
                publisher: ProxyPublisher {
                    response_sender,
                    frame_response_sender: replay_response_sender,
                },
            },
            response_receiver,
            replay_response_receiver,
        )
    }

    /// Subscribe to this channel's response stream.
    pub fn subscribe(&self) -> broadcast::Receiver<TaggedResponse> {
        self.publisher.response_sender.subscribe()
    }

    /// Get a typed response stream (BroadcastStream) from this channel.
    ///
    /// Converts the raw broadcast receiver into a filtered stream of
    /// `TaggedResponse`, ready for reactive composition in the pipeline.
    pub fn response_stream(
        &self,
    ) -> impl tokio_stream::Stream<Item = TaggedResponse> + Send + Unpin {
        tokio_stream::StreamExt::filter_map(
            BroadcastStream::new(self.publisher.response_sender.subscribe()),
            |r| r.ok(),
        )
    }

    pub(crate) fn into_replay_parts(self) -> (PlayerId, String, ProxyPublisher) {
        (self.player_id, self.upstream_url, self.publisher)
    }

    // ── Run modes ───────────────────────────────────────────────────────

    /// Host mode (Player1 in VsBot):
    /// 1. Connect upstream WS
    /// 2. Send CreateGame, publish response
    /// 3. Signal CreateGameSignal
    /// 4. Accept bot client, send JoinGame + silent GameInfo
    /// 5. Bridge loop
    pub async fn run_host(
        self,
        ready_signal: ProxyReadySignal,
        create_game_signal: CreateGameSignal,
        join_barrier: Option<JoinResponseBarrier>,
        create_game_request: Request,
        multiplayer_ports: Option<MultiplayerPorts>,
        debug_tx: Option<mpsc::Sender<crate::debug_draw::DebugDrawEvent>>,
        chat_tx: Option<mpsc::Sender<crate::chat_overlay::ChatMessageEvent>>,
        pause_state: Option<Arc<AtomicBool>>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let pid = self.player_id;
        let publisher = self.publisher.clone();

        // 1. Open own upstream WS
        let upstream = connect_upstream(&self.upstream_url).await?;
        let (mut up_w, mut up_r) = upstream.split();

        // 2. CreateGame
        create_game(pid, create_game_request, &mut up_w, &mut up_r, &publisher).await?;

        // 3. Signal guest
        create_game_signal.signal();

        // 4. Accept client + join
        let client_ws = accept_proxy_client(&self.listen_addr, pid, &ready_signal).await?;

        bridge::accept_and_bridge(
            pid,
            client_ws,
            &mut up_w,
            &mut up_r,
            &publisher,
            BridgeOptions {
                join_barrier,
                multiplayer_ports,
                observer_sender: None,
                debug_tx,
                chat_tx,
                pause_state,
            },
        )
        .await
    }

    /// Guest mode (Player2 in VsBot):
    /// 1. Wait for CreateGameSignal
    /// 2. Connect own upstream WS
    /// 3. Accept bot client, send JoinGame + silent GameInfo
    /// 4. Bridge loop
    pub async fn run_guest(
        self,
        ready_signal: ProxyReadySignal,
        create_game_signal: CreateGameSignal,
        join_barrier: Option<JoinResponseBarrier>,
        multiplayer_ports: Option<MultiplayerPorts>,
        debug_tx: Option<mpsc::Sender<crate::debug_draw::DebugDrawEvent>>,
        chat_tx: Option<mpsc::Sender<crate::chat_overlay::ChatMessageEvent>>,
        pause_state: Option<Arc<AtomicBool>>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let pid = self.player_id;
        let publisher = self.publisher.clone();

        // 1. Wait for game creation
        println!("[{pid}] Waiting for CreateGame signal...");
        create_game_signal.wait().await;
        println!("[{pid}] CreateGame signal received");

        // 2. Own upstream WS
        let upstream = connect_upstream(&self.upstream_url).await?;
        let (mut up_w, mut up_r) = upstream.split();

        // 3. Accept client + join
        let client_ws = accept_proxy_client(&self.listen_addr, pid, &ready_signal).await?;

        bridge::accept_and_bridge(
            pid,
            client_ws,
            &mut up_w,
            &mut up_r,
            &publisher,
            BridgeOptions {
                join_barrier,
                multiplayer_ports,
                observer_sender: None,
                debug_tx,
                chat_tx,
                pause_state,
            },
        )
        .await
    }

    /// Solo mode (VsAI — single bot):
    /// 1. Connect upstream WS
    /// 2. Send CreateGame, publish response
    /// 3. Optionally signal CreateGameSignal (for observer coordination)
    /// 4. Accept bot client, send JoinGame + silent GameInfo
    /// 5. Bridge loop (interleaving observer observations if sender provided)
    pub async fn run_solo(
        self,
        ready_signal: ProxyReadySignal,
        create_game_request: Request,
        observer_sender: Option<broadcast::Sender<TaggedResponse>>,
        debug_tx: Option<mpsc::Sender<crate::debug_draw::DebugDrawEvent>>,
        chat_tx: Option<mpsc::Sender<crate::chat_overlay::ChatMessageEvent>>,
        pause_state: Option<Arc<AtomicBool>>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let pid = self.player_id;
        let publisher = self.publisher.clone();

        let upstream = connect_upstream(&self.upstream_url).await?;
        let (mut up_w, mut up_r) = upstream.split();

        create_game(pid, create_game_request, &mut up_w, &mut up_r, &publisher).await?;

        // Signal observer (if present) that CreateGame is done
        // (Not needed anymore — observer is interleaved in bridge loop)

        // Accept client
        let client_ws = accept_proxy_client(&self.listen_addr, pid, &ready_signal).await?;

        bridge::accept_and_bridge(
            pid,
            client_ws,
            &mut up_w,
            &mut up_r,
            &publisher,
            BridgeOptions {
                join_barrier: None,
                multiplayer_ports: None,
                observer_sender,
                debug_tx,
                chat_tx,
                pause_state,
            },
        )
        .await
    }

}

// ─── Observer broadcast channel helper ──────────────────────────────────────

/// Create a broadcast channel for observer data.
///
/// Used in VsAI mode: the proxy interleaves `disable_fog` observation requests
/// in its bridge loop and publishes responses on the observer sender.
/// Returns (sender for proxy, receiver for subscription).
pub fn create_observer_channel() -> (
    broadcast::Sender<TaggedResponse>,
    broadcast::Receiver<TaggedResponse>,
) {
    broadcast::channel(CHANNEL_BUFFER_SIZE)
}

/// Get a typed response stream from an observer broadcast sender.
pub fn observer_response_stream(
    sender: &broadcast::Sender<TaggedResponse>,
) -> impl tokio_stream::Stream<Item = TaggedResponse> + Send + Unpin {
    tokio_stream::StreamExt::filter_map(BroadcastStream::new(sender.subscribe()), |r| r.ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::{Frame, FrameSource};
    use sc2_proto::sc2api::ResponseObservation;

    #[test]
    fn test_player_id_display() {
        assert_eq!(format!("{}", PlayerId::Player1), "Player1");
        assert_eq!(format!("{}", PlayerId::Player2), "Player2");
    }

    #[test]
    fn frame_history_keeps_latest_frames() {
        let mut buffer = crate::frame::FrameHistory::new(3);

        buffer.push_frame(Frame::new(FrameSource::Game, PlayerId::Player1, 10));
        buffer.push_frame(Frame::new(FrameSource::Game, PlayerId::Player1, 11));
        buffer.push_frame(Frame::new(FrameSource::Game, PlayerId::Player1, 12));
        buffer.push_frame(Frame::new(FrameSource::Game, PlayerId::Player1, 13));

        assert_eq!(buffer.len(), 3);
        assert_eq!(buffer.latest_loop(), Some(13));
        assert_eq!(buffer.oldest_loop(), Some(11));
    }

    #[test]
    fn game_assembler_closes_frames_on_step_responses() {
        let mut assembler = crate::sources::game::FrameAssembler::default();
        let mut observation_response = Response::new();
        let mut observation = ResponseObservation::new();
        observation.mut_observation().set_game_loop(1);
        observation_response.set_observation(observation);

        assert!(assembler
            .push(TaggedResponse {
                player_id: PlayerId::Player1,
                response: observation_response,
            })
            .is_none());

        let mut step_response = Response::new();
        step_response.mut_step();
        let frame = assembler
            .push(TaggedResponse {
                player_id: PlayerId::Player1,
                response: step_response,
            })
            .unwrap();

        assert_eq!(frame.player_id, PlayerId::Player1);
        assert_eq!(frame.game_loop, 1);
        assert_eq!(frame.source, FrameSource::Game);
        assert!(assembler
            .push(TaggedResponse {
                player_id: PlayerId::Player1,
                response: Response::new(),
            })
            .is_none());
    }

    #[test]
    fn replay_assembler_tags_frames_as_replay() {
        let mut assembler = crate::sources::replay::FrameAssembler::default();
        let mut observation_response = Response::new();
        let mut observation = ResponseObservation::new();
        observation.mut_observation().set_game_loop(1);
        observation_response.set_observation(observation);

        assert!(assembler
            .push(TaggedResponse {
                player_id: PlayerId::Player1,
                response: observation_response,
            })
            .is_none());

        let mut step_response = Response::new();
        step_response.mut_step();
        let frame = assembler
            .push(TaggedResponse {
                player_id: PlayerId::Player1,
                response: step_response,
            })
            .unwrap();

        assert_eq!(frame.source, FrameSource::Replay);
    }

    #[tokio::test]
    async fn publisher_routes_observer_responses_and_captures_replay() {
        let (response_sender, mut response_receiver) = broadcast::channel(4);
        let (frame_response_sender, mut frame_response_receiver) = mpsc::channel(4);
        let (observer_sender, mut observer_receiver) = broadcast::channel(4);
        let publisher = ProxyPublisher {
            response_sender,
            frame_response_sender,
        };
        let mut assembler = crate::sources::game::FrameAssembler::default();

        let mut observation_response = Response::new();
        let mut observation = ResponseObservation::new();
        observation.mut_observation().set_game_loop(1);
        observation_response.set_observation(observation);
        publisher
            .publish_to(&observer_sender, PlayerId::Player2, observation_response)
            .await;

        let observer_message = observer_receiver.try_recv().unwrap();
        assert_eq!(observer_message.player_id, PlayerId::Player2);
        assert!(response_receiver.try_recv().is_err());
        assert!(assembler
            .push(frame_response_receiver.try_recv().unwrap())
            .is_none());

        let mut step_response = Response::new();
        step_response.mut_step();
        publisher.publish(PlayerId::Player1, step_response).await;

        assert!(response_receiver.try_recv().unwrap().response.has_step());
        let frame = assembler
            .push(frame_response_receiver.try_recv().unwrap())
            .unwrap();
        assert_eq!(frame.game_loop, 1);
    }

    #[tokio::test]
    async fn publisher_waits_when_replay_queue_is_full() {
        let (response_sender, _response_receiver) = broadcast::channel(1);
        let (frame_response_sender, mut frame_response_receiver) = mpsc::channel(1);
        let publisher = ProxyPublisher {
            response_sender,
            frame_response_sender,
        };
        publisher.publish(PlayerId::Player1, Response::new()).await;

        let mut step_response = Response::new();
        step_response.mut_step();
        let publish_step = publisher.publish(PlayerId::Player1, step_response);
        tokio::pin!(publish_step);

        let first_response = tokio::select! {
            _ = &mut publish_step => panic!("publisher should wait for queue capacity"),
            response = frame_response_receiver.recv() => response.unwrap(),
        };
        assert!(!first_response.response.has_step());

        publish_step.await;
        assert!(frame_response_receiver
            .recv()
            .await
            .unwrap()
            .response
            .has_step());
    }

    #[tokio::test]
    async fn coordination_waits_observe_signals_sent_before_waiting() {
        let ready = ProxyReadySignal::new(2);
        ready.signal_ready();
        ready.signal_ready();
        tokio::time::timeout(std::time::Duration::from_secs(1), ready.wait_ready())
            .await
            .unwrap();

        let create_game = CreateGameSignal::new();
        create_game.signal();
        tokio::time::timeout(std::time::Duration::from_secs(1), create_game.wait())
            .await
            .unwrap();

        let join_barrier = JoinResponseBarrier::new(2);
        join_barrier.mark_joined();
        join_barrier.mark_joined();
        tokio::time::timeout(std::time::Duration::from_secs(1), join_barrier.wait_ready())
            .await
            .unwrap();
    }

    #[test]
    fn observer_game_loop_advances_only_for_newer_observations() {
        let mut last_game_loop = 10;

        assert!(!bridge::advance_observer_game_loop(&mut last_game_loop, 10));
        assert!(!bridge::advance_observer_game_loop(&mut last_game_loop, 9));
        assert!(bridge::advance_observer_game_loop(&mut last_game_loop, 11));
        assert_eq!(last_game_loop, 11);
    }
}
