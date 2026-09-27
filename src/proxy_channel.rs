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

use bevy::prelude::Resource;
use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use protobuf::{Message, RepeatedField};
use sc2_proto::debug::{DebugCommand, DebugGameState};
use sc2_proto::sc2api::{
    PortSet, Request, Request_oneof_request, Response, ResponseGameInfo, ResponseObservation,
};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;
use tokio::sync::{broadcast, mpsc, Notify};
use tokio_stream::wrappers::BroadcastStream;
use tokio_tungstenite::{accept_async, connect_async, WebSocketStream};

type WsStream = WebSocketStream<tokio::net::TcpStream>;
type UpstreamWs = WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// Size of the broadcast channel buffer.
const CHANNEL_BUFFER_SIZE: usize = 64;

// ─── Signals ────────────────────────────────────────────────────────────────

/// Shared signal indicating how many proxy listeners are ready.
/// Bots should wait for this before connecting.
#[derive(Debug, Clone, Default)]
pub struct ProxyReadySignal {
    ready_count: Arc<AtomicU8>,
    expected_count: Arc<AtomicU8>,
}

impl ProxyReadySignal {
    pub fn new(expected_count: u8) -> Self {
        Self {
            ready_count: Arc::new(AtomicU8::new(0)),
            expected_count: Arc::new(AtomicU8::new(expected_count)),
        }
    }

    pub fn signal_ready(&self) {
        let prev = self.ready_count.fetch_add(1, Ordering::SeqCst);
        println!(
            "[ProxyReadySignal] Proxy ready ({}/{})",
            prev + 1,
            self.expected_count.load(Ordering::SeqCst)
        );
    }

    pub fn is_ready(&self) -> bool {
        self.ready_count.load(Ordering::SeqCst) >= self.expected_count.load(Ordering::SeqCst)
    }

    pub fn has_count(&self, count: u8) -> bool {
        self.ready_count.load(Ordering::SeqCst) >= count
    }

    pub async fn wait_ready(&self) {
        while !self.is_ready() {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }

    pub async fn wait_for_count(&self, count: u8) {
        while !self.has_count(count) {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }
}

/// One-shot signal: host fires after CreateGame succeeds.
/// Guest awaits before sending JoinGame.
#[derive(Debug, Clone)]
pub struct CreateGameSignal {
    done: Arc<AtomicBool>,
    notify: Arc<Notify>,
}

impl CreateGameSignal {
    pub fn new() -> Self {
        Self {
            done: Arc::new(AtomicBool::new(false)),
            notify: Arc::new(Notify::new()),
        }
    }

    pub fn signal(&self) {
        self.done.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
    }

    pub async fn wait(&self) {
        while !self.done.load(Ordering::SeqCst) {
            self.notify.notified().await;
        }
    }
}

/// Barrier to synchronize JoinGame responses across proxies.
#[derive(Debug, Clone)]
pub struct JoinResponseBarrier {
    expected: u8,
    count: Arc<AtomicU8>,
    notify: Arc<Notify>,
}

impl JoinResponseBarrier {
    pub fn new(expected: u8) -> Self {
        Self {
            expected,
            count: Arc::new(AtomicU8::new(0)),
            notify: Arc::new(Notify::new()),
        }
    }

    pub fn mark_joined(&self) {
        let current = self.count.fetch_add(1, Ordering::SeqCst) + 1;
        if current >= self.expected {
            self.notify.notify_waiters();
        }
    }

    pub async fn wait_ready(&self) {
        while self.count.load(Ordering::SeqCst) < self.expected {
            self.notify.notified().await;
        }
    }
}

// ─── Multiplayer ports ──────────────────────────────────────────────────────

/// Port configuration for SC2 multiplayer internal communication.
///
/// These are NOT the WebSocket proxy ports — they are TCP ports that SC2
/// opens internally for game synchronisation between participants.
/// Both players must send the **same** server_ports and client_ports in
/// their JoinGame requests.
#[derive(Debug, Clone)]
pub struct MultiplayerPorts {
    pub server_game_port: i32,
    pub server_base_port: i32,
    /// One (game_port, base_port) pair per participant.
    pub client_ports: Vec<(i32, i32)>,
}

impl MultiplayerPorts {
    /// Derive ports automatically from a base port.
    ///
    /// Layout (for `base`=5002, 2 players):
    ///   server  : (5002, 5003)
    ///   client 1: (5004, 5005)
    ///   client 2: (5006, 5007)
    pub fn from_base(base: u16, num_players: u8) -> Self {
        let b = base as i32;
        let mut clients = Vec::new();
        for i in 0..num_players {
            let offset = 2 + (i as i32) * 2; // +2, +4, +6, …
            clients.push((b + offset, b + offset + 1));
        }
        Self {
            server_game_port: b,
            server_base_port: b + 1,
            client_ports: clients,
        }
    }

    /// Build a `PortSet` for the server ports.
    fn server_port_set(&self) -> PortSet {
        let mut ps = PortSet::new();
        ps.set_game_port(self.server_game_port);
        ps.set_base_port(self.server_base_port);
        ps
    }

    /// Build the repeated `PortSet` list for client ports.
    fn client_port_sets(&self) -> RepeatedField<PortSet> {
        let sets: Vec<PortSet> = self
            .client_ports
            .iter()
            .map(|&(gp, bp)| {
                let mut ps = PortSet::new();
                ps.set_game_port(gp);
                ps.set_base_port(bp);
                ps
            })
            .collect();
        RepeatedField::from_vec(sets)
    }

    /// Inject server_ports and client_ports into a parsed JoinGame request.
    fn inject_into(&self, req: &mut Request) {
        if let Some(Request_oneof_request::join_game(ref mut jg)) = req.request {
            jg.set_server_ports(self.server_port_set());
            jg.set_client_ports(self.client_port_sets());
            println!(
                "[MultiplayerPorts] Injected server=({},{}) clients={:?}",
                self.server_game_port, self.server_base_port, self.client_ports
            );
        }
    }
}

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
/// When paused, the proxy stops forwarding requests/responses and keeps
/// client-side pending traffic queued until resumed.
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

/// Single snapshot for a given SC2 loop step.
///
/// The replay buffer stores one frame per loop and keeps a full set of the
/// important data needed to scrub and re-render past game state.
#[derive(Debug, Clone)]
pub struct ReplayFrame {
    pub player_id: PlayerId,
    pub game_loop: u32,
    pub observation: Option<ResponseObservation>,
    pub game_info: Option<ResponseGameInfo>,
    pub debug: Option<Vec<u8>>,
}

/// Proxy-owned frame accumulator for one simulation step.
///
/// Responses update this frame until the corresponding step response arrives.
/// At that boundary the frame is committed to history and the accumulator is
/// reset for the next step.
#[derive(Debug, Clone, Default)]
pub struct CurrentFrame {
    pub player_id: Option<PlayerId>,
    pub game_loop: u32,
    pub observation: Option<ResponseObservation>,
    pub game_info: Option<ResponseGameInfo>,
    pub debug: Option<Vec<u8>>,
}

impl CurrentFrame {
    pub fn update(&mut self, player_id: PlayerId, response: &Response) {
        self.player_id = Some(player_id);
        match response.response.as_ref() {
            Some(sc2_proto::sc2api::Response_oneof_response::observation(observation)) => {
                self.game_loop = observation
                    .observation
                    .as_ref()
                    .map(|inner| inner.get_game_loop())
                    .unwrap_or(self.game_loop);
                self.observation = Some(observation.clone());
            }
            Some(sc2_proto::sc2api::Response_oneof_response::game_info(game_info)) => {
                self.game_info = Some(game_info.clone());
            }
            Some(sc2_proto::sc2api::Response_oneof_response::debug(_)) => {
                self.debug = response.write_to_bytes().ok();
            }
            _ => {}
        }
    }

    pub fn finish(&mut self) -> Option<ReplayFrame> {
        let player_id = self.player_id?;
        if self.observation.is_none() && self.game_info.is_none() && self.debug.is_none() {
            self.reset();
            return None;
        }

        let frame = ReplayFrame::new(
            player_id,
            self.game_loop,
            self.observation.take(),
            self.game_info.take(),
            self.debug.take(),
        );
        self.reset();
        Some(frame)
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

impl ReplayFrame {
    pub fn new(
        player_id: PlayerId,
        game_loop: u32,
        observation: Option<ResponseObservation>,
        game_info: Option<ResponseGameInfo>,
        debug: Option<Vec<u8>>,
    ) -> Self {
        Self {
            player_id,
            game_loop,
            observation,
            game_info,
            debug,
        }
    }

    pub fn from_response(player_id: PlayerId, response: &Response) -> Self {
        let game_loop = match response.response.as_ref() {
            Some(sc2_proto::sc2api::Response_oneof_response::observation(obs)) => obs
                .observation
                .as_ref()
                .map(|inner| inner.get_game_loop())
                .unwrap_or(0),
            Some(sc2_proto::sc2api::Response_oneof_response::game_info(_)) => 0,
            Some(sc2_proto::sc2api::Response_oneof_response::debug(_)) => 0,
            _ => 0,
        };

        let observation = match response.response.as_ref() {
            Some(sc2_proto::sc2api::Response_oneof_response::observation(obs)) => Some(obs.clone()),
            _ => None,
        };
        let game_info = match response.response.as_ref() {
            Some(sc2_proto::sc2api::Response_oneof_response::game_info(gi)) => Some(gi.clone()),
            _ => None,
        };
        let debug = match response.response.as_ref() {
            Some(sc2_proto::sc2api::Response_oneof_response::debug(_)) => response
                .write_to_bytes()
                .ok()
                .filter(|bytes| !bytes.is_empty()),
            _ => None,
        };

        Self {
            player_id,
            game_loop,
            observation,
            game_info,
            debug,
        }
    }
}

/// Bounded replay history for live loop scrubbing.
#[derive(Debug, Clone, Default)]
pub struct ReplayBuffer {
    capacity: usize,
    frames: VecDeque<ReplayFrame>,
}

impl ReplayBuffer {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            frames: VecDeque::new(),
        }
    }

    pub fn push_frame(&mut self, frame: ReplayFrame) {
        if self.frames.len() == self.capacity {
            self.frames.pop_front();
        }
        self.frames.push_back(frame);
    }

    pub fn len(&self) -> usize {
        self.frames.len()
    }

    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    pub fn latest_loop(&self) -> Option<u32> {
        self.frames.back().map(|frame| frame.game_loop)
    }

    pub fn oldest_loop(&self) -> Option<u32> {
        self.frames.front().map(|frame| frame.game_loop)
    }

    pub fn frames(&self) -> impl Iterator<Item = &ReplayFrame> {
        self.frames.iter()
    }

    pub fn latest_frame(&self) -> Option<&ReplayFrame> {
        self.frames.back()
    }

    pub fn clear(&mut self) {
        self.frames.clear();
    }
}

// ─── Small helpers ──────────────────────────────────────────────────────────

/// Parse raw bytes into a protobuf Request, or None.
fn try_parse_request(raw: &[u8]) -> Option<Request> {
    let mut req = Request::new();
    req.merge_from_bytes(raw).ok().map(|_| req)
}

/// Parse raw bytes into a protobuf Response, or None.
fn try_parse_response(raw: &[u8]) -> Option<Response> {
    let mut res = Response::new();
    res.merge_from_bytes(raw).ok().map(|_| res)
}

/// Check if a parsed Request is a JoinGame.
fn is_join_game(req: &Request) -> bool {
    matches!(req.request, Some(Request_oneof_request::join_game(_)))
}

/// Build a GameInfo request (for silent post-join fetch).
fn make_game_info_request() -> Result<Vec<u8>, String> {
    let mut req = Request::new();
    req.mut_game_info();
    req.write_to_bytes()
        .map_err(|e| format!("Protobuf encode: {e}"))
}

/// Build an Observation request with `disable_fog = true` for the observer.
fn make_disable_fog_observation_request() -> Result<Vec<u8>, String> {
    let mut req = Request::new();
    let obs = req.mut_observation();
    obs.set_disable_fog(true);
    req.write_to_bytes()
        .map_err(|e| format!("Protobuf encode: {e}"))
}

/// Build a Debug request that reveals the full map (`show_map`).
fn make_reveal_map_debug_request() -> Result<Vec<u8>, String> {
    let mut req = Request::new();
    let mut cmd = DebugCommand::new();
    cmd.set_game_state(DebugGameState::show_map);
    req.mut_debug()
        .set_debug(RepeatedField::from_vec(vec![cmd]));
    req.write_to_bytes()
        .map_err(|e| format!("Protobuf encode: {e}"))
}

/// Connect to SC2 upstream with retries.
async fn connect_upstream(url: &str) -> Result<UpstreamWs, tungstenite::Error> {
    let mut retries = 10u32;
    let delay = std::time::Duration::from_secs(2);
    println!("[connect_upstream] Connecting to {url}");
    loop {
        match connect_async(url).await {
            Ok((ws, _)) => {
                println!("[connect_upstream] Connected");
                return Ok(ws);
            }
            Err(e) => {
                if retries == 0 {
                    return Err(e);
                }
                println!("[connect_upstream] Retry ({retries} left): {e}");
                retries -= 1;
                tokio::time::sleep(delay).await;
            }
        }
    }
}

/// Accept exactly one WebSocket client on the listener.
async fn accept_one_client(
    listener: &TcpListener,
    player_id: PlayerId,
) -> Result<WsStream, Box<dyn std::error::Error + Send + Sync>> {
    println!("[{player_id}] Waiting for client...");
    loop {
        let (stream, addr) = listener.accept().await?;
        println!("[{player_id}] Client connected from {addr}");
        match accept_async(stream).await {
            Ok(ws) => return Ok(ws),
            Err(e) => eprintln!("[{player_id}] Handshake failed (retrying): {e}"),
        }
    }
}

/// Send raw bytes upstream and read one response. Returns raw response bytes.
async fn roundtrip(
    write: &mut futures_util::stream::SplitSink<UpstreamWs, tungstenite::Message>,
    read: &mut futures_util::stream::SplitStream<UpstreamWs>,
    data: Vec<u8>,
) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
    write
        .send(tungstenite::Message::Binary(Bytes::from(data)))
        .await?;
    let msg = read.next().await.ok_or("SC2 closed connection")??;
    Ok(msg.into_data().to_vec())
}

/// Publish a response to the broadcast channel (best-effort).
fn publish(
    sender: &broadcast::Sender<TaggedResponse>,
    frame_sender: &broadcast::Sender<ReplayFrame>,
    replay_buffer: Option<&Arc<Mutex<ReplayBuffer>>>,
    current_frame: Option<&Arc<Mutex<CurrentFrame>>>,
    player_id: PlayerId,
    res: Response,
) {
    let response_clone = res.clone();
    let _ = sender.send(TaggedResponse {
        player_id,
        response: res,
    });

    let Some(current_frame) = current_frame else {
        return;
    };

    let mut frame = current_frame.lock().expect("current frame lock poisoned");
    frame.update(player_id, &response_clone);
    let is_step = matches!(
        response_clone.response,
        Some(sc2_proto::sc2api::Response_oneof_response::step(_))
    );
    if is_step {
        if let Some(completed) = frame.finish() {
            let _ = frame_sender.send(completed.clone());
            if let Some(buffer) = replay_buffer {
                buffer
                    .lock()
                    .expect("replay buffer lock poisoned")
                    .push_frame(completed);
            }
        }
    }
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
    sender: broadcast::Sender<TaggedResponse>,
    frame_sender: broadcast::Sender<ReplayFrame>,
    replay_buffer: Arc<Mutex<ReplayBuffer>>,
    current_frame: Arc<Mutex<CurrentFrame>>,
}

impl ProxyDataChannel {
    /// Create a new proxy data channel.
    /// Returns `(channel, broadcast_receiver)`.
    pub fn new(
        player_id: PlayerId,
        listen_addr: impl Into<String>,
        upstream_url: impl Into<String>,
    ) -> (Self, broadcast::Receiver<TaggedResponse>) {
        let (sender, receiver) = broadcast::channel(CHANNEL_BUFFER_SIZE);
        let (frame_sender, _) = broadcast::channel(CHANNEL_BUFFER_SIZE);
        (
            Self {
                player_id,
                listen_addr: listen_addr.into(),
                upstream_url: upstream_url.into(),
                sender,
                frame_sender,
                replay_buffer: Arc::new(Mutex::new(ReplayBuffer::new(512))),
                current_frame: Arc::new(Mutex::new(CurrentFrame::default())),
            },
            receiver,
        )
    }

    /// Subscribe to this channel's response stream.
    pub fn subscribe(&self) -> broadcast::Receiver<TaggedResponse> {
        self.sender.subscribe()
    }

    pub fn completed_frame_stream(
        &self,
    ) -> impl tokio_stream::Stream<Item = ReplayFrame> + Send + Unpin {
        tokio_stream::StreamExt::filter_map(
            BroadcastStream::new(self.frame_sender.subscribe()),
            |frame| frame.ok(),
        )
    }

    /// Access the replay history captured for this proxy.
    pub fn replay_buffer(&self) -> Arc<Mutex<ReplayBuffer>> {
        Arc::clone(&self.replay_buffer)
    }

    pub fn current_frame(&self) -> Arc<Mutex<CurrentFrame>> {
        Arc::clone(&self.current_frame)
    }

    /// Get a typed response stream (BroadcastStream) from this channel.
    ///
    /// Converts the raw broadcast receiver into a filtered stream of
    /// `TaggedResponse`, ready for reactive composition in the pipeline.
    pub fn response_stream(
        &self,
    ) -> impl tokio_stream::Stream<Item = TaggedResponse> + Send + Unpin {
        tokio_stream::StreamExt::filter_map(BroadcastStream::new(self.sender.subscribe()), |r| {
            r.ok()
        })
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
        let sender = self.sender.clone();
        let frame_sender = self.frame_sender.clone();
        let replay_buffer = Arc::clone(&self.replay_buffer);
        let current_frame = Arc::clone(&self.current_frame);

        // 1. Open own upstream WS
        let upstream = connect_upstream(&self.upstream_url).await?;
        let (mut up_w, mut up_r) = upstream.split();

        // 2. CreateGame
        println!("[{pid}] Sending CreateGame");
        let cg_bytes = create_game_request
            .write_to_bytes()
            .map_err(|e| format!("Protobuf encode: {e}"))?;
        let cg_resp = roundtrip(&mut up_w, &mut up_r, cg_bytes).await?;

        if let Some(res) = try_parse_response(&cg_resp) {
            if res.has_create_game() {
                let cg = res.get_create_game();
                if cg.has_error() {
                    eprintln!(
                        "[{pid}] CreateGame error: {:?} - {}",
                        cg.get_error(),
                        cg.get_error_details()
                    );
                } else {
                    println!("[{pid}] CreateGame succeeded");
                }
            }
            publish(
                &sender,
                &frame_sender,
                Some(&replay_buffer),
                Some(&current_frame),
                pid,
                res,
            );
        }

        // 3. Signal guest
        create_game_signal.signal();

        // 4. Accept client + join
        let listener = TcpListener::bind(&self.listen_addr).await?;
        println!("[{pid}] Listening on ws://{}", self.listen_addr);
        ready_signal.signal_ready();
        let client_ws = accept_one_client(&listener, pid).await?;

        Self::accept_and_bridge(
            pid,
            client_ws,
            &mut up_w,
            &mut up_r,
            &sender,
            &frame_sender,
            join_barrier,
            multiplayer_ports,
            Some(&replay_buffer),
            Some(&current_frame),
            None,
            debug_tx,
            chat_tx,
            pause_state,
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
        let sender = self.sender.clone();
        let frame_sender = self.frame_sender.clone();
        let replay_buffer = Arc::clone(&self.replay_buffer);
        let current_frame = Arc::clone(&self.current_frame);

        // 1. Wait for game creation
        println!("[{pid}] Waiting for CreateGame signal...");
        create_game_signal.wait().await;
        println!("[{pid}] CreateGame signal received");

        // 2. Own upstream WS
        let upstream = connect_upstream(&self.upstream_url).await?;
        let (mut up_w, mut up_r) = upstream.split();

        // 3. Accept client + join
        let listener = TcpListener::bind(&self.listen_addr).await?;
        println!("[{pid}] Listening on ws://{}", self.listen_addr);
        ready_signal.signal_ready();
        let client_ws = accept_one_client(&listener, pid).await?;

        Self::accept_and_bridge(
            pid,
            client_ws,
            &mut up_w,
            &mut up_r,
            &sender,
            &frame_sender,
            join_barrier,
            multiplayer_ports,
            Some(&replay_buffer),
            Some(&current_frame),
            None,
            debug_tx,
            chat_tx,
            pause_state,
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
        let sender = self.sender.clone();
        let frame_sender = self.frame_sender.clone();
        let replay_buffer = Arc::clone(&self.replay_buffer);
        let current_frame = Arc::clone(&self.current_frame);

        let upstream = connect_upstream(&self.upstream_url).await?;
        let (mut up_w, mut up_r) = upstream.split();

        // CreateGame
        println!("[{pid}] Sending CreateGame");
        let cg_bytes = create_game_request
            .write_to_bytes()
            .map_err(|e| format!("Protobuf encode: {e}"))?;
        let cg_resp = roundtrip(&mut up_w, &mut up_r, cg_bytes).await?;

        if let Some(res) = try_parse_response(&cg_resp) {
            if res.has_create_game() {
                let cg = res.get_create_game();
                if cg.has_error() {
                    eprintln!(
                        "[{pid}] CreateGame error: {:?} - {}",
                        cg.get_error(),
                        cg.get_error_details()
                    );
                } else {
                    println!("[{pid}] CreateGame succeeded");
                }
            }
            publish(
                &sender,
                &frame_sender,
                Some(&replay_buffer),
                Some(&current_frame),
                pid,
                res,
            );
        }

        // Signal observer (if present) that CreateGame is done
        // (Not needed anymore — observer is interleaved in bridge loop)

        // Accept client
        let listener = TcpListener::bind(&self.listen_addr).await?;
        println!("[{pid}] Listening on ws://{}", self.listen_addr);
        ready_signal.signal_ready();
        let client_ws = accept_one_client(&listener, pid).await?;

        Self::accept_and_bridge(
            pid,
            client_ws,
            &mut up_w,
            &mut up_r,
            &sender,
            &frame_sender,
            None,
            None,
            Some(&replay_buffer),
            Some(&current_frame),
            observer_sender,
            debug_tx,
            chat_tx,
            pause_state,
        )
        .await
    }

    // ── Shared bridge logic ─────────────────────────────────────────────

    /// Accept first bot message (must be JoinGame), forward,
    /// fetch silent GameInfo, then enter the bridge loop.
    ///
    /// If `multiplayer_ports` is `Some`, the proxy injects `server_ports`
    /// and `client_ports` into the JoinGame request before forwarding it
    /// to SC2. This is required for multiplayer games.
    ///
    /// If `observer_sender` is `Some`, the bridge interleaves `disable_fog`
    /// observation requests after each bot roundtrip. Responses are published
    /// on the observer sender tagged as `Player2`, providing a full-visibility
    /// data stream for the observation pipeline.
    async fn accept_and_bridge(
        pid: PlayerId,
        client_ws: WsStream,
        up_w: &mut futures_util::stream::SplitSink<UpstreamWs, tungstenite::Message>,
        up_r: &mut futures_util::stream::SplitStream<UpstreamWs>,
        sender: &broadcast::Sender<TaggedResponse>,
        frame_sender: &broadcast::Sender<ReplayFrame>,
        join_barrier: Option<JoinResponseBarrier>,
        multiplayer_ports: Option<MultiplayerPorts>,
        replay_buffer: Option<&Arc<Mutex<ReplayBuffer>>>,
        current_frame: Option<&Arc<Mutex<CurrentFrame>>>,
        observer_sender: Option<broadcast::Sender<TaggedResponse>>,
        debug_tx: Option<mpsc::Sender<crate::debug_draw::DebugDrawEvent>>,
        chat_tx: Option<mpsc::Sender<crate::chat_overlay::ChatMessageEvent>>,
        pause_state: Option<Arc<AtomicBool>>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let (mut cw, mut cr) = client_ws.split();
        let mut pending_client: VecDeque<Vec<u8>> = VecDeque::new();

        //join game
        if let Some(join_msg) = cr.next().await {
            let join_msg = join_msg?;

            let raw = join_msg.into_data().to_vec();
            let parsed = try_parse_request(&raw);
            let is_join = parsed.as_ref().is_some_and(is_join_game);
            assert!(is_join);

            // Inject multiplayer ports if provided
            let raw = if let Some(ref mp) = multiplayer_ports {
                let mut req = parsed.unwrap(); // already verified is_join
                mp.inject_into(&mut req);
                req.write_to_bytes()
                    .map_err(|e| format!("Protobuf re-encode JoinGame: {e}"))?
            } else {
                raw
            };

            println!("[{pid}] Forwarding JoinGame");
            let resp = roundtrip(up_w, up_r, raw).await?;

            if let Some(res) = try_parse_response(&resp) {
                publish(sender, frame_sender, replay_buffer, current_frame, pid, res);
            }
            cw.send(tungstenite::Message::Binary(Bytes::from(resp)))
                .await?;

            if let Some(ref barrier) = join_barrier {
                barrier.mark_joined();
            }
        }

        // Wait for all players to have joined before starting the bridge loop.
        // This ensures neither bot starts sending game requests before both are in.
        if let Some(ref barrier) = join_barrier {
            barrier.wait_ready().await;
        }

        // If observer is present, fetch GameInfo for it (map data for pipeline).
        if let Some(ref obs_sender) = observer_sender {
            // Enable reveal-map debug mode in VsAI so observer polling can see
            // both players and neutrals from the shared Player1 connection.
            // let dbg_req = make_reveal_map_debug_request()?;
            // let _dbg_resp = roundtrip(up_w, up_r, dbg_req).await?;
            // println!("[{pid}] Observer: debug show_map enabled");

            let gi_req = make_game_info_request()?;
            let gi_resp = roundtrip(up_w, up_r, gi_req).await?;
            if let Some(res) = try_parse_response(&gi_resp) {
                publish(
                    obs_sender,
                    frame_sender,
                    replay_buffer,
                    current_frame,
                    PlayerId::Player2,
                    res,
                );
            }
            println!("[{pid}] Observer: initial GameInfo published");
        }

        let mut observer_last_game_loop: u32 = 0;

        loop {
            while pause_state
                .as_ref()
                .is_some_and(|state| state.load(Ordering::SeqCst))
            {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }

            let Some(msg) = cr.next().await else {
                break;
            };
            let msg = msg?;
            let raw = msg.into_data().to_vec();

            if !pending_client.is_empty() {
                let mut queued = VecDeque::new();
                std::mem::swap(&mut queued, &mut pending_client);
                for pending_raw in queued {
                    if let Some(ref tx) = debug_tx {
                        if let Some(evt) =
                            crate::debug_draw::debug_draw_from_request(pid, &pending_raw)
                        {
                            let _ = tx.try_send(evt);
                        }
                    }
                    if let Some(ref tx) = chat_tx {
                        for evt in
                            crate::chat_overlay::chat_messages_from_request(pid, &pending_raw)
                        {
                            let _ = tx.try_send(evt);
                        }
                    }

                    let resp = roundtrip(up_w, up_r, pending_raw).await?;
                    if let Some(res) = try_parse_response(&resp) {
                        publish(sender, frame_sender, replay_buffer, current_frame, pid, res);
                    }
                    cw.send(tungstenite::Message::Binary(Bytes::from(resp)))
                        .await?;

                    if let Some(ref obs_sender) = observer_sender {
                        let obs_req = make_disable_fog_observation_request()?;
                        let obs_resp = roundtrip(up_w, up_r, obs_req).await?;
                        if let Some(res) = try_parse_response(&obs_resp) {
                            let current_loop = if let Some(
                                sc2_proto::sc2api::Response_oneof_response::observation(ref obs),
                            ) = res.response
                            {
                                obs.observation
                                    .as_ref()
                                    .map(|inner| inner.get_game_loop())
                                    .unwrap_or(0)
                            } else {
                                0
                            };

                            if current_loop > observer_last_game_loop {
                                observer_last_game_loop = current_loop;
                                publish(
                                    obs_sender,
                                    frame_sender,
                                    replay_buffer,
                                    current_frame,
                                    PlayerId::Player2,
                                    res,
                                );
                            }
                        }
                    }
                }
            }

            if let Some(ref tx) = debug_tx {
                if let Some(evt) = crate::debug_draw::debug_draw_from_request(pid, &raw) {
                    let _ = tx.try_send(evt);
                }
            }
            if let Some(ref tx) = chat_tx {
                for evt in crate::chat_overlay::chat_messages_from_request(pid, &raw) {
                    let _ = tx.try_send(evt);
                }
            }

            let resp = roundtrip(up_w, up_r, raw).await?;
            if let Some(res) = try_parse_response(&resp) {
                publish(sender, frame_sender, replay_buffer, current_frame, pid, res);
            }
            cw.send(tungstenite::Message::Binary(Bytes::from(resp)))
                .await?;

            // Interleave observer: send a disable_fog observation after each
            // bot roundtrip. De-duplicated by game_loop so redundant polls
            // (e.g. after action requests in the same step) are dropped.
            if let Some(ref obs_sender) = observer_sender {
                let obs_req = make_disable_fog_observation_request()?;
                let obs_resp = roundtrip(up_w, up_r, obs_req).await?;
                if let Some(res) = try_parse_response(&obs_resp) {
                    let current_loop = if let Some(
                        sc2_proto::sc2api::Response_oneof_response::observation(ref obs),
                    ) = res.response
                    {
                        obs.observation
                            .as_ref()
                            .map(|inner| inner.get_game_loop())
                            .unwrap_or(0)
                    } else {
                        0
                    };

                    if current_loop > observer_last_game_loop {
                        let receivers = obs_sender.receiver_count();
                        if observer_last_game_loop == 0 {
                            println!(
                                "[{pid}] Observer: first obs at game_loop={current_loop}, \
                                 receivers={receivers}, response_type={}",
                                res.response
                                    .as_ref()
                                    .map(|r| format!("{:?}", std::mem::discriminant(r)))
                                    .unwrap_or_else(|| "None".to_string())
                            );
                        }
                        observer_last_game_loop = current_loop;
                        publish(
                            obs_sender,
                            frame_sender,
                            replay_buffer,
                            current_frame,
                            PlayerId::Player2,
                            res,
                        );
                    }
                } else {
                    static LOGGED_PARSE_FAIL: AtomicBool = AtomicBool::new(false);
                    if !LOGGED_PARSE_FAIL.swap(true, Ordering::Relaxed) {
                        eprintln!("[{pid}] Observer: failed to parse disable_fog response");
                    }
                }
            }
        }

        println!("[{pid}] Proxy finished.");
        Ok(())
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
}
