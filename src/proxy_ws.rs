use futures_util::{future, StreamExt, SinkExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::{accept_async, connect_async, tungstenite::Result};

/// ProxyWS holds:
///  * listener address for incoming client
///  * URL of the upstream server we proxy to
pub struct ProxyWS {
    listen_addr: String,
    upstream_url: String,
}

impl ProxyWS {
    pub fn new(listen_addr: impl Into<String>, upstream_url: impl Into<String>) -> Self {
        Self {
            listen_addr: listen_addr.into(),
            upstream_url: upstream_url.into(),
        }
    }

    /// Run the proxy: wait for **one** client, then bridge traffic until closed.
    pub async fn run(&self) -> Result<()> {
        // Bind TCP listener for the browser/client.
        let listener = TcpListener::bind(&self.listen_addr).await?;
        println!("Proxy listening on ws://{}", self.listen_addr);

        // Accept exactly one connection.
        let (client_stream, addr) = listener.accept().await?;
        println!("Client connected from {}", addr);

        // Upgrade client TCP -> WebSocket.
        let ws_client = accept_async(client_stream).await?;
        println!("Client handshake done.");

        // Connect upstream to the real server.
        println!("Connecting upstream to {}", self.upstream_url);
        let (ws_server, _) = connect_async(&self.upstream_url).await?;
        println!("Connected to upstream.");

        // Split into read/write halves.
        let (mut client_write, mut client_read) = ws_client.split();
        let (mut server_write, mut server_read) = ws_server.split();

        // Task: client -> server
        let c2s = async {
            while let Some(msg) = client_read.next().await {
                let msg = msg?;
                server_write.send(msg).await?;
            }
            Result::<()>::Ok(())
        };

        // Task: server -> client
        let s2c = async {
            while let Some(msg) = server_read.next().await {
                let msg = msg?;
                client_write.send(msg).await?;
            }
            Result::<()>::Ok(())
        };

        // Run until either side closes.
        future::select(Box::pin(c2s), Box::pin(s2c)).await;
        println!("ProxyWS: connection closed.");

        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // Example usage: proxy from local port 9001 to a public echo server
    let proxy = ProxyWS::new("127.0.0.1:9001", "wss://echo.websocket.events");
    proxy.run().await
}
