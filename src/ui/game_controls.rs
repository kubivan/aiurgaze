use crate::controller::{LoopPlaybackMode, LoopPlaybackState};
use crate::map::Terrain3dSettings;
use crate::sources::game::VisionMode;
use crate::proxy_channel::ProxyStreamPause;
use crate::render_layers::{LayerRegistry, RenderLayerKind};
use crate::render_view::RenderViewMode;
use crate::ui::start_screen::beveled_button;
use crate::ui::selected_unit_info::render_selected_unit_info;
use crate::units::{
    CurrentOrderAbility, SelectedUnit, UnitCompositionVisibility, UnitProto, UnitRegistry, UnitTag,
    UnitType,
};
use bevy::prelude::*;
use bevy_egui::egui;

const PANEL_WIDTH: f32 = 286.0;
const BLUE: egui::Color32 = egui::Color32::from_rgb(105, 166, 213);
const PALE: egui::Color32 = egui::Color32::from_rgb(205, 224, 238);
const MUTED: egui::Color32 = egui::Color32::from_rgb(137, 159, 180);

#[derive(Resource)]
pub(crate) struct GameControlsVisibility {
    pub open: bool,
}

impl Default for GameControlsVisibility {
    fn default() -> Self {
        Self { open: true }
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn render_game_controls(
    ctx: &egui::Context,
    visibility: &mut GameControlsVisibility,
    selected: &Res<SelectedUnit>,
    registry: &Res<UnitRegistry>,
    unit_query: &Query<(&UnitProto, &UnitTag, &CurrentOrderAbility, &UnitType)>,
    vision_mode_channel: &mut ResMut<crate::ui::VisionModeChannel>,
    layer_registry: &mut ResMut<LayerRegistry>,
    unit_visibility: &mut ResMut<UnitCompositionVisibility>,
    playback: &mut ResMut<LoopPlaybackState>,
    proxy_pause: &mut ResMut<ProxyStreamPause>,
    render_view_mode: &mut ResMut<RenderViewMode>,
    terrain_3d_settings: &mut ResMut<Terrain3dSettings>,
) {
    if !visibility.open {
        egui::Area::new("show_game_controls".into())
            .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-12.0, 12.0))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                if beveled_button(ui, "CONTROLS", egui::vec2(116.0, 38.0), true, true).clicked() {
                    visibility.open = true;
                }
            });
        return;
    }

    let max_height = (ctx.input(|input| input.viewport_rect().height()) - 24.0).max(180.0);
    egui::Window::new("game_controls_overlay")
        .title_bar(false)
        .collapsible(false)
        .resizable(false)
        .default_width(PANEL_WIDTH + 24.0)
        .max_width(PANEL_WIDTH + 24.0)
        .max_height(max_height)
        .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-12.0, 12.0))
        .frame(
            egui::Frame::new()
                .fill(egui::Color32::from_rgba_unmultiplied(8, 14, 26, 232))
                .stroke(egui::Stroke::new(
                    1.0,
                    egui::Color32::from_rgba_unmultiplied(68, 108, 154, 190),
                ))
                .corner_radius(egui::CornerRadius::same(4))
                .inner_margin(egui::Margin::same(12)),
        )
        .show(ctx, |ui| {
            ui.set_width(PANEL_WIDTH);
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("TACTICAL CONTROLS")
                        .color(PALE)
                        .size(13.0)
                        .strong(),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if beveled_button(ui, "HIDE", egui::vec2(48.0, 28.0), true, false).clicked() {
                        visibility.open = false;
                    }
                });
            });
            ui.separator();
            render_view_controls(ui, render_view_mode, terrain_3d_settings);
            ui.add_space(7.0);
            render_vision_controls(ui, vision_mode_channel);
            ui.add_space(10.0);
            render_playback_controls(ui, playback, proxy_pause);
            ui.add_space(6.0);
            render_layer_controls(ui, layer_registry);
            ui.add_space(4.0);
            ui.checkbox(&mut unit_visibility.show_orders, "Show order indicators");
            render_selected_unit_info(ui, selected, registry, unit_query);
        });
}

fn render_view_controls(
    ui: &mut egui::Ui,
    render_view_mode: &mut ResMut<RenderViewMode>,
    terrain_3d_settings: &mut ResMut<Terrain3dSettings>,
) {
    section_label(ui, "VIEW");
    egui::ComboBox::from_id_salt("render_view_mode_combo")
        .selected_text(match **render_view_mode {
            RenderViewMode::TwoD => "2D",
            RenderViewMode::ThreeD => "3D (experimental)",
        })
        .width(ui.available_width())
        .show_ui(ui, |ui| {
            ui.selectable_value(&mut **render_view_mode, RenderViewMode::TwoD, "2D");
            ui.selectable_value(
                &mut **render_view_mode,
                RenderViewMode::ThreeD,
                "3D (experimental)",
            );
        });

    if **render_view_mode == RenderViewMode::ThreeD {
        let mut height_scale = terrain_3d_settings.height_scale;
        if ui
            .add(egui::Slider::new(&mut height_scale, 0.1..=4.0).text("Terrain height"))
            .changed()
        {
            terrain_3d_settings.height_scale = height_scale;
        }
    }
}

fn render_vision_controls(
    ui: &mut egui::Ui,
    vision_mode_channel: &mut ResMut<crate::ui::VisionModeChannel>,
) {
    section_label(ui, "VISION");
    let current_mode = vision_mode_channel.current;
    egui::ComboBox::from_id_salt("vision_mode_combo")
        .selected_text(format!("{current_mode}"))
        .width(ui.available_width())
        .show_ui(ui, |ui| {
            for mode in [VisionMode::Player1, VisionMode::Player2, VisionMode::All] {
                if ui
                    .selectable_label(current_mode == mode, format!("{mode}"))
                    .clicked()
                {
                    vision_mode_channel.set(mode);
                }
            }
        });
}

fn render_playback_controls(
    ui: &mut egui::Ui,
    playback: &mut ResMut<LoopPlaybackState>,
    proxy_pause: &mut ResMut<ProxyStreamPause>,
) {
    section_label(ui, "PLAYBACK");
    ui.label(
        egui::RichText::new(format!(
            "{:?}  /  LOOP {}",
            playback.mode,
            playback.current_loop()
        ))
        .color(MUTED)
        .size(10.0),
    );
    ui.horizontal(|ui| {
        if beveled_button(ui, "BACK", egui::vec2(66.0, 32.0), true, false).clicked() {
            playback.step_back();
        }
        let is_live = playback.mode == LoopPlaybackMode::Live;
        if beveled_button(
            ui,
            if is_live { "PAUSE" } else { "RESUME" },
            egui::vec2(94.0, 32.0),
            true,
            true,
        )
        .clicked()
        {
            if is_live {
                playback.pause();
                proxy_pause.set_paused(true);
            } else {
                playback.resume();
                proxy_pause.set_paused(false);
            }
        }
        if beveled_button(ui, "NEXT", egui::vec2(66.0, 32.0), true, false).clicked() {
            playback.step_forward();
        }
    });
}

fn render_layer_controls(ui: &mut egui::Ui, layer_registry: &mut ResMut<LayerRegistry>) {
    egui::CollapsingHeader::new(
        egui::RichText::new("MAP LAYERS")
            .color(PALE)
            .size(11.0)
            .strong(),
    )
    .default_open(false)
    .show(ui, |ui| {
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
    });
}

fn section_label(ui: &mut egui::Ui, label: &str) {
    ui.label(
        egui::RichText::new(label)
            .color(BLUE)
            .size(9.0)
            .strong(),
    );
}
