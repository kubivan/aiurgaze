//! Reactive proxy data channel for SC2 bot communication.
//!
//! Each bot gets a `ProxyDataChannel` that:
//! - Runs a WebSocket proxy bridging bot ↔ SC2 server
//! - Publishes all SC2 Response messages to a broadcast channel
//! - Consumers can subscribe to get a Stream of responses
//!
//! For bot-vs-bot, both proxies share a SINGLE upstream WS connection to SC2
//! (SC2 headless only accepts one WebSocket per process).
//! Requests are serialized through an mpsc channel.
//! JoinGame requests have multiplayer port fields stripped so SC2 treats
//! them as single-process joins (same approach as stephanzlatarev/starcraft go.js).

use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use sc2_proto::sc2api::{Request, Request_oneof_request, Response, Response_oneof_response};
use tokio::net::TcpListener;
use tokio::sync::{broadcast, mpsc, oneshot, Notify};
use tokio_tungstenite::{accept_async, connect_async};
use tungstenite;
use protobuf::Message;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

/// Size of the broadcast channel buffer.
/// Messages are dropped if consumers fall behind.
const CHANNEL_BUFFER_SIZE: usize = 64;

/// Shared signal indicating how many proxy listeners are ready.
/// Bots should wait for this before connecting.
#[derive(Debug, Clone, Default)]
pub struct ProxyReadySignal {
    ready_count: Arc<AtomicU8>,
    expected_count: Arc<AtomicU8>,
}

impl ProxyReadySignal {
    /// Create a new signal expecting `count` proxies to become ready.
    pub fn new(expected_count: u8) -> Self {
        Self {
            ready_count: Arc::new(AtomicU8::new(0)),
            expected_count: Arc::new(AtomicU8::new(expected_count)),
        }
    }

    /// Signal that one proxy is ready.
    pub fn signal_ready(&self) {
        let prev = self.ready_count.fetch_add(1, Ordering::SeqCst);
        println!("[ProxyReadySignal] Proxy ready ({}/{})", prev + 1, self.expected_count.load(Ordering::SeqCst));
    }

    /// Check if all expected proxies are ready.
    pub fn is_ready(&self) -> bool {
        self.ready_count.load(Ordering::SeqCst) >= self.expected_count.load(Ordering::SeqCst)
    }

    /// Check if at least `count` proxies are ready.
    pub fn has_count(&self, count: u8) -> bool {
        self.ready_count.load(Ordering::SeqCst) >= count
    }

    /// Wait until all proxies are ready (async polling).
    pub async fn wait_ready(&self) {
        while !self.is_ready() {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }

    /// Wait until at least `count` proxies are ready (async polling).
    pub async fn wait_for_count(&self, count: u8) {
        while !self.has_count(count) {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }
}

/// Barrier that holds all proxy communication until every proxy has received
/// its JoinGame response (and fetched silent GameInfo).
///
/// In bot-vs-bot, SC2 sends JoinGame responses once both players join.
/// This barrier ensures neither proxy forwards further game traffic until
/// ALL proxies have fully joined, so both bots start stepping simultaneously.
#[derive(Debug, Clone)]
pub struct JoinBarrier {
    join_count: Arc<AtomicU8>,
    notify: Arc<Notify>,
    expected: u8,
}

impl JoinBarrier {
    pub fn new(expected: u8) -> Self {
        Self {
            join_count: Arc::new(AtomicU8::new(0)),
            notify: Arc::new(Notify::new()),
            expected,
        }
    }

    /// Called by each proxy after it has processed JoinGame response.
    /// Blocks until all proxies have arrived.
    pub async fn arrive_and_wait(&self) {
        let prev = self.join_count.fetch_add(1, Ordering::SeqCst);
        let current = prev + 1;
        println!("[JoinBarrier] Proxy arrived ({}/{})", current, self.expected);
        if current >= self.expected {
            self.notify.notify_waiters();
        } else {
            self.notify.notified().await;
        }
        println!("[JoinBarrier] Barrier released");
    }
}

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

/// A request sent through the SharedUpstream serializer.
struct UpstreamRequest {
    data: Vec<u8>,
    reply: oneshot::Sender<Vec<u8>>,
    /// JoinGame requests are batched: the serializer accumulates them until
    /// `expected_joins` arrive, sends all, then collects all responses.
    /// This avoids a deadlock where SC2 holds the first JoinGame response
    /// until the second JoinGame arrives.
    is_join_game: bool,
}

/// Shared upstream connection to SC2.
///
/// SC2 headless accepts only ONE WebSocket connection per process.
/// All proxies share this single connection, with requests serialized
/// through an mpsc channel (like go.js's `while (request) await sleep(10)`).
///
/// JoinGame requests are special: SC2 won't respond to the first JoinGame
/// until ALL players have joined. The serializer batches JoinGames and sends
/// them all before waiting for responses.
#[derive(Clone)]
pub struct SharedUpstream {
    tx: mpsc::Sender<UpstreamRequest>,
}

impl SharedUpstream {
    /// Connect to SC2 upstream and optionally send a CreateGame request first.
    ///
    /// `expected_joins` — how many JoinGame requests to batch before waiting
    /// for responses (1 for VsAI, 2 for VsBot).
    ///
    /// Spawns a background task that owns the upstream WS and serializes
    /// all requests from proxies. Returns the SharedUpstream handle.
    pub async fn connect(
        upstream_url: &str,
        create_game_request: Option<Request>,
        publish_sender: broadcast::Sender<TaggedResponse>,
        expected_joins: u8,
    ) -> Result<Self, tungstenite::Error> {
        // Connect upstream to SC2 with retries
        let mut retries = 10;
        let delay_secs = 2;
        println!("[SharedUpstream] Connecting to {}", upstream_url);

        let upstream_ws = loop {
            match connect_async(upstream_url).await {
                Ok((ws, _)) => {
                    println!("[SharedUpstream] Connected to SC2");
                    break ws;
                }
                Err(e) => {
                    println!(
                        "[SharedUpstream] Failed to connect: {}. Retries left: {}",
                        e, retries
                    );
                    if retries == 0 {
                        return Err(e);
                    }
                    retries -= 1;
                    tokio::time::sleep(std::time::Duration::from_secs(delay_secs)).await;
                }
            }
        };

        let (mut upstream_write, mut upstream_read) = upstream_ws.split();

        // Optionally send CreateGame before anything else
        if let Some(req) = create_game_request {
            println!("[SharedUpstream] Sending CreateGame request");
            let bytes = req.write_to_bytes()
                .map_err(|e| tungstenite::Error::Io(
                    std::io::Error::new(std::io::ErrorKind::Other, format!("Protobuf error: {}", e))
                ))?;
            upstream_write.send(tungstenite::Message::Binary(Bytes::from(bytes))).await?;

            // Wait for CreateGame response
            if let Some(msg) = upstream_read.next().await {
                let msg = msg?;
                let mut res = Response::new();
                if res.merge_from_bytes(msg.into_data().iter().as_slice()).is_ok() {
                    if res.has_create_game() {
                        let cg = res.get_create_game();
                        if cg.has_error() {
                            eprintln!("[SharedUpstream] CreateGame error: {:?} - {}",
                                cg.get_error(), cg.get_error_details());
                        } else {
                            println!("[SharedUpstream] CreateGame succeeded");
                        }
                    }
                    // Publish CreateGame response to pipeline (tag as Player1 since it initiated)
                    let _ = publish_sender.send(TaggedResponse {
                        player_id: PlayerId::Player1,
                        response: res,
                    });
                }
            }
        }

        // Create the request serializer channel
        let (tx, mut rx) = mpsc::channel::<UpstreamRequest>(32);

        // Spawn the upstream serializer task
        tokio::spawn(async move {
            // Accumulated JoinGame requests waiting to be batched
            let mut pending_joins: Vec<(Vec<u8>, oneshot::Sender<Vec<u8>>)> = Vec::new();

            while let Some(req) = rx.recv().await {
                if req.is_join_game {
                    // Accumulate JoinGame — don't send yet
                    println!("[SharedUpstream] JoinGame request queued ({}/{})",
                        pending_joins.len() + 1, expected_joins);
                    pending_joins.push((req.data, req.reply));

                    if pending_joins.len() >= expected_joins as usize {
                        // All JoinGames collected — send all to SC2
                        println!("[SharedUpstream] Sending {} batched JoinGame requests", pending_joins.len());
                        for (data, _) in &pending_joins {
                            if let Err(e) = upstream_write.send(
                                tungstenite::Message::Binary(Bytes::from(data.clone()))
                            ).await {
                                eprintln!("[SharedUpstream] Failed to send JoinGame to SC2: {}", e);
                            }
                        }

                        // Collect all responses (one per JoinGame)
                        for (_, reply) in pending_joins.drain(..) {
                            match upstream_read.next().await {
                                Some(Ok(msg)) => {
                                    let _ = reply.send(msg.into_data().to_vec());
                                }
                                Some(Err(e)) => {
                                    eprintln!("[SharedUpstream] Error reading JoinGame response: {}", e);
                                    let _ = reply.send(Vec::new());
                                }
                                None => {
                                    eprintln!("[SharedUpstream] SC2 closed during JoinGame");
                                    let _ = reply.send(Vec::new());
                                }
                            }
                        }
                        println!("[SharedUpstream] All JoinGame responses received");
                    }
                    continue;
                }

                // Normal request-response (one at a time)
                if let Err(e) = upstream_write.send(
                    tungstenite::Message::Binary(Bytes::from(req.data))
                ).await {
                    eprintln!("[SharedUpstream] Failed to send to SC2: {}", e);
                    let _ = req.reply.send(Vec::new());
                    continue;
                }

                match upstream_read.next().await {
                    Some(Ok(msg)) => {
                        let _ = req.reply.send(msg.into_data().to_vec());
                    }
                    Some(Err(e)) => {
                        eprintln!("[SharedUpstream] Error reading from SC2: {}", e);
                        let _ = req.reply.send(Vec::new());
                        break;
                    }
                    None => {
                        eprintln!("[SharedUpstream] SC2 connection closed");
                        let _ = req.reply.send(Vec::new());
                        break;
                    }
                }
            }
            println!("[SharedUpstream] Serializer task finished");
        });

        Ok(Self { tx })
    }

    /// Send a raw request to SC2 and wait for the response.
    /// Requests are serialized — only one in-flight at a time.
    pub async fn request(&self, data: Vec<u8>) -> Result<Vec<u8>, String> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx.send(UpstreamRequest { data, reply: reply_tx, is_join_game: false })
            .await
            .map_err(|_| "SharedUpstream channel closed".to_string())?;
        reply_rx.await.map_err(|_| "SharedUpstream reply dropped".to_string())
    }

    /// Send a JoinGame request to SC2.
    /// JoinGame requests are batched — the serializer accumulates them until
    /// `expected_joins` arrive, sends all, then collects all responses.
    /// This prevents deadlock where SC2 holds the first JoinGame response.
    pub async fn join_game_request(&self, data: Vec<u8>) -> Result<Vec<u8>, String> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx.send(UpstreamRequest { data, reply: reply_tx, is_join_game: true })
            .await
            .map_err(|_| "SharedUpstream channel closed".to_string())?;
        reply_rx.await.map_err(|_| "SharedUpstream reply dropped".to_string())
    }
}

/// Strip multiplayer port fields from a JoinGame request.
///
/// SC2 headless in single-process mode doesn't need serverPorts, clientPorts,
/// sharedPort, or hostIp. Keeping them causes "Already in a game" errors
/// when the second player tries to join.
/// This mirrors stephanzlatarev/starcraft go.js `replaceJoinRequest()`.
fn strip_join_game_ports(req: &mut Request) {
    if let Some(Request_oneof_request::join_game(ref mut join)) = req.request {
        join.clear_server_ports();
        join.clear_client_ports();
        join.clear_shared_port();
        join.clear_host_ip();
        println!("[strip_join_game_ports] Stripped multiplayer port fields, keeping race={:?} options={:?}",
            join.get_race(), join.has_options());
    }
}

/// Response tagged with its source player.
#[derive(Debug, Clone)]
pub struct TaggedResponse {
    pub player_id: PlayerId,
    pub response: Response,
}

/// A reactive proxy channel for a single bot.
///
/// Accepts one bot client on a local port and sends requests through
/// the SharedUpstream connection to SC2.
pub struct ProxyDataChannel {
    pub player_id: PlayerId,
    pub listen_addr: String,
    sender: broadcast::Sender<TaggedResponse>,
}

impl ProxyDataChannel {
    /// Create a new proxy data channel.
    ///
    /// Returns the channel and a receiver that can be used to subscribe.
    /// Additional receivers can be obtained via `subscribe()`.
    pub fn new(
        player_id: PlayerId,
        listen_addr: impl Into<String>,
    ) -> (Self, broadcast::Receiver<TaggedResponse>) {
        let (sender, receiver) = broadcast::channel(CHANNEL_BUFFER_SIZE);
        (
            Self {
                player_id,
                listen_addr: listen_addr.into(),
                sender,
            },
            receiver,
        )
    }

    /// Get a new subscriber to this channel's response stream.
    pub fn subscribe(&self) -> broadcast::Receiver<TaggedResponse> {
        self.sender.subscribe()
    }

    /// Get a clone of the sender for external use.
    pub fn sender(&self) -> broadcast::Sender<TaggedResponse> {
        self.sender.clone()
    }

    /// Run the proxy: accept one bot client, bridge traffic through SharedUpstream.
    ///
    /// All Response messages from SC2 are published to the broadcast channel.
    /// JoinGame requests have multiplayer port fields stripped before forwarding.
    /// If `join_barrier` is provided, holds further communication after JoinGame
    /// response until every proxy has joined.
    pub async fn run(
        self,
        ready_signal: Option<ProxyReadySignal>,
        upstream: SharedUpstream,
        join_barrier: Option<JoinBarrier>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let player_id = self.player_id;
        let sender = self.sender.clone();

        // 1. Accept one bot client
        let listener = TcpListener::bind(&self.listen_addr).await?;
        println!("[{}] Listener bound on ws://{}", player_id, self.listen_addr);

        if let Some(signal) = &ready_signal {
            signal.signal_ready();
        }

        println!("[{}] Waiting for client connection...", player_id);
        let client_ws = loop {
            let (client_stream, addr) = listener.accept().await?;
            println!("[{}] Client connected from {}", player_id, addr);
            match accept_async(client_stream).await {
                Ok(ws) => break ws,
                Err(e) => {
                    eprintln!("[{}] WebSocket handshake failed (retrying): {}", player_id, e);
                    continue;
                }
            }
        };

        // 2. Bridge traffic: client → SharedUpstream → client
        let (mut client_write, mut client_read) = client_ws.split();
        let mut join_done = false;

        while let Some(msg) = client_read.next().await {
            let msg = msg?;
            let raw = msg.into_data().to_vec();

            // Parse the request to detect JoinGame
            let mut req = Request::new();
            let is_join = if req.merge_from_bytes(&raw).is_ok() {
                matches!(req.request, Some(Request_oneof_request::join_game(_)))
            } else {
                false
            };

            // Determine what bytes to send upstream
            let send_bytes = if is_join && !join_done {
                // Strip multiplayer port fields from JoinGame
                strip_join_game_ports(&mut req);
                println!("[{}] Sending stripped JoinGame request", player_id);
                req.write_to_bytes()
                    .map_err(|e| format!("Protobuf serialize error: {}", e))?
            } else {
                raw
            };

            // Send through SharedUpstream
            // JoinGame uses batched path to avoid deadlock (SC2 holds
            // first JoinGame response until all players have joined)
            let response_bytes = if is_join && !join_done {
                upstream.join_game_request(send_bytes).await?
            } else {
                upstream.request(send_bytes).await?
            };
            if response_bytes.is_empty() {
                eprintln!("[{}] Empty response from SC2, connection may be lost", player_id);
                break;
            }

            // Parse and publish response
            let mut res = Response::new();
            if res.merge_from_bytes(&response_bytes).is_ok() {
                // Publish to broadcast channel for the observation pipeline
                let _ = sender.send(TaggedResponse { player_id, response: res.clone() });

                // Handle JoinGame response: barrier + silent GameInfo
                if is_join && !join_done {
                    join_done = true;
                    println!("[{}] JoinGame response received", player_id);

                    // Forward JoinGame response to bot
                    client_write.send(tungstenite::Message::Binary(
                        Bytes::from(response_bytes)
                    )).await?;

                    // Wait at barrier for all proxies to join
                    if let Some(ref barrier) = join_barrier {
                        println!("[{}] Waiting at JoinBarrier", player_id);
                        barrier.arrive_and_wait().await;
                        println!("[{}] JoinBarrier released", player_id);
                    }

                    // Send silent GameInfo request to get map data
                    println!("[{}] Sending silent GameInfo request after JoinGame", player_id);
                    let mut game_info_req = Request::new();
                    game_info_req.mut_game_info();
                    let gi_bytes = game_info_req.write_to_bytes()
                        .map_err(|e| format!("Protobuf serialize error: {}", e))?;
                    let gi_response_bytes = upstream.request(gi_bytes).await?;

                    if !gi_response_bytes.is_empty() {
                        let mut gi_res = Response::new();
                        if gi_res.merge_from_bytes(&gi_response_bytes).is_ok() {
                            println!("[{}] Received silent GameInfo response (map data)", player_id);
                            let _ = sender.send(TaggedResponse { player_id, response: gi_res });
                        }
                    }

                    continue; // JoinGame response already forwarded
                }
            }

            // Forward response to bot client
            client_write.send(tungstenite::Message::Binary(
                Bytes::from(response_bytes)
            )).await?;
        }

        println!("[{}] Proxy finished.", player_id);
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
