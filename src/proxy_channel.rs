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

mod bridge;
mod coordination;
mod replay;

use bridge::{accept_proxy_client, connect_upstream, create_game, BridgeOptions};
pub use coordination::{CreateGameSignal, JoinResponseBarrier, MultiplayerPorts, ProxyReadySignal};
use replay::ProxyPublisher;
pub use replay::{CurrentFrame, ReplayBuffer, ReplayFrame};

use bevy::prelude::Resource;
use futures_util::StreamExt;
use sc2_proto::sc2api::{Request, Response};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::{broadcast, mpsc};
use tokio_stream::wrappers::BroadcastStream;
use tokio_tungstenite::WebSocketStream;

type WsStream = WebSocketStream<tokio::net::TcpStream>;
type UpstreamWs = WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// Size of the broadcast channel buffer.
const CHANNEL_BUFFER_SIZE: usize = 64;

// ─── Types ──────────────────────────────────────────────────────────────────

/// Identifier for the player/bot this channel belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PlayerId {
    Player1,
    Player2,
}

impl std::fmt::Display for PlayerId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PlayerId::Player1 => write!(f, "Player1"),
            PlayerId::Player2 => write!(f, "Player2"),
        }
    }
}

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
    /// Returns `(channel, broadcast_receiver)`.
    pub fn new(
        player_id: PlayerId,
        listen_addr: impl Into<String>,
        upstream_url: impl Into<String>,
    ) -> (Self, broadcast::Receiver<TaggedResponse>) {
        let (response_sender, receiver) = broadcast::channel(CHANNEL_BUFFER_SIZE);
        let (frame_sender, _) = broadcast::channel(CHANNEL_BUFFER_SIZE);
        (
            Self {
                player_id,
                listen_addr: listen_addr.into(),
                upstream_url: upstream_url.into(),
                publisher: ProxyPublisher {
                    response_sender,
                    frame_sender,
                    replay_buffer: Arc::new(Mutex::new(ReplayBuffer::new(512))),
                    current_frame: Arc::new(Mutex::new(CurrentFrame::default())),
                },
            },
            receiver,
        )
    }

    /// Subscribe to this channel's response stream.
    pub fn subscribe(&self) -> broadcast::Receiver<TaggedResponse> {
        self.publisher.response_sender.subscribe()
    }

    pub fn completed_frame_stream(
        &self,
    ) -> impl tokio_stream::Stream<Item = ReplayFrame> + Send + Unpin {
        tokio_stream::StreamExt::filter_map(
            BroadcastStream::new(self.publisher.frame_sender.subscribe()),
            |frame| frame.ok(),
        )
    }

    /// Access the replay history captured for this proxy.
    pub fn replay_buffer(&self) -> Arc<Mutex<ReplayBuffer>> {
        Arc::clone(&self.publisher.replay_buffer)
    }

    pub fn current_frame(&self) -> Arc<Mutex<CurrentFrame>> {
        Arc::clone(&self.publisher.current_frame)
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
    use sc2_proto::sc2api::ResponseObservation;
    use std::sync::{Arc, Mutex};

    #[test]
    fn test_player_id_display() {
        assert_eq!(format!("{}", PlayerId::Player1), "Player1");
        assert_eq!(format!("{}", PlayerId::Player2), "Player2");
    }

    #[test]
    fn replay_buffer_keeps_latest_frames() {
        let mut buffer = ReplayBuffer::new(3);

        buffer.push_frame(ReplayFrame::new(PlayerId::Player1, 10, None, None, None));
        buffer.push_frame(ReplayFrame::new(PlayerId::Player1, 11, None, None, None));
        buffer.push_frame(ReplayFrame::new(PlayerId::Player1, 12, None, None, None));
        buffer.push_frame(ReplayFrame::new(PlayerId::Player1, 13, None, None, None));

        assert_eq!(buffer.len(), 3);
        assert_eq!(buffer.latest_loop(), Some(13));
        assert_eq!(buffer.oldest_loop(), Some(11));
    }

    #[test]
    fn current_frame_finishes_and_resets_once() {
        let mut current = CurrentFrame::default();
        let mut observation_response = Response::new();
        let observation = ResponseObservation::new();
        observation_response.set_observation(observation.clone());

        current.update(PlayerId::Player1, &observation_response);
        let frame = current.finish().unwrap();

        assert_eq!(frame.player_id, PlayerId::Player1);
        assert!(frame.observation.is_some());
        assert!(current.player_id.is_none());
        assert!(current.observation.is_none());
        assert!(current.finish().is_none());
    }

    #[test]
    fn replay_frame_tracks_optional_debug_data() {
        let frame = ReplayFrame {
            player_id: PlayerId::Player2,
            game_loop: 42,
            observation: None,
            game_info: None,
            debug: Some(vec![1, 2, 3]),
        };

        assert_eq!(frame.debug.as_deref(), Some(&[1, 2, 3][..]));
    }

    #[test]
    fn publisher_routes_observer_responses_and_captures_replay() {
        let (response_sender, mut response_receiver) = broadcast::channel(4);
        let (frame_sender, mut frame_receiver) = broadcast::channel(4);
        let (observer_sender, mut observer_receiver) = broadcast::channel(4);
        let replay_buffer = Arc::new(Mutex::new(ReplayBuffer::new(4)));
        let publisher = ProxyPublisher {
            response_sender,
            frame_sender,
            replay_buffer: Arc::clone(&replay_buffer),
            current_frame: Arc::new(Mutex::new(CurrentFrame::default())),
        };

        let mut observation_response = Response::new();
        observation_response.set_observation(ResponseObservation::new());
        publisher.publish_to(&observer_sender, PlayerId::Player2, observation_response);

        let observer_message = observer_receiver.try_recv().unwrap();
        assert_eq!(observer_message.player_id, PlayerId::Player2);
        assert!(response_receiver.try_recv().is_err());

        let mut step_response = Response::new();
        step_response.mut_step();
        publisher.publish(PlayerId::Player1, step_response);

        assert!(response_receiver.try_recv().unwrap().response.has_step());
        assert!(frame_receiver.try_recv().unwrap().observation.is_some());
        assert_eq!(replay_buffer.lock().unwrap().len(), 1);
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
