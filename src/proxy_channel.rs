//! Reactive proxy data channel for SC2 bot communication.
//!
//! Each bot gets a `ProxyDataChannel` that:
//! - Runs a WebSocket proxy bridging bot ↔ SC2 server
//! - Publishes all SC2 Response messages to a broadcast channel
//! - Consumers can subscribe to get a Stream of responses

use futures_util::{future, SinkExt, StreamExt};
use sc2_proto::sc2api::{Request, Response};
use tokio::net::TcpListener;
use tokio::sync::broadcast;
use tokio_tungstenite::{accept_async, connect_async, tungstenite::Result};
use protobuf::Message;

/// Size of the broadcast channel buffer.
/// Messages are dropped if consumers fall behind.
const CHANNEL_BUFFER_SIZE: usize = 64;

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

/// A reactive proxy channel for a single bot.
///
/// Creates a WebSocket proxy that bridges bot ↔ SC2 and emits
/// all Response messages to subscribers via a broadcast channel.
pub struct ProxyDataChannel {
    pub player_id: PlayerId,
    pub listen_addr: String,
    pub upstream_url: String,
    sender: broadcast::Sender<TaggedResponse>,
}

/// Response tagged with its source player.
#[derive(Debug, Clone)]
pub struct TaggedResponse {
    pub player_id: PlayerId,
    pub response: Response,
}

impl ProxyDataChannel {
    /// Create a new proxy data channel.
    ///
    /// Returns the channel and a receiver that can be used to subscribe.
    /// Additional receivers can be obtained via `subscribe()`.
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

    /// Get a new subscriber to this channel's response stream.
    pub fn subscribe(&self) -> broadcast::Receiver<TaggedResponse> {
        self.sender.subscribe()
    }

    /// Get a clone of the sender for external use.
    pub fn sender(&self) -> broadcast::Sender<TaggedResponse> {
        self.sender.clone()
    }

    /// Run the proxy: wait for one client, then bridge traffic until closed.
    ///
    /// All Response messages from SC2 are published to the broadcast channel.
    pub async fn run(self) -> Result<()> {
        let player_id = self.player_id;
        let sender = self.sender.clone();

        // 1. Connect upstream to the real SC2 server
        let mut retries = 5;
        let delay_secs = 2;
        println!("[{}] Connecting upstream to {}", player_id, self.upstream_url);

        let upstream_ws = loop {
            match connect_async(&self.upstream_url).await {
                Ok((ws, _)) => {
                    println!("[{}] Connected to upstream.", player_id);
                    break ws;
                }
                Err(e) => {
                    println!(
                        "[{}] Failed to connect upstream: {}. Retries left: {}",
                        player_id, e, retries
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

        // 2. Wait for a single client (bot) to connect
        let listener = TcpListener::bind(&self.listen_addr).await?;
        println!("[{}] Waiting for client on ws://{}", player_id, self.listen_addr);
        let (client_stream, addr) = listener.accept().await?;
        println!("[{}] Client connected from {}", player_id, addr);
        let client_ws = accept_async(client_stream).await?;

        // 3. Proxy messages in both directions
        let (mut client_write, mut client_read) = client_ws.split();

        // Client → Server: forward requests
        let c2s = async {
            while let Some(msg) = client_read.next().await {
                let msg = msg?;

                // Parse for debugging (optional)
                let mut req = Request::new();
                if let Ok(_) = req.merge_from_bytes(msg.clone().into_data().iter().as_slice()) {
                    // Could log request type here if needed
                }

                upstream_write.send(msg).await?;
            }
            Ok::<_, tungstenite::Error>(())
        };

        // Server → Client: forward responses AND publish to channel
        let s2c = {
            let sender = sender.clone();
            async move {
                while let Some(msg) = upstream_read.next().await {
                    let msg = msg?;

                    // Parse response and publish to channel
                    let mut res = Response::new();
                    if res.merge_from_bytes(msg.clone().into_data().iter().as_slice()).is_ok() {
                        // Publish to broadcast channel (ignore send errors - no receivers is ok)
                        let _ = sender.send(TaggedResponse {
                            player_id,
                            response: res,
                        });
                    }

                    // Forward to client
                    client_write.send(msg).await?;
                }
                Ok::<_, tungstenite::Error>(())
            }
        };

        // Wait for either direction to finish
        match future::select(Box::pin(c2s), Box::pin(s2c)).await {
            future::Either::Left((res, _)) => {
                if let Err(e) = res {
                    eprintln!("[{}] client → server ended with error: {}", player_id, e);
                } else {
                    println!("[{}] client → server closed normally", player_id);
                }
            }
            future::Either::Right((res, _)) => {
                if let Err(e) = res {
                    eprintln!("[{}] server → client ended with error: {}", player_id, e);
                } else {
                    println!("[{}] server → client closed normally", player_id);
                }
            }
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
