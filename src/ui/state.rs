use crate::bot_runner::StartBotProcessesEvent;
use bevy::prelude::*;
use sc2_proto::sc2api::Request;

#[derive(Resource, PartialEq, Eq, Hash, Clone, Debug)]
pub enum AppState {
    StartScreen,
    GameScreen,
}

/// Resource to hold the pending CreateGame request.
#[derive(Resource, Default)]
pub struct PendingCreateGameRequest(pub Option<Request>);

#[derive(Resource, Default, Debug, PartialEq, Eq, Clone)]
pub struct GameCreated(pub bool);

/// Holds bot start info until proxies are ready to accept connections.
#[derive(Resource, Default)]
pub struct PendingBotStart(pub Option<StartBotProcessesEvent>);
