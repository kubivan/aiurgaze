use crate::ui::{GameConfigPanel, GameType};
use protobuf::RepeatedField;
use sc2_proto::common::Race;
use sc2_proto::sc2api::{Difficulty, InterfaceOptions, LocalMap, PlayerSetup, PlayerType, Request};

pub fn build_create_game_request(panel: &GameConfigPanel) -> Result<Request, String> {
    let (Some(map_name), Some(game_type)) = (panel.map_name.clone(), Some(panel.game_type.clone()))
    else {
        return Err("Please select a map and fill all required fields.".to_string());
    };
    let mut req = Request::new();
    let req_create_game = req.mut_create_game();
    let mut local_map = LocalMap::new();
    local_map.set_map_path(map_name);
    req_create_game.set_local_map(local_map);

    let mut participant_setup = PlayerSetup::default();
    participant_setup.set_field_type(PlayerType::Participant);
    participant_setup.set_race(Race::Protoss);

    let mut opponent_setup = PlayerSetup::default();
    match game_type {
        GameType::VsAI => {
            opponent_setup.set_field_type(PlayerType::Computer);
            opponent_setup.set_race(panel.ai_race.unwrap_or(Race::Protoss));
            opponent_setup.set_difficulty(match panel.ai_difficulty.as_deref() {
                Some("Easy") => Difficulty::Easy,
                Some("Medium") => Difficulty::Medium,
                Some("Hard") => Difficulty::Hard,
                Some("Cheat") => Difficulty::CheatInsane,
                _ => Difficulty::Medium,
            });
        }
        GameType::VsBot => {
            opponent_setup.set_field_type(PlayerType::Participant);
            opponent_setup.set_race(Race::Protoss);
        }
    }
    let participants = vec![participant_setup, opponent_setup];
    req_create_game.set_player_setup(RepeatedField::from_vec(participants));

    req_create_game.set_disable_fog(panel.disable_fog);
    req_create_game.set_realtime(panel.realtime);
    if let Some(seed) = panel.random_seed {
        req_create_game.set_random_seed(seed);
    }
    println!("!!! CreateGame request: req={:?}", req);
    Ok(req)
}

pub fn build_start_replay_request(replay_data: Vec<u8>) -> Request {
    let mut request = Request::new();
    let start_replay = request.mut_start_replay();
    start_replay.set_replay_data(replay_data);
    start_replay.set_observed_player_id(1);
    start_replay.set_disable_fog(true);
    start_replay.set_realtime(false);
    start_replay.set_record_replay(true);

    let mut options = InterfaceOptions::new();
    options.set_raw(true);
    options.set_score(true);
    start_replay.set_options(options);
    request
}

#[cfg(test)]
mod tests {
    use super::*;
    use sc2_proto::sc2api::Request_oneof_request;

    #[test]
    fn start_replay_request_embeds_replay_and_enables_raw_observations() {
        let replay_data = vec![1, 2, 3];
        let request = build_start_replay_request(replay_data.clone());
        let Some(Request_oneof_request::start_replay(start_replay)) = request.request else {
            panic!("expected StartReplay request");
        };

        assert_eq!(start_replay.get_replay_data(), &replay_data);
        assert_eq!(start_replay.get_observed_player_id(), 1);
        assert!(start_replay.get_disable_fog());
        assert!(start_replay.get_record_replay());
        assert!(start_replay.get_options().get_raw());
        assert!(start_replay.get_options().get_score());
    }
}
