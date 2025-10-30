use std::fs;
use std::path::Path;
use bevy::prelude::*;
use bevy_egui::egui;
use sc2_proto::common::Race;
use crate::app_settings::GameConfigPanelDefaults;

#[derive(Resource, Default)]
pub struct GameConfigPanel {
    pub game_type: GameType,
    pub map_name: Option<String>,
    pub available_maps: Vec<String>,
    pub player_name: String,
    pub ai_difficulty: Option<String>,
    pub ai_race: Option<Race>,
    pub bot_name: Option<String>,
    pub disable_fog: bool,
    pub random_seed: Option<u32>,
    pub realtime: bool,
    pub bot_command: String,
    pub bot_opponent_command: String,
}

impl GameConfigPanel {
    pub fn new() -> Self {
        let available_maps = list_maps_folder();
        Self {
            available_maps,
            ai_race: Some(Race::Random),
            ..Default::default()
        }
    }

    pub fn from_defaults(defaults: &GameConfigPanelDefaults, available_maps: Vec<String>) -> Self {
        let game_type = match defaults.game_type.as_deref() {
            Some("VsBot") => GameType::VsBot,
            _ => GameType::VsAI,
        };
        let map_name = defaults.map_name.clone().or_else(|| available_maps.get(0).cloned());
        let player_name = defaults.player_name.clone().unwrap_or_else(|| "Player1".to_string());
        let ai_difficulty = defaults.ai_difficulty.clone();
        let ai_race = match defaults.ai_race.as_deref() {
            Some("Terran") => Some(Race::Terran),
            Some("Protoss") => Some(Race::Protoss),
            Some("Zerg") => Some(Race::Zerg),
            Some("Random") | _ => Some(Race::Random),
        };
        let bot_name = defaults.bot_name.clone();
        let disable_fog = defaults.disable_fog.unwrap_or(false);
        let random_seed = defaults.random_seed;
        let realtime = defaults.realtime.unwrap_or(false);
        let bot_command = defaults.bot_command.clone().unwrap_or_default();
        let bot_opponent_command = defaults.bot_opponent_command.clone().unwrap_or_default();
        Self {
            game_type,
            map_name,
            available_maps,
            player_name,
            ai_difficulty,
            ai_race,
            bot_name,
            disable_fog,
            random_seed,
            realtime,
            bot_command,
            bot_opponent_command,
        }
    }
}

pub fn list_maps_folder() -> Vec<String> {
    let path = Path::new("maps");
    if !path.exists() {
        return vec![];
    }
    match fs::read_dir(path) {
        Ok(entries) => entries
            .filter_map(|e| e.ok())
            .filter_map(|e| {
                let fname = e.file_name().into_string().ok()?;
                if fname.ends_with(".SC2Map") {
                    Some(fname)
                } else {
                    None
                }
            })
            .collect(),
        Err(_) => vec![],
    }
}

pub fn show_game_config_panel(ui: &mut egui::Ui, panel: &mut GameConfigPanel) -> bool {
    let mut start_game = false;
    ui.heading("Configure Game");
    ui.separator();

    ui.label("Game Type:");
    egui::ComboBox::from_id_source("game_type_combo")
        .selected_text(match panel.game_type {
            GameType::VsAI => "vs AI",
            GameType::VsBot => "vs Bot",
        })
        .show_ui(ui, |ui| {
            ui.selectable_value(&mut panel.game_type, GameType::VsAI, "vs AI");
            ui.selectable_value(&mut panel.game_type, GameType::VsBot, "vs Bot");
        });

    ui.label("Map Name:");
    if panel.available_maps.is_empty() {
        ui.label("No maps found in ./maps");
    } else {
        // Set default only if not already selected
        if panel.map_name.is_none() {
            panel.map_name = Some(panel.available_maps[0].clone());
        }
        egui::ComboBox::from_id_source("map_name_combo")
            .selected_text(panel.map_name.clone().unwrap_or_else(|| "Select map".to_string()))
            .show_ui(ui, |ui| {
                for map in &panel.available_maps {
                    ui.selectable_value(&mut panel.map_name, Some(map.clone()), map);
                }
            });
    }

    ui.label("Player Name:");
    ui.text_edit_singleline(&mut panel.player_name);

    match panel.game_type {
        GameType::VsAI => {
            ui.label("AI Difficulty:");
            egui::ComboBox::from_id_source("ai_difficulty_combo")
                .selected_text(panel.ai_difficulty.clone().unwrap_or_else(|| "Select difficulty".to_string()))
                .show_ui(ui, |ui| {
                    for diff in ["Easy", "Medium", "Hard", "Cheat"] {
                        ui.selectable_value(&mut panel.ai_difficulty, Some(diff.to_string()), diff);
                    }
                });

            ui.label("AI Race:");
            egui::ComboBox::from_id_source("ai_race_combo")
                .selected_text(panel.ai_race.map(|r| format!("{:?}", r)).unwrap_or_else(|| "Select race".to_string()))
                .show_ui(ui, |ui| {
                    for &race in &[Race::Random, Race::Terran, Race::Zerg, Race::Protoss] {
                        ui.selectable_value(&mut panel.ai_race, Some(race), format!("{:?}", race));
                    }
                });
        }
        GameType::VsBot => {
            ui.label("Bot Name:");
            ui.text_edit_singleline(panel.bot_name.get_or_insert_with(String::new));
            
            ui.label("Bot Opponent Command:");
            ui.text_edit_singleline(&mut panel.bot_opponent_command);
            ui.label("(Bash command to run opponent bot)");
        }
    }

    ui.add_space(10.0);
    ui.label("Bot Command:");
    ui.text_edit_singleline(&mut panel.bot_command);
    ui.label("(Optional: Bash command to run player bot)");
    
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        ui.checkbox(&mut panel.disable_fog, "Disable Fog");
        ui.checkbox(&mut panel.realtime, "Realtime");
        ui.label("Random Seed:");
        let mut seed_str = panel.random_seed.map(|v| v.to_string()).unwrap_or_default();
        if ui.text_edit_singleline(&mut seed_str).changed() {
            panel.random_seed = seed_str.parse().ok();
        }
    });
    ui.add_space(10.0);
    if ui.button("Create Game").clicked() {
        start_game = true;
    }
    start_game
}

#[derive(PartialEq, Eq, Clone, Debug, Default)]
pub enum GameType {
    #[default]
    VsAI,
    VsBot,
}
