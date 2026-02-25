#![allow(dead_code)]
use crate::app_settings::AppSettings;
use crate::bot_runner::StartBotProcessesEvent;
use crate::observation_pipeline::VisionMode;
use crate::units::{
    get_set_fields, CurrentOrderAbility, SelectedUnit, UnitProto, UnitRegistry, UnitTag, UnitType,
};
use bevy::input::mouse::{MouseMotion, MouseWheel};
use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};
use protobuf::RepeatedField;
use sc2_proto::common::Race;
use sc2_proto::sc2api::{Difficulty, LocalMap, PlayerSetup, PlayerType, Request};
use tokio::sync::watch;

pub(crate) mod game_config_panel;
mod setup_game_config_panel; // kept for now if referenced elsewhere
pub(crate) use game_config_panel::{show_game_config_panel, GameConfigPanel, GameType};

#[derive(Resource, PartialEq, Eq, Hash, Clone, Debug)]
pub enum AppState {
    StartScreen,
    GameScreen,
}

/// Resource to hold a pending CreateGame request from CLI
#[derive(Resource, Default)]
pub struct PendingCreateGameRequest(pub Option<Request>);

/// Resource to hold the vision mode watch channel sender.
/// UI updates this to change which bot's perspective is rendered.
#[derive(Resource)]
pub struct VisionModeChannel {
    pub sender: watch::Sender<VisionMode>,
    pub current: VisionMode,
}

impl VisionModeChannel {
    pub fn new() -> (Self, watch::Receiver<VisionMode>) {
        let (sender, receiver) = watch::channel(VisionMode::default());
        (
            Self {
                sender,
                current: VisionMode::default(),
            },
            receiver,
        )
    }

    pub fn set(&mut self, mode: VisionMode) {
        self.current = mode;
        let _ = self.sender.send(mode);
    }
}

impl Default for VisionModeChannel {
    fn default() -> Self {
        let (sender, _) = watch::channel(VisionMode::default());
        Self {
            sender,
            current: VisionMode::default(),
        }
    }
}

pub fn setup_camera(mut commands: Commands) {
    commands.spawn((Camera2d, Transform::from_xyz(0.0, 0.0, 1000.0)));
}

#[derive(Resource, Default)]
pub struct CameraPanState {
    dragging: bool,
}

pub fn camera_controls(
    mut state: ResMut<CameraPanState>,
    buttons: Res<ButtonInput<MouseButton>>,
    mut motion_evr: MessageReader<MouseMotion>,
    mut scroll_evr: MessageReader<MouseWheel>,
    mut q_camera: Query<(&mut Transform, &mut Projection), With<Camera2d>>,
) {
    if let Ok((mut transform, mut projection)) = q_camera.single_mut() {
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

#[allow(clippy::too_many_arguments)]
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
    mut pending_bot_start: ResMut<PendingBotStart>,
    mut vision_mode_channel: ResMut<VisionModeChannel>,
) {
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };

    // Check if there's a pending request from CLI — store it for proxy to send
    if pending_request.0.is_some() && !game_created.0 {
        println!("[ui_system] Pending create game request from CLI, marking ready for proxy");
        game_created.0 = true;
        *app_state = AppState::GameScreen;

        // Store bot start info — will be emitted once proxies are ready
        let player_bot = if !game_config_panel.bot_command.is_empty() {
            Some(game_config_panel.bot_command.clone())
        } else {
            None
        };
        let opponent_bot = if !game_config_panel.bot_opponent_command.is_empty()
            && game_config_panel.game_type == GameType::VsBot
        {
            Some(game_config_panel.bot_opponent_command.clone())
        } else {
            None
        };

        if player_bot.is_some() || opponent_bot.is_some() {
            pending_bot_start.0 = Some(StartBotProcessesEvent {
                player_bot_command: player_bot,
                opponent_bot_command: opponent_bot,
                player_name: Some(game_config_panel.player_name.clone()),
                opponent_name: game_config_panel.bot_name.clone(),
                listen_port: app_settings.starcraft.listen_port,
            });
        }
        return;
    }

    match *app_state {
        AppState::StartScreen => {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.heading("SC2 Proxy");
                ui.separator();
                if show_game_config_panel(ui, &mut game_config_panel) {
                    match build_create_game_request(&game_config_panel) {
                        Ok(req) => {
                            println!("!!! CreateGame request: req={:?}", req);
                            // Store request — proxy will send it on its upstream connection
                            pending_request.0 = Some(req);
                            game_created.0 = true;
                            *app_state = AppState::GameScreen;

                            // Send event to start bot processes
                            let player_bot = if !game_config_panel.bot_command.is_empty() {
                                Some(game_config_panel.bot_command.clone())
                            } else {
                                None
                            };
                            let opponent_bot = if !game_config_panel.bot_opponent_command.is_empty()
                                && game_config_panel.game_type == GameType::VsBot
                            {
                                Some(game_config_panel.bot_opponent_command.clone())
                            } else {
                                None
                            };

                            if player_bot.is_some() || opponent_bot.is_some() {
                                pending_bot_start.0 = Some(StartBotProcessesEvent {
                                    player_bot_command: player_bot,
                                    opponent_bot_command: opponent_bot,
                                    player_name: Some(game_config_panel.player_name.clone()),
                                    opponent_name: game_config_panel.bot_name.clone(),
                                    listen_port: app_settings.starcraft.listen_port,
                                });
                            }
                        }
                        Err(e) => {
                            eprintln!("Create game failed: {}", e);
                            game_config_panel.error_message = Some(e);
                        }
                    }
                }
            });
        }
        AppState::GameScreen => {
            egui::SidePanel::right("unit_info_panel")
                .resizable(true)
                .default_width(300.0)
                .show(ctx, |ui| {
                    ui.heading("Game Controls");
                    ui.separator();

                    // Vision mode selector
                    ui.label("Vision Mode:");
                    let current_mode = vision_mode_channel.current;
                    egui::ComboBox::from_id_salt("vision_mode_combo")
                        .selected_text(format!("{}", current_mode))
                        .show_ui(ui, |ui| {
                            for mode in [VisionMode::Player1, VisionMode::Player2, VisionMode::All]
                            {
                                if ui
                                    .selectable_label(current_mode == mode, format!("{}", mode))
                                    .clicked()
                                {
                                    vision_mode_channel.set(mode);
                                }
                            }
                        });

                    ui.add_space(10.0);
                    ui.separator();
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

            // Central panel for game rendering - transparent to allow Bevy rendering
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE)
                .show(ctx, |_ui| {
                    // Empty panel - game world renders via Bevy camera behind egui
                });
        }
    }
}

#[derive(Resource, Clone, Debug, PartialEq, Eq)]
pub enum DockerStatus {
    Idle,
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
                DockerStatus::Idle => ui.colored_label(egui::Color32::GRAY, "Idle"),
                DockerStatus::Running => ui.colored_label(egui::Color32::GREEN, "Running"),
                DockerStatus::Starting => ui.colored_label(egui::Color32::YELLOW, "Starting"),
                DockerStatus::NotFound => ui.colored_label(egui::Color32::RED, "Not Found"),
                DockerStatus::Error(e) => {
                    ui.colored_label(egui::Color32::RED, format!("Error: {}", e))
                }
            };
        });
    });
}

#[derive(Resource, Default, Debug, PartialEq, Eq, Clone)]
pub struct GameCreated(pub bool);

/// Holds bot start info until proxies are ready to accept connections.
#[derive(Resource, Default)]
pub struct PendingBotStart(pub Option<StartBotProcessesEvent>);

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
    //participant_setup.set_race(panel.ai_race.unwrap_or(Race::Protoss));
    //participant_setup.set_player_name(panel.player_name.clone());

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
            //opponent_setup.set_player_name(panel.bot_name.clone().unwrap_or_default());
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
    println!("!!! CreateGame request: req={:?}", req,);
    Ok(req)
}
