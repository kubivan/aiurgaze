use futures_util::{future, StreamExt, SinkExt};
use sc2_proto::sc2api::{Request, Response};
use tokio::net::{TcpListener};
use tokio_tungstenite::{accept_async, connect_async, tungstenite::Result};

use protobuf::Message;
use std::sync::Arc;

/// ProxyWS holds:
///  * listener address for incoming client
///  * URL of the upstream server we proxy to
///  * callback for emitting responses
///
pub struct ProxyWS<F>
where
    F: Fn(Response) + Send + Sync + 'static,
{
    listen_addr: String,
    upstream_url: String,
    on_response: Arc<F>,
}

impl<F> ProxyWS<F>
where
    F: Fn(Response) + Send + Sync + 'static,
{
    pub fn new(
        listen_addr: impl Into<String>,
        upstream_url: impl Into<String>,
        on_response: F,
    ) -> Self {
        Self {
            listen_addr: listen_addr.into(),
            upstream_url: upstream_url.into(),
            on_response: Arc::new(on_response),
        }
    }

    /// Run the proxy: wait for **one** client, then bridge traffic until closed.
    pub async fn run(self) -> Result<()> {
        let on_response = self.on_response.clone();
        let mut retries = 5;
        let delay_secs = 2;
        let mut last_err = None;
        //1. Connect upstream to the real server.
        println!("Connecting upstream to {}", self.upstream_url);
        let upstream_ws = loop {
            match connect_async(&self.upstream_url).await {
                Ok((ws, _)) => {
                    println!("Connected to upstream.");
                    break ws;
                }
                Err(e) => {
                    println!("Failed to connect upstream: {}. Retries left: {}", e, retries);
                    last_err = Some(e);
                    if retries == 0 {
                        return Err(last_err.unwrap());
                    }
                    retries -= 1;
                    tokio::time::sleep(std::time::Duration::from_secs(delay_secs)).await;
                }
            }
        };
        let (mut upstream_write, mut upstream_read) = upstream_ws.split();

        // 3. Wait for a single client to connect.
        let listener = TcpListener::bind(&self.listen_addr).await?;
        println!("Waiting for client on ws://{}", self.listen_addr);
        let (client_stream, addr) = listener.accept().await?;
        println!("Client connected from {}", addr);
        let client_ws = accept_async(client_stream).await?;

        // 4. Proxy messages in both directions until either side closes.
        let (mut client_write, mut client_read) = client_ws.split();

        let c2s = async {
            while let Some(msg) = client_read.next().await {
                let msg = msg?;

                let mut req = Request::new();
                req.merge_from_bytes(msg.clone().into_data().iter().as_slice()).unwrap();

                upstream_write.send(msg).await?;
            }
            Ok::<_, tungstenite::Error>(())
        };

        let s2c = async {
            while let Some(msg) = upstream_read.next().await {
                let msg = msg?;

                let mut res = Response::new();
                res.merge_from_bytes(msg.clone().into_data().iter().as_slice()).ok();
                
                // Call the callback with the response
                (on_response)(res);

                client_write.send(msg).await?;
            }
            Ok::<_, tungstenite::Error>(())
        };

        // Wait for either direction to finish and log if it's an error
        match future::select(Box::pin(c2s), Box::pin(s2c)).await {
            future::Either::Left((res, _)) => {
                if let Err(e) = res {
                    eprintln!("client → server forwarding ended with error: {e}");
                } else {
                    println!("client → server closed normally");
                }
            }
            future::Either::Right((res, _)) => {
                if let Err(e) = res {
                    eprintln!("server → client forwarding ended with error: {e}");
                } else {
                    println!("server → client closed normally");
                }
            }
        }
        println!("Proxy finished.");
        Ok(())
    }
}

/// ObserverClient connects directly to SC2 as an observer and emits responses.
pub struct ObserverClient<F>
where
    F: Fn(Response) + Send + Sync + 'static,
{
    upstream_url: String,
    on_response: Arc<F>,
}

impl<F> ObserverClient<F>
where
    F: Fn(Response) + Send + Sync + 'static,
{
    pub fn new(
        upstream_url: impl Into<String>,
        on_response: F,
    ) -> Self {
        Self {
            upstream_url: upstream_url.into(),
            on_response: Arc::new(on_response),
        }
    }

    /// Run the observer: connect to SC2, send JoinGame as observer, then read responses.
    pub async fn run(self) -> Result<()> {
        use sc2_proto::sc2api::RequestJoinGame;
        use bytes::Bytes;

        let on_response = self.on_response.clone();
        let mut retries = 20;  // Increased retries for observer
        let delay_secs = 1;     // More frequent retries

        // Wait before connecting to ensure proxy establishes first
        println!("Observer: Waiting 2 seconds for proxy to establish...");
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;

        println!("Observer connecting to {}", self.upstream_url);
        let ws_stream = loop {
            match connect_async(&self.upstream_url).await {
                Ok((ws, _)) => {
                    println!("Observer connected to SC2.");
                    break ws;
                }
                Err(e) => {
                    println!("Observer failed to connect: {}. Retries left: {}", e, retries);
                    if retries == 0 {
                        eprintln!("Observer: Failed to connect after all retries. Giving up.");
                        return Ok(()); // Exit gracefully instead of erroring
                    }
                    retries -= 1;
                    tokio::time::sleep(std::time::Duration::from_secs(delay_secs)).await;
                }
            }
        };

        let (mut write, mut read) = ws_stream.split();

        // Send JoinGame request as observer
        let mut join_req = Request::new();
        let mut join_game = RequestJoinGame::new();
        // Join as observer by setting observed player to 0 (any)
        join_game.set_observed_player_id(0);
        join_req.set_join_game(join_game);

        let bytes = match join_req.write_to_bytes() {
            Ok(b) => b,
            Err(e) => {
                eprintln!("Observer: Failed to serialize JoinGame: {}", e);
                return Ok(());
            }
        };
        
        if let Err(e) = write.send(tungstenite::Message::Binary(Bytes::from(bytes))).await {
            eprintln!("Observer: Failed to send JoinGame: {}", e);
            return Ok(());
        }
        println!("Observer: JoinGame request sent");

        // Send RequestGameInfo to get map info
        let mut game_info_req = Request::new();
        game_info_req.mut_game_info();
        let game_info_bytes = match game_info_req.write_to_bytes() {
            Ok(b) => b,
            Err(e) => {
                eprintln!("Observer: Failed to serialize GameInfo request: {}", e);
                return Ok(());
            }
        };
        if let Err(e) = write.send(tungstenite::Message::Binary(Bytes::from(game_info_bytes))).await {
            eprintln!("Observer: Failed to send GameInfo request: {}", e);
            return Ok(());
        }
        println!("Observer: RequestGameInfo sent");

        // Read responses and emit via callback
        while let Some(msg) = read.next().await {
            match msg {
                Ok(tungstenite::Message::Binary(data)) => {
                    let mut res = Response::new();
                    if res.merge_from_bytes(data.iter().as_slice()).is_ok() {
                        // Call the callback with the response
                        (on_response)(res);
                    }
                }
                Err(e) => {
                    eprintln!("Observer: Connection error: {}", e);
                    break;
                }
                _ => {}
            }
        }

        println!("Observer connection closed.");
        Ok(())
    }
}