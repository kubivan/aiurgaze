use futures_util::{future, StreamExt, SinkExt};
use sc2_proto::sc2api::{Request, Response, ResponseObservation};
use tokio::net::TcpListener;
use tokio_tungstenite::{accept_async, connect_async, tungstenite::Result};

use protobuf::Message;
use std::sync::Arc;
use tokio::sync::Mutex;

/// ProxyWS holds:
///  * listener address for incoming client
///  * URL of the upstream server we proxy to
///  * callback for emitting responses
///  * buffers for GameInfo and latest Observation
///  * name for logging
///
pub struct ProxyWS<F>
where
    F: Fn(Response) + Send + Sync + 'static,
{
    name: String,
    listen_addr: String,
    upstream_url: String,
    on_response: Arc<F>,
    pub last_observation: Arc<Mutex<Option<ResponseObservation>>>,
}

impl<F> ProxyWS<F>
where
    F: Fn(Response) + Send + Sync + 'static,
{
    pub fn new(
        name: impl Into<String>,
        listen_addr: impl Into<String>,
        upstream_url: impl Into<String>,
        on_response: F,
    ) -> Self {
        Self {
            name: name.into(),
            listen_addr: listen_addr.into(),
            upstream_url: upstream_url.into(),
            on_response: Arc::new(on_response),
            last_observation: Arc::new(Mutex::new(None)),
        }
    }

    /// Run the proxy: wait for **one** client, then bridge traffic until closed.
    /// After JoinedResponse, request GameInfo manually.
    pub async fn run(self) -> Result<()> {
        use bytes::Bytes;
        use sc2_proto::sc2api::Response_oneof_response;

        let name = self.name.clone();
        let on_response = self.on_response.clone();
        let last_obs = self.last_observation.clone();
        let mut retries = 5;
        let delay_secs = 2;
        let mut last_err = None;

        // 1. Wait for a single client to connect first.
        let listener = TcpListener::bind(&self.listen_addr).await?;
        println!("[{}] Waiting for client on ws://{}", name, self.listen_addr);
        let (client_stream, addr) = listener.accept().await?;
        println!("[{}] Client connected from {}", name, addr);
        let client_ws = accept_async(client_stream).await?;

        // 2. Connect upstream to the real server after client connects.
        println!("[{}] Connecting upstream to {}", name, self.upstream_url);
        let upstream_ws = loop {
            match connect_async(&self.upstream_url).await {
                Ok((ws, _)) => {
                    println!("[{}] Connected to upstream.", name);
                    break ws;
                }
                Err(e) => {
                    println!("[{}] Failed to connect upstream: {}. Retries left: {}", name, e, retries);
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

        // Proxy messages in both directions until either side closes.
        let (mut client_write, mut client_read) = client_ws.split();
        let upstream_write_arc = Arc::new(Mutex::new(upstream_write));

        let c2s = {
            let upstream_write = upstream_write_arc.clone();
            async move {
                while let Some(msg) = client_read.next().await {
                    let msg = msg?;
                    let mut upstream = upstream_write.lock().await;
                    upstream.send(msg).await?;
                }
                Ok::<_, tungstenite::Error>(())
            }
        };

        let s2c = {
            let upstream_write = upstream_write_arc.clone();
            let on_response = on_response.clone();
            let last_obs = last_obs.clone();
            let name_clone = name.clone();
            async move {
                let mut game_info_requested = false;
                let mut waiting_for_game_info = false;
                while let Some(msg) = upstream_read.next().await {
                    let msg = msg?;

                    let mut res = Response::new();
                    res.merge_from_bytes(msg.clone().into_data().iter().as_slice()).ok();
                    
                    // Handle JoinedResponse: request GameInfo manually
                    if !game_info_requested {
                        if let Some(Response_oneof_response::join_game(_)) = res.response.as_ref() {
                            println!("[{}] Received JoinedResponse, sending GameInfoRequest", name_clone);
                            let mut game_info_req = Request::new();
                            game_info_req.mut_game_info();
                            if let Ok(bytes) = game_info_req.write_to_bytes() {
                                let mut upstream = upstream_write.lock().await;
                                let _ = upstream.send(tungstenite::Message::Binary(Bytes::from(bytes))).await;
                            }
                            game_info_requested = true;
                            waiting_for_game_info = true;
                        }
                    }

                    // Filter out auto-requested GameInfoResponse (don't forward to client)
                    let should_forward_to_client = if waiting_for_game_info {
                        if let Some(Response_oneof_response::game_info(_)) = res.response.as_ref() {
                            println!("[{}] Received auto-requested GameInfoResponse, not forwarding to client", name_clone);
                            waiting_for_game_info = false;
                            false
                        } else {
                            true
                        }
                    } else {
                        true
                    };

                    // Buffer observations
                    if let Some(Response_oneof_response::observation(resp_obs)) = res.response.as_ref() {
                        if let Some(_obs_data) = resp_obs.observation.as_ref() {
                            *last_obs.lock().await = Some(resp_obs.clone());
                        }
                    }

                    // Call the callback with the response
                    (on_response)(res.clone());

                    // Only forward if not filtered
                    if should_forward_to_client {
                        client_write.send(msg).await?;
                    }
                }
                Ok::<_, tungstenite::Error>(())
            }
        };

        // Wait for either direction to finish and log if it's an error
        match future::select(Box::pin(c2s), Box::pin(s2c)).await {
            future::Either::Left((res, _)) => {
                if let Err(e) = res {
                    eprintln!("[{}] client → server forwarding ended with error: {e}", name);
                } else {
                    println!("[{}] client → server closed normally", name);
                }
            }
            future::Either::Right((res, _)) => {
                if let Err(e) = res {
                    eprintln!("[{}] server → client forwarding ended with error: {e}", name);
                } else {
                    println!("[{}] server → client closed normally", name);
                }
            }
        }
        println!("[{}] Proxy finished.", name);
        Ok(())
    }
}
