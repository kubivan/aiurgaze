// src/proxy.rs
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use sc2_proto::sc2api::{LocalMap, PlayerSetup, PlayerType, Request, Response};

use crate::connection::*;
use tokio::time::{sleep, Duration};

use std::{
    error::Error,
    fmt,
    fs::File,
    io::Write,
    net::{TcpListener, TcpStream},
    ops::{Deref, DerefMut},
    process::{Child, Command},
};
use tungstenite::{connect, stream::MaybeTlsStream, WebSocket};

use protobuf::{Message, RepeatedField};
use tungstenite::Message::Binary;

use futures::{SinkExt, StreamExt};

pub(crate) type WS = WebSocket<MaybeTlsStream<TcpStream>>;
pub type SC2Result<T> = Result<T, Box<dyn Error>>;


// pub mod sc2api_protocol {
//     include!(concat!(env!("OUT_DIR"), "/sc2api_protocol.rs"));
// }

use sc2_proto::*;
use sc2_proto::common::Race;
use tokio_tungstenite::connect_async;
//use sc2_proto::sc2api;

#[derive(Debug, Clone)]
pub enum ProxyCommand {
    Connect { addr: String },
    CreateGame { map: String, players: Vec<PlayerConfig> },
    Step { frames: u32 },
    LeaveGame,
}

#[derive(Debug, Clone)]
pub struct PlayerConfig {
    pub name: Option<String>,
    // Extend with race, type, etc.
}

#[derive(Debug, Clone)]
pub enum ProxyEvent {
    Connected,
    Disconnected { reason: String },
    GameCreated { map: String },
    GameStateDelta(GameDelta),
    Error(String),
}

// Minimal domain delta to demonstrate the flow.
// Expand to include units, scores, etc.
#[derive(Debug, Clone)]
pub enum GameDelta {
    Tick,
}


// After we are connected, this loop listens for client requests and forwards to the server.
// Replace TODOs with actual tonic client calls and translate responses into ProxyEvent.
async fn game_loop(
    cmd_rx: &mut UnboundedReceiver<ProxyCommand>,
    evt_tx: &UnboundedSender<ProxyEvent>,
    sc2_con: &mut Connection
) {


    // //let mut sc2_server =  Sc2ProxyServer::new(channel.clone());
    // let mut client = Sc2ProxyClient::new(channel);

    while let Some(cmd) = cmd_rx.recv().await {
        match cmd {
            ProxyCommand::CreateGame { map, players: _ } => {
                // TODO: construct SC2APIProtocol::Request join/create payload and call client.SendJoinRequest(...)
                // let response = client.send_join_request(request).await;
                // let create_game_request = RequestCreateGame {
                //     player_setup: vec![],
                //     disable_fog: Some(true),
                //     random_seed: None,
                //     realtime: Some(false),
                //     map: Some(sc2api_protocol::request_create_game::Map::BattlenetMapName(
                //         "2000AtmospheresAIE".to_string(),
                //     )),
                // };

                //debug!("Sending CreateGame request");
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

                let res = sc2_con.send(req).await;

                println!("GOT RESPONSE: {:?}", res);
            }
            ProxyCommand::Step { frames: _ } => {
                // TODO: construct step request and call client.SendStepRequest(...)
                // let response = client.send_step_request(request).await;
                // Translate response to GameStateDelta and emit:
                let _ = evt_tx.send(ProxyEvent::GameStateDelta(GameDelta::Tick));
            }
            ProxyCommand::LeaveGame => {
                // TODO: optionally call a leave/quit RPC if applicable
                let _ = evt_tx.send(ProxyEvent::Disconnected { reason: "Left game".into() });
                break; // leave game loop, return to waiting-for-connect state
            }
            ProxyCommand::Connect { .. } => {
                // Already connected; you may choose to reconnect here instead.
                let _ = evt_tx.send(ProxyEvent::Error("Already connected".into()));
            }
        }
    }
}




/// Spawned on a tokio runtime. Reads commands and emits events.
/// Replace stubs with tonic gRPC calls.
pub async fn start_proxy(
    mut cmd_rx: UnboundedReceiver<ProxyCommand>,
    evt_tx: UnboundedSender<ProxyEvent>,
    port_client: u16,
    port_server: u16,
) {
    // State machine:
    // - Wait for Connect
    // - On success, enter game_loop (handles requests/responses)
    // - On LeaveGame or error, go back to waiting for Connect

    loop {
        // Wait for a Connect command
        let cmd = match cmd_rx.recv().await {
            Some(c) => c,
            None => break, // sender dropped
        };

        match cmd {
            ProxyCommand::Connect { addr } => {
                // 1. Connect to the SC2 WebSocket
                let url = "ws://127.0.0.1:5000/sc2api";
                let mut sc2_con = Connection::connect(url).await;

                game_loop(&mut cmd_rx, &evt_tx, &mut sc2_con).await;

            }
            // Any other command before connecting -> error
            _ => {
                let _ = evt_tx.send(ProxyEvent::Error("Not connected".into()));
            }
        }
    }
}
