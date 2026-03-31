use crate::app_settings::AppSettings;
use crate::bot_runner::StartBotProcessesEvent;
use crate::controller::{PlayerResources, ProtocolActivityState};
use crate::observation_pipeline::VisionMode;
use crate::render_layers::{LayerRegistry, RenderLayerKind};
use crate::units::{
    get_set_fields, CurrentOrderAbility, SelectedUnit, UnitBuildProgress,
    UnitCompositionVisibility, UnitProto, UnitRegistry, UnitTag, UnitType,
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
    mut motion_evr: EventReader<MouseMotion>,
    mut scroll_evr: EventReader<MouseWheel>,
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
    mut layer_registry: ResMut<LayerRegistry>,
    mut unit_visibility: ResMut<UnitCompositionVisibility>,
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
                    ui.heading("Map Layers");

                    for layer in [
                        RenderLayerKind::Pathing,
                        RenderLayerKind::Placement,
                        RenderLayerKind::HeightMap,
                        RenderLayerKind::Creep,
                        RenderLayerKind::Energy,
                        RenderLayerKind::DebugOverlay,
                        RenderLayerKind::Minimap,
                    ] {
                        let mut visible = layer_registry.is_visible(layer);
                        if ui.checkbox(&mut visible, layer.label()).changed() {
                            layer_registry.set_visible(layer, visible);
                        }
                    }

                    ui.add_space(8.0);
                    ui.separator();
                    ui.heading("Unit Composition");

                    ui.checkbox(&mut unit_visibility.show_orders, "Order Indicators");

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

pub fn status_bar_system(
    mut contexts: EguiContexts,
    docker_status: Res<DockerStatus>,
    activity: Res<ProtocolActivityState>,
) {
    let ctx = match contexts.ctx_mut() {
        Ok(ctx) => ctx,
        Err(_) => return,
    };

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

            ui.separator();
            ui.label(format!("P1: {}", activity.player1_last));
            ui.separator();
            ui.label(format!("P2: {}", activity.player2_last));
        });
    });
}

/// HUD overlay: resource counts, supply, build queue — shown in top-left corner during GameScreen.
pub fn hud_system(
    mut contexts: EguiContexts,
    app_state: Res<AppState>,
    player_res: Res<PlayerResources>,
    in_progress_query: Query<(&UnitType, &UnitBuildProgress)>,
) {
    if *app_state != AppState::GameScreen {
        return;
    }
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };

    let mineral_color = egui::Color32::from_rgb(90, 190, 255);
    let vespene_color = egui::Color32::from_rgb(80, 210, 130);
    let ratio = if player_res.food_cap > 0 {
        player_res.food_used as f32 / player_res.food_cap as f32
    } else {
        0.0
    };
    let supply_color = if player_res.food_cap > 0 && player_res.food_used >= player_res.food_cap {
        egui::Color32::from_rgb(255, 80, 70)
    } else if ratio >= 0.8 {
        egui::Color32::from_rgb(255, 200, 50)
    } else {
        egui::Color32::from_rgb(215, 220, 230)
    };
    let dim_color = egui::Color32::from_rgb(150, 160, 175);

    let hud_frame = egui::Frame {
        fill: egui::Color32::from_rgba_premultiplied(8, 14, 26, 215),
        inner_margin: egui::Margin::same(12),
        corner_radius: egui::CornerRadius::same(7),
        stroke: egui::Stroke::new(1.0, egui::Color32::from_rgba_premultiplied(55, 110, 175, 110)),
        ..Default::default()
    };

    egui::Window::new("hud_panel")
        .title_bar(false)
        .movable(false)
        .resizable(false)
        .anchor(egui::Align2::LEFT_TOP, egui::vec2(12.0, 12.0))
        .frame(hud_frame)
        .show(ctx, |ui| {
            ui.spacing_mut().item_spacing = egui::vec2(10.0, 5.0);

            // --- Resources row ---
            ui.horizontal(|ui| {
                // Minerals
                ui.label(egui::RichText::new("◈").color(mineral_color).size(15.0));
                ui.label(
                    egui::RichText::new(format!("{:>6}", player_res.minerals))
                        .color(mineral_color)
                        .strong()
                        .size(15.0),
                );
                ui.add_space(4.0);

                // Vespene
                ui.label(egui::RichText::new("⬡").color(vespene_color).size(15.0));
                ui.label(
                    egui::RichText::new(format!("{:>6}", player_res.vespene))
                        .color(vespene_color)
                        .strong()
                        .size(15.0),
                );
                ui.add_space(4.0);

                // Supply
                ui.label(egui::RichText::new("⬆").color(supply_color).size(15.0));
                ui.label(
                    egui::RichText::new(format!("{}/{}", player_res.food_used, player_res.food_cap))
                        .color(supply_color)
                        .strong()
                        .size(15.0),
                );
                ui.add_space(8.0);

                // Army + Workers (smaller, dimmer)
                ui.label(
                    egui::RichText::new(format!("⚔ {}", player_res.army_count))
                        .color(dim_color)
                        .size(13.0),
                );
                ui.label(
                    egui::RichText::new(format!("⚒ {}", player_res.worker_count))
                        .color(dim_color)
                        .size(13.0),
                );
                if player_res.idle_workers > 0 {
                    ui.label(
                        egui::RichText::new(format!("idle:{}", player_res.idle_workers))
                            .color(egui::Color32::from_rgb(255, 165, 50))
                            .size(12.0),
                    );
                }
            });

            // --- Build queue ---
            let in_progress: Vec<_> = in_progress_query
                .iter()
                .filter(|(_, bp)| bp.0 < 1.0)
                .collect();

            if !in_progress.is_empty() {
                ui.separator();
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(6.0, 4.0);
                    for (unit_type, build_progress) in &in_progress {
                        ui.vertical(|ui| {
                            ui.set_max_width(50.0);
                            ui.label(
                                egui::RichText::new(format!("#{}", unit_type.0))
                                    .color(egui::Color32::from_rgb(180, 195, 215))
                                    .size(11.0),
                            );
                            let pg = build_progress.0.clamp(0.0, 1.0);
                            let (rect, _) = ui.allocate_exact_size(
                                egui::vec2(50.0, 5.0),
                                egui::Sense::hover(),
                            );
                            let painter = ui.painter();
                            painter.rect_filled(
                                rect,
                                2.0,
                                egui::Color32::from_rgba_premultiplied(35, 45, 60, 200),
                            );
                            let filled = egui::Rect::from_min_size(
                                rect.min,
                                egui::vec2(rect.width() * pg, rect.height()),
                            );
                            let r = (30.0 + 60.0 * (1.0 - pg)) as u8;
                            let g = (140.0 + 80.0 * pg) as u8;
                            painter.rect_filled(
                                filled,
                                2.0,
                                egui::Color32::from_rgb(r, g, 70),
                            );
                        });
                    }
                });
            }

            // --- Game loop (subtle footer) ---
            if player_res.game_loop > 0 {
                ui.label(
                    egui::RichText::new(format!("loop {}", player_res.game_loop))
                        .color(egui::Color32::from_rgba_premultiplied(100, 115, 140, 180))
                        .size(10.0),
                );
            }
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
