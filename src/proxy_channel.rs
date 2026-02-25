//! Reactive proxy data channel for SC2 bot communication.
#![allow(dead_code)]
//!
//! Each bot gets its own `ProxyDataChannel` with an **independent** upstream
//! WS connection to SC2. SC2 headless accepts one WS per player — no sharing.
//!
//! Coordination between proxies is minimal:
//! - Host sends CreateGame, then signals via `CreateGameSignal`.
//! - Both proxies independently send JoinGame on their own WS.
//! - After JoinGame, each proxy fetches a silent GameInfo for map data.
//! - Then the proxy enters the bridge loop forwarding traffic in both directions.

use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use protobuf::{Message, RepeatedField};
use sc2_proto::sc2api::{PortSet, Request, Request_oneof_request, Response};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::{broadcast, Notify};
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

/// Response tagged with its source player.
#[derive(Debug, Clone)]
pub struct TaggedResponse {
    pub player_id: PlayerId,
    pub response: Response,
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
fn publish(sender: &broadcast::Sender<TaggedResponse>, player_id: PlayerId, res: Response) {
    let _ = sender.send(TaggedResponse {
        player_id,
        response: res,
    });
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
        (
            Self {
                player_id,
                listen_addr: listen_addr.into(),
                upstream_url: upstream_url.into(),
                sender,
            },
            receiver,
        )
    }

    /// Subscribe to this channel's response stream.
    pub fn subscribe(&self) -> broadcast::Receiver<TaggedResponse> {
        self.sender.subscribe()
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
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let pid = self.player_id;
        let sender = self.sender.clone();

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
            publish(&sender, pid, res);
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
            join_barrier,
            multiplayer_ports,
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
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let pid = self.player_id;
        let sender = self.sender.clone();

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
            join_barrier,
            multiplayer_ports,
        )
        .await
    }

    /// Solo mode (VsAI — single bot):
    /// 1. Connect upstream WS
    /// 2. Send CreateGame, publish response
    /// 3. Accept bot client, send JoinGame + silent GameInfo
    /// 4. Bridge loop
    pub async fn run_solo(
        self,
        ready_signal: ProxyReadySignal,
        create_game_request: Request,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let pid = self.player_id;
        let sender = self.sender.clone();

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
            publish(&sender, pid, res);
        }

        // Accept client
        let listener = TcpListener::bind(&self.listen_addr).await?;
        println!("[{pid}] Listening on ws://{}", self.listen_addr);
        ready_signal.signal_ready();
        let client_ws = accept_one_client(&listener, pid).await?;

        Self::accept_and_bridge(pid, client_ws, &mut up_w, &mut up_r, &sender, None, None).await
    }

    // ── Shared bridge logic ─────────────────────────────────────────────

    /// Accept first bot message (must be JoinGame), forward,
    /// fetch silent GameInfo, then enter the bridge loop.
    ///
    /// If `multiplayer_ports` is `Some`, the proxy injects `server_ports`
    /// and `client_ports` into the JoinGame request before forwarding it
    /// to SC2. This is required for multiplayer games.
    async fn accept_and_bridge(
        pid: PlayerId,
        client_ws: WsStream,
        up_w: &mut futures_util::stream::SplitSink<UpstreamWs, tungstenite::Message>,
        up_r: &mut futures_util::stream::SplitStream<UpstreamWs>,
        sender: &broadcast::Sender<TaggedResponse>,
        join_barrier: Option<JoinResponseBarrier>,
        multiplayer_ports: Option<MultiplayerPorts>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let (mut cw, mut cr) = client_ws.split();

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
                publish(sender, pid, res);
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

        while let Some(msg) = cr.next().await {
            let msg = msg?;
            let raw = msg.into_data().to_vec();

            let resp = roundtrip(up_w, up_r, raw).await?;
            if let Some(res) = try_parse_response(&resp) {
                publish(sender, pid, res);
            }
            cw.send(tungstenite::Message::Binary(Bytes::from(resp)))
                .await?;
        }

        println!("[{pid}] Proxy finished.");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_player_id_display() {
        assert_eq!(format!("{}", PlayerId::Player1), "Player1");
        assert_eq!(format!("{}", PlayerId::Player2), "Player2");
    }
}
