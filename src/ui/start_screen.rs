use crate::app_settings::AppSettings;
use crate::bot_runner::StartBotProcessesEvent;
use crate::ui::{
    build_create_game_request, show_game_config_panel, AppState, GameConfigPanel, GameCreated,
    GameType, PendingBotStart, PendingCreateGameRequest,
};
use bevy::prelude::*;
use bevy_egui::egui;

const CYAN: egui::Color32 = egui::Color32::from_rgb(76, 205, 238);
const MUTED: egui::Color32 = egui::Color32::from_rgb(130, 157, 177);

pub(crate) fn render_start_screen(
    ui: &mut egui::Ui,
    app_state: &mut ResMut<AppState>,
    game_config_panel: &mut ResMut<GameConfigPanel>,
    game_created: &mut ResMut<GameCreated>,
    pending_request: &mut ResMut<PendingCreateGameRequest>,
    app_settings: &Res<AppSettings>,
    pending_bot_start: &mut ResMut<PendingBotStart>,
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
                            render_launch_layout(
                                ui,
                                game_config_panel,
                                app_state,
                                game_created,
                                pending_request,
                                app_settings,
                                pending_bot_start,
                            );
                        },
                    );
                });
            });
        });
}

fn apply_terminal_theme(ui: &mut egui::Ui) {
    let style = ui.style_mut();
    style.visuals = egui::Visuals::dark();
    style.visuals.panel_fill = egui::Color32::from_rgb(6, 12, 20);
    style.visuals.window_fill = egui::Color32::from_rgb(10, 19, 29);
    style.visuals.faint_bg_color = egui::Color32::from_rgb(14, 27, 39);
    style.visuals.extreme_bg_color = egui::Color32::from_rgb(5, 10, 16);
    style.visuals.text_edit_bg_color = Some(egui::Color32::from_rgb(7, 15, 23));
    style.visuals.selection.bg_fill = egui::Color32::from_rgb(17, 88, 115);
    style.visuals.selection.stroke = egui::Stroke::new(1.0, CYAN);
    style.visuals.widgets.noninteractive.fg_stroke.color =
        egui::Color32::from_rgb(205, 221, 232);
    style.visuals.widgets.inactive.bg_fill = egui::Color32::from_rgb(12, 25, 36);
    style.visuals.widgets.inactive.bg_stroke =
        egui::Stroke::new(1.0, egui::Color32::from_rgb(45, 75, 94));
    style.visuals.widgets.hovered.bg_fill = egui::Color32::from_rgb(17, 48, 65);
    style.visuals.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, CYAN);
    style.visuals.widgets.active.bg_fill = egui::Color32::from_rgb(20, 70, 88);
    style.visuals.widgets.active.bg_stroke = egui::Stroke::new(1.0, CYAN);
    style.spacing.item_spacing = egui::vec2(10.0, 8.0);
    style.spacing.button_padding = egui::vec2(12.0, 8.0);
}

fn paint_backdrop(ui: &egui::Ui) {
    let rect = ui.max_rect();
    let painter = ui.painter();
    painter.rect_filled(rect, 0.0, egui::Color32::from_rgb(6, 12, 20));

    let grid = egui::Color32::from_rgba_unmultiplied(42, 83, 105, 24);
    let spacing = 56.0;
    let mut x = rect.left();
    while x < rect.right() {
        painter.line_segment(
            [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
            egui::Stroke::new(1.0, grid),
        );
        x += spacing;
    }
    let mut y = rect.top();
    while y < rect.bottom() {
        painter.line_segment(
            [egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)],
            egui::Stroke::new(1.0, grid),
        );
        y += spacing;
    }

    for index in 0..46 {
        let x = rect.left() + ((index * 73 % 997) as f32 / 997.0) * rect.width();
        let y = rect.top() + ((index * 193 % 991) as f32 / 991.0) * rect.height();
        let alpha = 35 + (index % 4) as u8 * 18;
        painter.circle_filled(
            egui::pos2(x, y),
            if index % 7 == 0 { 1.8 } else { 1.0 },
            egui::Color32::from_rgba_unmultiplied(115, 205, 229, alpha),
        );
    }

    painter.line_segment(
        [
            egui::pos2(rect.left(), rect.bottom() - 1.0),
            egui::pos2(rect.right(), rect.bottom() - 1.0),
        ],
        egui::Stroke::new(1.0, egui::Color32::from_rgb(29, 74, 96)),
    );
}

fn render_masthead(ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("◈").color(CYAN).size(24.0).strong());
        ui.vertical(|ui| {
            ui.label(
                egui::RichText::new("AIURGAZE")
                    .color(egui::Color32::from_rgb(224, 237, 245))
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
                .color(CYAN)
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
        egui::Stroke::new(1.0, CYAN),
    );
}

#[allow(clippy::too_many_arguments)]
fn render_launch_layout(
    ui: &mut egui::Ui,
    game_config_panel: &mut GameConfigPanel,
    app_state: &mut ResMut<AppState>,
    game_created: &mut ResMut<GameCreated>,
    pending_request: &mut ResMut<PendingCreateGameRequest>,
    app_settings: &Res<AppSettings>,
    pending_bot_start: &mut ResMut<PendingBotStart>,
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
                    .fill(egui::Color32::from_rgb(9, 18, 28))
                    .stroke(egui::Stroke::new(
                        1.0,
                        egui::Color32::from_rgb(45, 91, 114),
                    ))
                    .corner_radius(egui::CornerRadius::same(4))
                    .inner_margin(egui::Margin::same(20))
                    .show(ui, |ui| {
                        ui.set_min_width(right_width - 40.0);
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new("01")
                                    .color(CYAN)
                                    .size(11.0)
                                    .strong(),
                            );
                            ui.label(
                                egui::RichText::new("MISSION CONFIGURATION")
                                    .color(egui::Color32::from_rgb(220, 233, 241))
                                    .size(15.0)
                                    .strong(),
                            );
                        });
                        ui.label(
                            egui::RichText::new("Define the battlefield and initialize a session.")
                                .color(MUTED)
                                .size(11.0),
                        );
                        ui.add_space(14.0);
                        ui.separator();

                        let scroll_height = (available_height - 130.0).max(140.0);
                        egui::ScrollArea::vertical()
                            .max_height(scroll_height)
                            .show(ui, |ui| {
                                if show_game_config_panel(ui, game_config_panel) {
                                    match build_create_game_request(game_config_panel) {
                                        Ok(request) => {
                                            println!("!!! CreateGame request: req={request:?}");
                                            pending_request.0 = Some(request);
                                            game_created.0 = true;
                                            **app_state = AppState::GameScreen;
                                            pending_bot_start.0 = build_pending_bot_start(
                                                game_config_panel,
                                                app_settings.starcraft.listen_port,
                                            );
                                        }
                                        Err(error) => {
                                            eprintln!("Create game failed: {error}");
                                            game_config_panel.error_message = Some(error);
                                        }
                                    }
                                }
                            });
                    });
            },
        );
    });
}

fn render_welcome_panel(ui: &mut egui::Ui) {
    ui.add_space(20.0);
    ui.label(
        egui::RichText::new("COMMAND DECK  /  01")
            .color(CYAN)
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

fn render_feature_line(ui: &mut egui::Ui, number: &str, title: &str, detail: &str) {
    ui.horizontal_top(|ui| {
        ui.label(
            egui::RichText::new(number)
                .color(CYAN)
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
