use serde::{Serialize, Deserialize};

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq)]
pub enum GameType {
    #[default]
    VsAI,
    VsBot,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CreateGameRequest {
    pub game_type: GameType,
    pub map_name: String,
    pub player_name: String,
    pub ai_difficulty: Option<String>,
    pub bot_name: Option<String>,
    // Add more fields as needed
}

