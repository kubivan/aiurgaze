use crate::app_settings::{get_maps_dir, GameConfigPanelDefaults};
use crate::ui::start_screen::beveled_button;
use bevy::prelude::*;
use bevy_egui::egui;
use sc2_proto::common::Race;
use std::fs;

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
    pub error_message: Option<String>,
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
        let map_name = match &defaults.map_name {
            Some(name) if available_maps.contains(name) => Some(name.clone()),
            _ => available_maps.first().cloned(),
        };
        let player_name = defaults
            .player_name
            .clone()
            .unwrap_or_else(|| "Player1".to_string());
        let ai_difficulty = defaults.ai_difficulty.clone();
        let ai_race = match defaults.ai_race.as_deref() {
            Some("Terran") => Some(Race::Terran),
            Some("Protoss") => Some(Race::Protoss),
            Some("Zerg") => Some(Race::Zerg),
            _ => Some(Race::Random),
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
            error_message: None,
        }
    }
}

pub fn list_maps_folder() -> Vec<String> {
    let path = get_maps_dir();
    if !path.exists() {
        return vec![];
    }
    match fs::read_dir(&path) {
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
    ui.label(
        egui::RichText::new("BATTLE PARAMETERS")
            .color(egui::Color32::from_rgb(105, 166, 213))
            .size(10.0)
            .strong(),
    );
    ui.add_space(8.0);
    ui.columns(2, |columns| {
        labeled_control(&mut columns[0], "MATCH TYPE", |ui| {
            egui::ComboBox::from_id_salt("game_type_combo")
                .selected_text(match panel.game_type {
                    GameType::VsAI => "vs AI",
                    GameType::VsBot => "vs Bot",
                })
                .width(ui.available_width())
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut panel.game_type, GameType::VsAI, "vs AI");
                    ui.selectable_value(&mut panel.game_type, GameType::VsBot, "vs Bot");
                });
        });
        labeled_control(&mut columns[1], "BATTLEFIELD", |ui| {
            if panel.available_maps.is_empty() {
                ui.label("No maps found in ./maps");
            } else {
                if panel.map_name.is_none() {
                    panel.map_name = panel.available_maps.first().cloned();
                }
                egui::ComboBox::from_id_salt("map_name_combo")
                    .selected_text(
                        panel
                            .map_name
                            .as_deref()
                            .unwrap_or("Select map"),
                    )
                    .width(ui.available_width())
                    .show_ui(ui, |ui| {
                        for map in &panel.available_maps {
                            ui.selectable_value(&mut panel.map_name, Some(map.clone()), map);
                        }
                    });
            }
        });
    });

    ui.add_space(8.0);
    labeled_control(ui, "COMMANDER", |ui| {
        ui.text_edit_singleline(&mut panel.player_name);
    });

    ui.add_space(8.0);
    match panel.game_type {
        GameType::VsAI => {
            ui.columns(2, |columns| {
                labeled_control(&mut columns[0], "AI DIFFICULTY", |ui| {
                    egui::ComboBox::from_id_salt("ai_difficulty_combo")
                        .selected_text(
                            panel
                                .ai_difficulty
                                .as_deref()
                                .unwrap_or("Select difficulty"),
                        )
                        .width(ui.available_width())
                        .show_ui(ui, |ui| {
                            for difficulty in ["Easy", "Medium", "Hard", "Cheat"] {
                                ui.selectable_value(
                                    &mut panel.ai_difficulty,
                                    Some(difficulty.to_string()),
                                    difficulty,
                                );
                            }
                        });
                });
                labeled_control(&mut columns[1], "AI RACE", |ui| {
                    egui::ComboBox::from_id_salt("ai_race_combo")
                        .selected_text(
                            panel
                                .ai_race
                                .map(|race| format!("{race:?}"))
                                .unwrap_or_else(|| "Select race".to_string()),
                        )
                        .width(ui.available_width())
                        .show_ui(ui, |ui| {
                            for &race in &[Race::Random, Race::Terran, Race::Zerg, Race::Protoss] {
                                ui.selectable_value(
                                    &mut panel.ai_race,
                                    Some(race),
                                    format!("{race:?}"),
                                );
                            }
                        });
                });
            });
        }
        GameType::VsBot => {
            labeled_control(ui, "OPPONENT BOT NAME", |ui| {
                ui.text_edit_singleline(panel.bot_name.get_or_insert_with(String::new));
            });
            ui.add_space(8.0);
            labeled_control(ui, "OPPONENT LAUNCH COMMAND", |ui| {
                ui.text_edit_singleline(&mut panel.bot_opponent_command);
            });
        }
    }

    ui.add_space(8.0);
    labeled_control(ui, "PLAYER BOT LAUNCH COMMAND  ·  OPTIONAL", |ui| {
        ui.text_edit_singleline(&mut panel.bot_command);
    });
    ui.add_space(8.0);
    ui.label(
        egui::RichText::new("SESSION OPTIONS")
            .color(egui::Color32::from_rgb(105, 166, 213))
            .size(10.0)
            .strong(),
    );
    ui.horizontal_wrapped(|ui| {
        ui.checkbox(&mut panel.disable_fog, "Disable fog");
        ui.checkbox(&mut panel.realtime, "Realtime");
        ui.label("Seed");
        let mut seed = panel.random_seed.map(|value| value.to_string()).unwrap_or_default();
        if ui
            .add_sized(egui::vec2(90.0, 24.0), egui::TextEdit::singleline(&mut seed))
            .changed()
        {
            panel.random_seed = seed.parse().ok();
        }
    });

    if let Some(error) = &panel.error_message {
        ui.colored_label(
            egui::Color32::from_rgb(255, 104, 95),
            format!("INITIALIZATION FAILED  /  {error}"),
        );
        ui.add_space(5.0);
    }

    let can_create_game = !panel.available_maps.is_empty() && panel.map_name.is_some();
    if !can_create_game {
        ui.colored_label(
            egui::Color32::from_rgb(255, 194, 86),
            "No maps available in ./maps",
        );
        ui.add_space(5.0);
    }
    let create_clicked = beveled_button(
        ui,
        "INITIALIZE SIMULATION  >",
        egui::vec2(ui.available_width(), 44.0),
        can_create_game,
        true,
    )
    .clicked();
    if create_clicked {
        panel.error_message = None;
    }
    create_clicked
}

fn labeled_control(ui: &mut egui::Ui, label: &str, add_control: impl FnOnce(&mut egui::Ui)) {
    ui.vertical(|ui| {
        ui.label(
            egui::RichText::new(label)
                .color(egui::Color32::from_rgb(142, 171, 190))
                .size(9.0)
                .strong(),
        );
        add_control(ui);
    });
}

#[derive(PartialEq, Eq, Clone, Debug, Default)]
pub enum GameType {
    #[default]
    VsAI,
    VsBot,
}
