use bevy::prelude::Resource;
use futures_util::{future, StreamExt, SinkExt};
use protobuf::RepeatedField;
use sc2_proto::common::Race;
use sc2_proto::sc2api::{LocalMap, PlayerSetup, PlayerType, Request, Response};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::{accept_async, connect_async, tungstenite::Result};
use tokio::sync::broadcast;

use tungstenite::Message::Binary;
use protobuf::{Message};
use tungstenite::http;

/// ProxyWS holds:
///  * listener address for incoming client
///  * URL of the upstream server we proxy to
///

#[derive(Resource)]
pub struct ProxyWSResource {
    pub rx: broadcast::Receiver<Response>,
}
pub struct ProxyWS {
    listen_addr: String,
    upstream_url: String,

    pub tx: broadcast::Sender<Response>
}

impl ProxyWS {
    pub fn new(listen_addr: impl Into<String>, upstream_url: impl Into<String>) -> Self {
        Self {
            listen_addr: listen_addr.into(),
            upstream_url: upstream_url.into(),
            tx: broadcast::channel(100).0,
        }
    }

    /// Run the proxy: wait for **one** client, then bridge traffic until closed.
    pub async fn run(&self) -> Result<()> {
        let tx = self.tx.clone();
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

        // println!("Creating the game...");
        // upstream_write.send(Binary(bytes::Bytes::from(make_create_game_request().write_to_bytes().unwrap()))).await?;
        // let msg = upstream_read.next().await.unwrap()?;
        //
        // let mut res = Response::new();
        // res.merge_from_bytes(msg.into_data().iter().as_slice());
        //
        // println!("Game created: {:?}", res);

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
                //println!("+++++++++++++++++++++++++++++++++++++++++++++++++++");
                let msg = msg?;
                //println!("c2s: Got message: {:?}", msg);

                let mut req = Request::new();
                req.merge_from_bytes(msg.clone().into_data().iter().as_slice()).unwrap();
                //println!("c2s: Got request: {:?}", req);

                upstream_write.send(msg).await?;
            }
            Ok::<_, tungstenite::Error>(())
        };

        let s2c = async {
            while let Some(msg) = upstream_read.next().await {
                // println!("---------------------------------------------------");
                let msg = msg?;
                // println!("s2c: Got message: {:?}", msg);

                let mut res = Response::new();
                res.merge_from_bytes(msg.clone().into_data().iter().as_slice());
                // println!("c2s: Got response: {:?}", res);
                tx.send(res).unwrap();

                client_write.send(msg).await?;
            }
            Ok::<_, tungstenite::Error>(())
        };

        //future::select(Box::pin(c2s), Box::pin(s2c)).await;
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

fn make_create_game_request() -> Request
{
    let mut req = Request::new();
    let req_create_game = req.mut_create_game();

    let map_path = "AbyssalReefAIE.SC2Map".to_string();
    let mut local_map = LocalMap::new();
    local_map.set_map_path(map_path);
    req_create_game.set_local_map(local_map);

    let mut comp_ai_setup = PlayerSetup::default();
    comp_ai_setup.set_race(Race::Protoss);
    comp_ai_setup.set_field_type(PlayerType::Computer);

    let mut bot_setup = PlayerSetup::default();
    bot_setup.set_field_type(PlayerType::Participant);

    let participants = Vec::from([comp_ai_setup, bot_setup]);
    req_create_game.set_player_setup(RepeatedField::<PlayerSetup>::from_vec(participants));

    req
}
