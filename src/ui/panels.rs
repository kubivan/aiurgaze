use crate::app_settings::AppSettings;
use crate::bot_runner::StartBotProcessesEvent;
use crate::observation_pipeline::VisionMode;
use crate::render_layers::{LayerRegistry, RenderLayerKind};
use crate::ui::selected_unit_info::render_selected_unit_info;
use crate::ui::{
    build_create_game_request, show_game_config_panel, AppState, GameConfigPanel, GameCreated,
    GameType, PendingBotStart, PendingCreateGameRequest, VisionModeChannel,
};
use crate::units::{
    CurrentOrderAbility, SelectedUnit, UnitCompositionVisibility, UnitProto, UnitRegistry, UnitTag,
    UnitType,
};
use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};

fn build_pending_bot_start(
    panel: &GameConfigPanel,
    listen_port: u16,
) -> Option<StartBotProcessesEvent> {
    let player_bot = if panel.bot_command.is_empty() {
        None
    } else {
        Some(panel.bot_command.clone())
    };
    let opponent_bot =
        if panel.bot_opponent_command.is_empty() || panel.game_type != GameType::VsBot {
            None
        } else {
            Some(panel.bot_opponent_command.clone())
        };

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

fn render_start_screen(
    ctx: &egui::Context,
    app_state: &mut ResMut<AppState>,
    game_config_panel: &mut ResMut<GameConfigPanel>,
    game_created: &mut ResMut<GameCreated>,
    pending_request: &mut ResMut<PendingCreateGameRequest>,
    app_settings: &Res<AppSettings>,
    pending_bot_start: &mut ResMut<PendingBotStart>,
) {
    let mut viewport_ui = egui::Ui::new(
        ctx.clone(),
        "viewport".into(),
        egui::UiBuilder::new()
            .layer_id(egui::LayerId::background())
            .max_rect(ctx.viewport_rect()),
    );

    egui::CentralPanel::default().show(&mut viewport_ui, |ui| {
        ui.heading("SC2 Proxy");
        ui.separator();
        if show_game_config_panel(ui, game_config_panel) {
            match build_create_game_request(game_config_panel) {
                Ok(req) => {
                    println!("!!! CreateGame request: req={:?}", req);
                    pending_request.0 = Some(req);
                    game_created.0 = true;
                    **app_state = AppState::GameScreen;
                    pending_bot_start.0 = build_pending_bot_start(
                        game_config_panel,
                        app_settings.starcraft.listen_port,
                    );
                }
                Err(e) => {
                    eprintln!("Create game failed: {}", e);
                    game_config_panel.error_message = Some(e);
                }
            }
        }
    });
}

fn render_game_screen(
    ctx: &egui::Context,
    selected: &Res<SelectedUnit>,
    registry: &Res<UnitRegistry>,
    unit_query: &Query<(&UnitProto, &UnitTag, &CurrentOrderAbility, &UnitType)>,
    vision_mode_channel: &mut ResMut<VisionModeChannel>,
    layer_registry: &mut ResMut<LayerRegistry>,
    unit_visibility: &mut ResMut<UnitCompositionVisibility>,
) {
    let mut viewport_ui = egui::Ui::new(
        ctx.clone(),
        "viewport".into(),
        egui::UiBuilder::new()
            .layer_id(egui::LayerId::background())
            .max_rect(ctx.viewport_rect()),
    );

    egui::Panel::right("unit_info_panel")
        .resizable(true)
        .default_size(300.0)
        .show(&mut viewport_ui, |ui| {
            ui.heading("Game Controls");
            ui.separator();

            ui.label("Vision Mode:");
            let current_mode = vision_mode_channel.current;
            egui::ComboBox::from_id_salt("vision_mode_combo")
                .selected_text(format!("{}", current_mode))
                .show_ui(ui, |ui| {
                    for mode in [VisionMode::Player1, VisionMode::Player2, VisionMode::All] {
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
            render_selected_unit_info(ui, selected, registry, unit_query);
        });

    egui::CentralPanel::default()
        .frame(egui::Frame::NONE)
        .show(&mut viewport_ui, |_ui| {});
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

    match *app_state {
        AppState::StartScreen => render_start_screen(
            ctx,
            &mut app_state,
            &mut game_config_panel,
            &mut game_created,
            &mut pending_request,
            &app_settings,
            &mut pending_bot_start,
        ),
        AppState::GameScreen => render_game_screen(
            ctx,
            &selected,
            &registry,
            &unit_query,
            &mut vision_mode_channel,
            &mut layer_registry,
            &mut unit_visibility,
        ),
    }
}
