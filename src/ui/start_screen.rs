use crate::app_settings::AppSettings;
use crate::bot_runner::StartBotProcessesEvent;
use crate::controller::ReplayFileLoadedEvent;
use crate::ui::{
    build_create_game_request, build_start_replay_request, show_game_config_panel, AppState,
    GameConfigPanel, GameCreated, GameType, PendingBotStart, PendingCreateGameRequest,
};
use bevy::prelude::*;
use bevy_egui::egui;
use bevy_tokio_tasks::TokioTasksRuntime;
use tokio::task::spawn_blocking;

const BLUE: egui::Color32 = egui::Color32::from_rgb(105, 166, 213);
const ICE: egui::Color32 = egui::Color32::from_rgb(205, 224, 238);
const MUTED: egui::Color32 = egui::Color32::from_rgb(137, 159, 180);
const INK: egui::Color32 = egui::Color32::from_rgb(5, 10, 20);
const ACTION_BUTTON_HEIGHT: f32 = 44.0;

#[derive(Resource, Default)]
pub(crate) struct StartScreenPanel {
    page: StartScreenPage,
    error_message: Option<String>,
    loading_replay: bool,
}

#[derive(Default, Clone, Copy)]
enum StartScreenPage {
    #[default]
    Home,
    CreateMatch,
}

pub(crate) fn render_start_screen(
    ui: &mut egui::Ui,
    app_state: &mut ResMut<AppState>,
    game_config_panel: &mut ResMut<GameConfigPanel>,
    game_created: &mut ResMut<GameCreated>,
    pending_request: &mut ResMut<PendingCreateGameRequest>,
    app_settings: &Res<AppSettings>,
    pending_bot_start: &mut ResMut<PendingBotStart>,
    start_screen: &mut ResMut<StartScreenPanel>,
    runtime: &Res<TokioTasksRuntime>,
) {
    egui::CentralPanel::default()
        .frame(egui::Frame::NONE)
        .show(ui, |ui| {
            ui.scope(|ui| {
                apply_terminal_theme(ui);
                paint_backdrop(ui);

                let available = ui.available_size();
                let content_width = available.x.min(1180.0).max(0.0);
                let side_padding = ((available.x - content_width) / 2.0).max(0.0);
                ui.horizontal(|ui| {
                    ui.add_space(side_padding);
                    ui.allocate_ui_with_layout(
                        egui::vec2(content_width, available.y),
                        egui::Layout::top_down(egui::Align::Center),
                        |ui| {
                            ui.add_space(18.0);
                            render_masthead(ui);
                            ui.add_space(24.0);
                            render_start_screen_layout(
                                ui,
                                game_config_panel,
                                app_state,
                                game_created,
                                pending_request,
                                app_settings,
                                pending_bot_start,
                                start_screen,
                                runtime,
                            );
                        },
                    );
                });
            });
        });
}

pub(crate) fn replay_file_loaded_system(
    mut events: MessageReader<ReplayFileLoadedEvent>,
    mut start_screen: ResMut<StartScreenPanel>,
    mut app_state: ResMut<AppState>,
    mut game_created: ResMut<GameCreated>,
    mut pending_request: ResMut<PendingCreateGameRequest>,
    mut pending_bot_start: ResMut<PendingBotStart>,
) {
    for event in events.read() {
        start_screen.loading_replay = false;
        match &event.result {
            Some(Ok(replay_data)) => {
                pending_request.0 = Some(build_start_replay_request(replay_data.clone()));
                pending_bot_start.0 = None;
                game_created.0 = true;
                *app_state = AppState::GameScreen;
            }
            Some(Err(error)) => {
                start_screen.error_message = Some(format!("Could not read replay file: {error}"));
            }
            None => {}
        }
    }
}

fn apply_terminal_theme(ui: &mut egui::Ui) {
    let style = ui.style_mut();
    style.visuals = egui::Visuals::dark();
    style.visuals.panel_fill = INK;
    style.visuals.window_fill = egui::Color32::from_rgb(11, 17, 31);
    style.visuals.faint_bg_color = egui::Color32::from_rgb(18, 32, 54);
    style.visuals.extreme_bg_color = egui::Color32::from_rgb(4, 8, 16);
    style.visuals.text_edit_bg_color = Some(egui::Color32::from_rgb(5, 12, 25));
    style.visuals.selection.bg_fill = egui::Color32::from_rgb(30, 75, 120);
    style.visuals.selection.stroke = egui::Stroke::new(1.0, BLUE);
    style.visuals.widgets.noninteractive.fg_stroke.color =
        egui::Color32::from_rgb(205, 219, 231);
    style.visuals.widgets.inactive.bg_fill = egui::Color32::from_rgb(10, 20, 37);
    style.visuals.widgets.inactive.bg_stroke =
        egui::Stroke::new(1.0, egui::Color32::from_rgb(48, 75, 111));
    style.visuals.widgets.hovered.bg_fill = egui::Color32::from_rgb(22, 48, 78);
    style.visuals.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, BLUE);
    style.visuals.widgets.active.bg_fill = egui::Color32::from_rgb(27, 57, 89);
    style.visuals.widgets.active.bg_stroke = egui::Stroke::new(1.0, BLUE);
    style.spacing.item_spacing = egui::vec2(10.0, 8.0);
    style.spacing.button_padding = egui::vec2(10.0, 6.0);
}

fn paint_backdrop(ui: &egui::Ui) {
    let rect = ui.max_rect();
    let painter = ui.painter();
    painter.rect_filled(rect, 0.0, egui::Color32::from_rgb(5, 9, 20));

    let glow_center = egui::pos2(rect.left() + rect.width() * 0.23, rect.bottom() - 26.0);
    for layer in (1..=12).rev() {
        let radius = rect.width().min(rect.height()) * layer as f32 * 0.085;
        let alpha = (14 - layer).max(1) as u8 * 3;
        painter.circle_filled(
            glow_center,
            radius,
            egui::Color32::from_rgba_unmultiplied(20, 93, 180, alpha),
        );
    }
    let horizon = rect.bottom() - rect.height() * 0.22;
    painter.rect_filled(
        egui::Rect::from_min_max(
            egui::pos2(rect.left(), horizon),
            egui::pos2(rect.right(), horizon + rect.height() * 0.12),
        ),
        0.0,
        egui::Color32::from_rgba_unmultiplied(16, 73, 144, 25),
    );

    for index in 0..88 {
        let x = rect.left() + ((index * 73 % 997) as f32 / 997.0) * rect.width();
        let y = rect.top() + ((index * 193 % 991) as f32 / 991.0) * rect.height();
        let alpha = 30 + (index % 4) as u8 * 18;
        painter.circle_filled(
            egui::pos2(x, y),
            if index % 9 == 0 { 1.5 } else { 0.9 },
            egui::Color32::from_rgba_unmultiplied(187, 204, 231, alpha),
        );
    }

    painter.line_segment(
        [
            egui::pos2(rect.left(), rect.bottom() - 1.0),
            egui::pos2(rect.right(), rect.bottom() - 1.0),
        ],
        egui::Stroke::new(1.0, egui::Color32::from_rgb(37, 66, 105)),
    );
}

fn render_masthead(ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("◈").color(BLUE).size(24.0).strong());
        ui.vertical(|ui| {
            ui.label(
                egui::RichText::new("AIURGAZE")
                    .color(ICE)
                    .size(19.0)
                    .strong(),
            );
            ui.label(
                egui::RichText::new("TACTICAL OBSERVATION SYSTEM")
                    .color(MUTED)
                    .size(9.0)
                    .strong(),
            );
        });
        ui.add_space(30.0);
        ui.label(
            egui::RichText::new("LOCAL COMMAND // READY")
                .color(BLUE)
                .size(10.0)
                .strong(),
        );
    });
    ui.add_space(10.0);
    let (line, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), 1.0),
        egui::Sense::hover(),
    );
    ui.painter().line_segment(
        [line.left_top(), line.right_top()],
        egui::Stroke::new(1.0, BLUE),
    );
}

#[allow(clippy::too_many_arguments)]
fn render_start_screen_layout(
    ui: &mut egui::Ui,
    game_config_panel: &mut GameConfigPanel,
    app_state: &mut ResMut<AppState>,
    game_created: &mut ResMut<GameCreated>,
    pending_request: &mut ResMut<PendingCreateGameRequest>,
    app_settings: &Res<AppSettings>,
    pending_bot_start: &mut ResMut<PendingBotStart>,
    start_screen: &mut ResMut<StartScreenPanel>,
    runtime: &Res<TokioTasksRuntime>,
) {
    let available_width = ui.available_width();
    let left_width = (available_width * 0.36).clamp(250.0, 390.0);
    let right_width = (available_width - left_width - 24.0).max(0.0);

    ui.horizontal_top(|ui| {
        ui.allocate_ui_with_layout(
            egui::vec2(left_width, ui.available_height()),
            egui::Layout::top_down(egui::Align::Min),
            render_welcome_panel,
        );
        ui.add_space(24.0);
        ui.allocate_ui_with_layout(
            egui::vec2(right_width, ui.available_height()),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                let available_height = ui.available_height();
                egui::Frame::new()
                    .fill(egui::Color32::from_rgb(10, 16, 29))
                    .stroke(egui::Stroke::new(
                        1.0,
                        egui::Color32::from_rgb(47, 72, 111),
                    ))
                    .corner_radius(egui::CornerRadius::same(4))
                    .inner_margin(egui::Margin::same(20))
                    .show(ui, |ui| {
                        ui.set_min_width(right_width - 40.0);
                        match start_screen.page {
                            StartScreenPage::Home => render_home_panel(
                                ui,
                                start_screen,
                                runtime,
                            ),
                            StartScreenPage::CreateMatch => {
                                render_card_heading(ui, "01", "MISSION CONFIGURATION");
                                ui.label(
                                    egui::RichText::new(
                                        "Define the battlefield and initialize a session.",
                                    )
                                    .color(MUTED)
                                    .size(11.0),
                                );
                                ui.add_space(14.0);
                                ui.separator();

                                let scroll_height = (available_height - 174.0).max(140.0);
                                egui::ScrollArea::vertical()
                                    .max_height(scroll_height)
                                    .show(ui, |ui| {
                                        if show_game_config_panel(ui, game_config_panel) {
                                            match build_create_game_request(game_config_panel) {
                                                Ok(request) => {
                                                    println!(
                                                        "!!! CreateGame request: req={request:?}"
                                                    );
                                                    pending_request.0 = Some(request);
                                                    game_created.0 = true;
                                                    **app_state = AppState::GameScreen;
                                                    pending_bot_start.0 =
                                                        build_pending_bot_start(
                                                            game_config_panel,
                                                            app_settings
                                                                .starcraft
                                                                .listen_port,
                                                        );
                                                }
                                                Err(error) => {
                                                    eprintln!("Create game failed: {error}");
                                                    game_config_panel.error_message = Some(error);
                                                }
                                            }
                                        }
                                    });
                                ui.add_space(8.0);
                                if beveled_button(
                                    ui,
                                    "‹  BACK TO COMMAND DECK",
                                    egui::vec2(ui.available_width(), ACTION_BUTTON_HEIGHT),
                                    true,
                                    false,
                                )
                                .clicked()
                                {
                                    start_screen.page = StartScreenPage::Home;
                                }
                            }
                        }
                    });
            },
        );
    });
}

fn render_welcome_panel(ui: &mut egui::Ui) {
    ui.add_space(20.0);
    ui.label(
        egui::RichText::new("COMMAND DECK  /  01")
            .color(BLUE)
            .size(10.0)
            .strong(),
    );
    ui.add_space(18.0);
    ui.label(
        egui::RichText::new("THE\nKOPRULU\nSECTOR")
            .color(egui::Color32::from_rgb(230, 239, 246))
            .size(38.0)
            .strong(),
    );
    ui.add_space(14.0);
    ui.label(
        egui::RichText::new("Observe every engagement.")
            .color(egui::Color32::from_rgb(190, 211, 224))
            .size(16.0),
    );
    ui.label(
        egui::RichText::new("Launch a match, watch your bots, and inspect the battle in real time.")
            .color(MUTED)
            .size(12.0),
    );

    ui.add_space(30.0);
    render_feature_line(ui, "01", "LIVE BATTLEFIELD", "Stream game state as it unfolds.");
    ui.add_space(14.0);
    render_feature_line(ui, "02", "TACTICAL OVERLAYS", "Inspect terrain, vision, and units.");
    ui.add_space(14.0);
    render_feature_line(ui, "03", "BOT ARENA", "Run one or two local bot processes.");
}

fn render_home_panel(
    ui: &mut egui::Ui,
    start_screen: &mut StartScreenPanel,
    runtime: &Res<TokioTasksRuntime>,
) {
    render_card_heading(ui, "00", "COMMAND OPTIONS");
    ui.label(
        egui::RichText::new("Choose how you want to enter the battlefield.")
            .color(MUTED)
            .size(11.0),
    );
    ui.add_space(18.0);

    if beveled_button(
        ui,
        "CREATE MATCH",
        egui::vec2(ui.available_width(), ACTION_BUTTON_HEIGHT),
        true,
        true,
    )
    .clicked()
    {
        start_screen.page = StartScreenPage::CreateMatch;
    }
    ui.add_space(8.0);
    if beveled_button(
        ui,
        if start_screen.loading_replay {
            "OPENING REPLAY..."
        } else {
            "PLAY REPLAY"
        },
        egui::vec2(ui.available_width(), ACTION_BUTTON_HEIGHT),
        !start_screen.loading_replay,
        false,
    )
    .clicked()
    {
        start_screen.error_message = None;
        start_screen.loading_replay = true;
        runtime.spawn_background_task(move |mut ctx| async move {
            let file = rfd::AsyncFileDialog::new()
                .add_filter("StarCraft II replay", &["SC2Replay"])
                .pick_file()
                .await;
            let result = match file {
                Some(file) => {
                    let path = file.path().to_path_buf();
                    Some(
                        spawn_blocking(move || std::fs::read(path))
                            .await
                            .map_err(|error| error.to_string())
                            .and_then(|result| result.map_err(|error| error.to_string())),
                    )
                }
                None => None,
            };
            ctx.run_on_main_thread(move |world| {
                world.world.write_message(ReplayFileLoadedEvent { result });
            })
            .await;
        });
    }
    if let Some(error) = &start_screen.error_message {
        ui.colored_label(egui::Color32::from_rgb(255, 104, 95), error);
    }
    ui.add_space(8.0);
    if beveled_button(
        ui,
        "EXIT",
        egui::vec2(ui.available_width(), ACTION_BUTTON_HEIGHT),
        true,
        false,
    )
    .clicked()
    {
        ui.ctx()
            .send_viewport_cmd(egui::ViewportCommand::Close);
    }
}

fn render_card_heading(ui: &mut egui::Ui, number: &str, title: &str) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(number).color(BLUE).size(11.0).strong());
        ui.label(
            egui::RichText::new(title)
                .color(egui::Color32::from_rgb(220, 233, 241))
                .size(15.0)
                .strong(),
        );
    });
}

pub(crate) fn beveled_button(
    ui: &mut egui::Ui,
    label: &str,
    size: egui::Vec2,
    enabled: bool,
    highlighted: bool,
) -> egui::Response {
    ui.add_enabled_ui(enabled, |ui| {
        let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
        let outer = if response.is_pointer_button_down_on() {
            egui::Color32::from_rgb(12, 24, 43)
        } else if response.hovered() {
            egui::Color32::from_rgb(76, 126, 177)
        } else {
            egui::Color32::from_rgb(49, 80, 123)
        };
        let face = if highlighted {
            if response.hovered() {
                egui::Color32::from_rgb(33, 67, 107)
            } else {
                egui::Color32::from_rgb(19, 39, 68)
            }
        } else if response.hovered() {
            egui::Color32::from_rgb(23, 46, 77)
        } else {
            egui::Color32::from_rgb(10, 22, 42)
        };

        let painter = ui.painter();
        painter.rect_filled(rect, egui::CornerRadius::same(4), outer);
        let face_rect = rect.shrink(2.0);
        painter.rect_filled(face_rect, egui::CornerRadius::same(3), face);
        painter.line_segment(
            [
                face_rect.left_top() + egui::vec2(3.0, 1.0),
                face_rect.right_top() + egui::vec2(-3.0, 1.0),
            ],
            egui::Stroke::new(1.0, egui::Color32::from_rgb(106, 145, 185)),
        );
        painter.line_segment(
            [
                face_rect.left_bottom() + egui::vec2(3.0, -1.0),
                face_rect.right_bottom() + egui::vec2(-3.0, -1.0),
            ],
            egui::Stroke::new(1.0, egui::Color32::from_rgb(3, 8, 17)),
        );
        painter.text(
            face_rect.center(),
            egui::Align2::CENTER_CENTER,
            label,
            egui::FontId::proportional(12.0),
            if enabled { ICE } else { MUTED },
        );
        response
    })
    .inner
}

fn render_feature_line(ui: &mut egui::Ui, number: &str, title: &str, detail: &str) {
    ui.horizontal_top(|ui| {
        ui.label(
            egui::RichText::new(number)
                .color(BLUE)
                .size(10.0)
                .strong(),
        );
        ui.vertical(|ui| {
            ui.label(
                egui::RichText::new(title)
                    .color(egui::Color32::from_rgb(210, 225, 235))
                    .size(11.0)
                    .strong(),
            );
            ui.label(
                egui::RichText::new(detail)
                    .color(MUTED)
                    .size(10.0),
            );
        });
    });
}

fn build_pending_bot_start(
    panel: &GameConfigPanel,
    listen_port: u16,
) -> Option<StartBotProcessesEvent> {
    let player_bot = (!panel.bot_command.is_empty()).then(|| panel.bot_command.clone());
    let opponent_bot = (!panel.bot_opponent_command.is_empty()
        && panel.game_type == GameType::VsBot)
        .then(|| panel.bot_opponent_command.clone());

    if player_bot.is_none() && opponent_bot.is_none() {
        return None;
    }

    Some(StartBotProcessesEvent {
        player_bot_command: player_bot,
        opponent_bot_command: opponent_bot,
        player_name: Some(panel.player_name.clone()),
        opponent_name: panel.bot_name.clone(),
        listen_port,
    })
}
