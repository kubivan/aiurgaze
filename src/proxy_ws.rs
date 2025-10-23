use futures_util::{future, StreamExt, SinkExt};
use sc2_proto::sc2api::{Request, Response};
use tokio::net::{TcpListener, TcpStream};
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