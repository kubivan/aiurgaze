use bevy::input::mouse::{MouseMotion, MouseWheel};
use bevy::prelude::*;
use bevy_egui::{egui, EguiPlugin, EguiContexts, EguiPrimaryContextPass};
use crate::units::{SelectedUnit, UnitRegistry, UnitType, UnitHealth, UnitTag, UnitProto, get_set_fields};
use crate::net_helpers::send_create_game_request;
use sc2_proto::sc2api::{Request, LocalMap, PlayerSetup, PlayerType, Difficulty};
use sc2_proto::common::Race;
use protobuf::RepeatedField;
use std::fs;
use std::path::Path;
use futures_util::SinkExt;

mod game_config_panel;
mod setup_game_config_panel;

pub(crate) use game_config_panel::{GameConfigPanel, GameType, show_game_config_panel};
use setup_game_config_panel::setup_game_config_panel;
use crate::controller::setup_proxy;
use crate::proxy_ws;

#[derive(Resource, PartialEq, Eq, Hash, Clone, Debug)]
pub enum AppState {
    StartScreen,
    GameScreen,
}

#[derive(Component)]
pub struct MainCamera;

pub fn setup_camera(mut commands: Commands) {
    commands.spawn((
        Camera2d,
        Transform::from_xyz(0.0, 0.0, 1000.0),
    ));
}

pub fn camera_pan_system(
    mut query: Query<&mut Transform, With<MainCamera>>,
    mouse_input: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    mut mouse_motion_events: EventReader<MouseMotion>,
    time: Res<Time>,
) {
    let mut camera_transform = query.single_mut().unwrap();

    // === Keyboard pan (WASD) ===
    let mut direction = Vec2::ZERO;
    let speed = 500.0 * time.delta_secs();

    if keys.pressed(KeyCode::KeyW) {
        direction.y += 1.0;
    }
    if keys.pressed(KeyCode::KeyS) {
        direction.y -= 1.0;
    }
    if keys.pressed(KeyCode::KeyA) {
        direction.x -= 1.0;
    }
    if keys.pressed(KeyCode::KeyD) {
        direction.x += 1.0;
    }

    camera_transform.translation.x += direction.x * speed;
    camera_transform.translation.y += direction.y * speed;

    // === Mouse drag pan ===
    if mouse_input.pressed(MouseButton::Right) {
        for event in mouse_motion_events.read() {
            camera_transform.translation.x -= event.delta.x;
            camera_transform.translation.y += event.delta.y;
        }
    }
}

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
        // 🖱️ Middle mouse drag to pan
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
) {
    let ctx = match contexts.ctx_mut() {
        Ok(ctx) => ctx,
        _ => return,
    };

    let mut error_msg: Option<String> = None;
    const WS_URL: &str = "ws://127.0.0.1:5555/sc2api";

    match *app_state {
        AppState::StartScreen => {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.heading("SC2 Proxy");
                ui.separator();
                ui.label("Start Screen label");
                ui.add_space(10.0);
                if show_game_config_panel(ui, &mut game_config_panel) {
                    match build_create_game_request(&game_config_panel) {
                        Ok(req) => {
                            match send_create_game_request(req, WS_URL, 5, 1) {
                                Ok(()) => {
                                    game_created.0 = true;
                                    *app_state = AppState::GameScreen;
                                }
                                Err(e) => error_msg = Some(format!("Failed to send: {}", e)),
                            }
                        }
                        Err(msg) => error_msg = Some(msg),
                    }
                }
                if let Some(msg) = error_msg {
                    ui.label(msg);
                }
            });
        }
        AppState::GameScreen => {
            egui::TopBottomPanel::top("top_panel").show(ctx, |ui| {
                ui.label("Top Panel");
            });

            egui::SidePanel::left("map_panel")
                .resizable(true)
                .default_width(300.0)
                .show(ctx, |ui| {
                    ui.heading("Map Panel");
                    ui.separator();
                    egui::CollapsingHeader::new("RTS map rendering")
                        .default_open(true)
                        .show(ui, |ui| {
                            ui.label("(RTS map rendering goes here)");
                        });
                });
        }
    };
}

pub fn selected_unit_panel_system(
    mut contexts: EguiContexts,
    selected: Res<SelectedUnit>,
    registry: Res<UnitRegistry>,
    unit_query: Query<(&UnitProto, &UnitTag)>,
) {
    let ctx = match contexts.ctx_mut() {
        Ok(ctx) => ctx,
        Err(_) => return,
    };
    egui::SidePanel::right("unit_info_panel")
        .resizable(true)
        .default_width(300.0)
        .show(ctx, |ui| {
            ui.heading("Selected Unit Info");
            ui.separator();
            let tag = match selected.tag {
                Some(tag) => tag,
                None => {
                    ui.label("No unit selected.");
                    return;
                }
            };
            let entity = match registry.map.get(&tag) {
                Some(&entity) => entity,
                None => {
                    ui.label("No unit selected.");
                    return;
                }
            };
            let (unit_proto, unit_tag) = match unit_query.get(entity) {
                Ok(data) => data,
                Err(_) => {
                    ui.label("Unit data not found.");
                    return;
                }
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
