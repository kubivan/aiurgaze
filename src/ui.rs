use bevy::input::mouse::{MouseMotion, MouseWheel};
use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};
use crate::units::{SelectedUnit, UnitRegistry, UnitTag, UnitProto, get_set_fields, CurrentOrderAbility, UnitType};
use crate::net_helpers::send_create_game_request;
use sc2_proto::sc2api::{Request, LocalMap, PlayerSetup, PlayerType, Difficulty};
use sc2_proto::common::Race;
use protobuf::RepeatedField;
use crate::app_settings::AppSettings;
use crate::bot_runner::StartBotProcessesEvent;

pub(crate) mod game_config_panel;
mod setup_game_config_panel; // kept for now if referenced elsewhere
pub(crate) use game_config_panel::{GameConfigPanel, GameType, show_game_config_panel};

#[derive(Resource, PartialEq, Eq, Hash, Clone, Debug)]
pub enum AppState { StartScreen, GameScreen }

/// Resource to hold a pending CreateGame request from CLI
#[derive(Resource, Default)]
pub struct PendingCreateGameRequest(pub Option<Request>);

#[derive(Component)]
pub struct MainCamera;

pub fn setup_camera(mut commands: Commands) { commands.spawn((Camera2d, Transform::from_xyz(0.0, 0.0, 1000.0))); }

#[derive(Resource, Default)]
pub struct CameraPanState {
    dragging: bool,
}

pub fn camera_controls(
    mut state: ResMut<CameraPanState>,
    buttons: Res<ButtonInput<MouseButton>>,
    mut motion_evr: EventReader<MouseMotion>,
    mut scroll_evr: EventReader<MouseWheel>,
    mut q_camera: Query<(&mut Transform, &mut Projection), With<Camera2d>>,
) {
    if let Ok((mut transform, mut projection)) = q_camera.get_single_mut() {
        // Middle mouse drag to pan
        if buttons.just_pressed(MouseButton::Middle) {
            state.dragging = true;
        }
        if buttons.just_released(MouseButton::Middle) {
            state.dragging = false;
        }

        if state.dragging {
            for ev in motion_evr.read() {
                transform.translation.x -= ev.delta.x;
                transform.translation.y += ev.delta.y;
            }
        }

        // 🔍 Scroll to zoom
        for ev in scroll_evr.read() {
            if let Projection::Orthographic(ref mut ortho) = *projection {
                ortho.scale = (ortho.scale * (1.0 - ev.y * 0.1)).clamp(0.1, 10.0);
            }
        }
    }
}

pub fn ui_system(
    mut contexts: EguiContexts,
    mut app_state: ResMut<AppState>,
    mut game_config_panel: ResMut<GameConfigPanel>,
    mut game_created: ResMut<GameCreated>,
    mut pending_request: ResMut<PendingCreateGameRequest>,
    selected: Res<SelectedUnit>,
    registry: Res<UnitRegistry>,
    unit_query: Query<(&UnitProto, &UnitTag, &CurrentOrderAbility, &UnitType)>,
    app_settings: Res<AppSettings>,
    mut bot_events: EventWriter<StartBotProcessesEvent>,
) {
    let Ok(ctx) = contexts.ctx_mut() else { return; };
    let ws_url = format!("{}:{}/sc2api", app_settings.starcraft.upstream_url, app_settings.starcraft.upstream_port);
    
    // Check if there's a pending request from CLI to send
    if let Some(req) = pending_request.0.take() {
        println!("[ui_system] Sending pending create game request from CLI");
        let res = send_create_game_request(req, &ws_url, 5, 1);
        match res {
            Err(e) => {
                eprintln!("[ui_system] Failed to send create game request: {}", e);
            },
            Ok(_) => {
                println!("[ui_system] Create game request sent successfully");
                game_created.0 = true;
                *app_state = AppState::GameScreen;

                // Send event to start bot processes
                let player_bot = if !game_config_panel.bot_command.is_empty() {
                    Some(game_config_panel.bot_command.clone())
                } else {
                    None
                };
                let opponent_bot = if !game_config_panel.bot_opponent_command.is_empty()
                    && game_config_panel.game_type == GameType::VsBot {
                    Some(game_config_panel.bot_opponent_command.clone())
                } else {
                    None
                };

                if player_bot.is_some() || opponent_bot.is_some() {
                    bot_events.send(StartBotProcessesEvent {
                        player_bot_command: player_bot,
                        opponent_bot_command: opponent_bot,
                    });
                }
            }
        }
        return;
    }
    
    match *app_state {
        AppState::StartScreen => {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.heading("SC2 Proxy");
                ui.separator();
                if show_game_config_panel(ui, &mut game_config_panel) {
                    let res = build_create_game_request(&game_config_panel)
                        .and_then(|req| {
                            println!("Sending create game request: {:?}", req);
                            send_create_game_request(req, &ws_url, 5, 1)
                        });
                    match res {
                        Err(e) => { ui.label(e); },
                        Ok(_) => {
                            game_created.0 = true;
                            *app_state = AppState::GameScreen;
                            ui.label("Create game request sent successfully.");

                            // Send event to start bot processes
                            let player_bot = if !game_config_panel.bot_command.is_empty() {
                                Some(game_config_panel.bot_command.clone())
                            } else {
                                None
                            };
                            let opponent_bot = if !game_config_panel.bot_opponent_command.is_empty()
                                && game_config_panel.game_type == GameType::VsBot {
                                Some(game_config_panel.bot_opponent_command.clone())
                            } else {
                                None
                            };

                            if player_bot.is_some() || opponent_bot.is_some() {
                                bot_events.send(StartBotProcessesEvent {
                                    player_bot_command: player_bot,
                                    opponent_bot_command: opponent_bot,
                                });
                            }
                        }
                    }
                }
            });
        }
        AppState::GameScreen =>
        {
            egui::SidePanel::right("unit_info_panel")
                .resizable(true)
                .default_width(300.0)
                .show(ctx, |ui| {
                    ui.heading("Selected Unit Info");
                    ui.separator();
                    
                    let Some(tag) = selected.tag else {
                        ui.label("No unit selected.");
                        return;
                    };
                    
                    let Some(&entity) = registry.map.get(&tag) else {
                        ui.label("No unit selected.");
                        return;
                    };
                    
                    let Ok((unit_proto, unit_tag, _, _)) = unit_query.get(entity) else {
                        ui.label("Unit data not found.");
                        return;
                    };
                    
                    egui::CollapsingHeader::new("Unit Details")
                        .default_open(true)
                        .show(ui, |ui| {
                            ui.label(format!("Tag: {}", unit_tag.0));
                            ui.separator();
                            for (field, value) in get_set_fields(&unit_proto.0) {
                                ui.label(format!("{}: {}", field, value));
                            }
                        });
                });
        }
    }
}

#[derive(Resource, Clone, Debug, PartialEq, Eq)]
pub enum DockerStatus {
    NotFound,
    Starting,
    Running,
    Error(String),
}

pub fn status_bar_system(mut contexts: EguiContexts, docker_status: Res<DockerStatus>) {
    let ctx = match contexts.ctx_mut() {
        Ok(ctx) => ctx,
        Err(_) => return,
    };
    
    // println!("[StatusBar] DockerStatus: {:?}", *docker_status);
    egui::TopBottomPanel::bottom("status_bar").show(ctx, |ui| {
        ui.horizontal(|ui| {
            ui.label("Docker status:");
            match &*docker_status {
                DockerStatus::Running => ui.colored_label(egui::Color32::GREEN, "Running"),
                DockerStatus::Starting => ui.colored_label(egui::Color32::YELLOW, "Starting"),
                DockerStatus::NotFound => ui.colored_label(egui::Color32::RED, "Not Found"),
                DockerStatus::Error(e) => ui.colored_label(egui::Color32::RED, format!("Error: {}", e)),
            };
        });
    });
}

#[derive(Resource, Default, Debug, PartialEq, Eq, Clone)]
pub struct GameCreated(pub bool);

pub fn build_create_game_request(panel: &GameConfigPanel) -> Result<Request, String> {
    let (Some(map_name), Some(game_type)) = (panel.map_name.clone(), Some(panel.game_type.clone())) else {
        return Err("Please select a map and fill all required fields.".to_string());
    };
    let mut req = Request::new();
    let req_create_game = req.mut_create_game();
    let mut local_map = LocalMap::new();
    local_map.set_map_path(map_name);
    req_create_game.set_local_map(local_map);

    let mut participant_setup = PlayerSetup::default();
    participant_setup.set_field_type(PlayerType::Participant);
    participant_setup.set_race(Race::Random);
    participant_setup.set_player_name(panel.player_name.clone());

    let mut opponent_setup = PlayerSetup::default();
    match game_type {
        GameType::VsAI => {
            opponent_setup.set_field_type(PlayerType::Computer);
            opponent_setup.set_race(panel.ai_race.unwrap_or(Race::Random));
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
            opponent_setup.set_race(Race::Random);
            opponent_setup.set_player_name(panel.bot_name.clone().unwrap_or_default());
        }
    }
    let participants = vec![participant_setup, opponent_setup];
    req_create_game.set_player_setup(RepeatedField::from_vec(participants));

    // Set game options from UI
    req_create_game.set_disable_fog(panel.disable_fog);
    req_create_game.set_realtime(panel.realtime);
    if let Some(seed) = panel.random_seed {
        req_create_game.set_random_seed(seed);
    }
    Ok(req)
}
