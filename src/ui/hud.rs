use crate::controller::{PlayerResources, ProtocolActivityState};
use crate::ui::AppState;
use crate::units::{UnitBuildProgress, UnitType};
use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};

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

            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("◈").color(mineral_color).size(15.0));
                ui.label(
                    egui::RichText::new(format!("{:>6}", player_res.minerals))
                        .color(mineral_color)
                        .strong()
                        .size(15.0),
                );
                ui.add_space(4.0);

                ui.label(egui::RichText::new("⬡").color(vespene_color).size(15.0));
                ui.label(
                    egui::RichText::new(format!("{:>6}", player_res.vespene))
                        .color(vespene_color)
                        .strong()
                        .size(15.0),
                );
                ui.add_space(4.0);

                ui.label(egui::RichText::new("⬆").color(supply_color).size(15.0));
                ui.label(
                    egui::RichText::new(format!("{}/{}", player_res.food_used, player_res.food_cap))
                        .color(supply_color)
                        .strong()
                        .size(15.0),
                );
                ui.add_space(8.0);

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

            if player_res.game_loop > 0 {
                ui.label(
                    egui::RichText::new(format!("loop {}", player_res.game_loop))
                        .color(egui::Color32::from_rgba_premultiplied(100, 115, 140, 180))
                        .size(10.0),
                );
            }
        });
}
