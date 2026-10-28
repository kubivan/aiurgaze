use crate::app_settings::AppSettings;
use crate::chat_overlay::ChatOverlay;
use crate::controller::{
    CurrentFrame, GameResultState, MapResource, PlayerResources, ProtocolActivityState,
};
use crate::debug_draw::render_debug_texts;
use crate::entity_system::EntitySystem;
use crate::map::Terrain3dSettings;
use crate::proxy_channel::ProxyStreamPause;
use crate::render_layers::LayerRegistry;
use crate::render_view::RenderViewMode;
use crate::ui::hud::{render_game_result, render_hud, render_status_bar};
use crate::ui::DockerStatus;
use crate::ui::{
    render_start_screen, AppState, GameConfigPanel, GameControlsVisibility, GameCreated,
    PendingBotStart, PendingCreateGameRequest, ReplaySaveState, StartScreenPanel, VisionModeChannel,
};
use crate::units::{
    CurrentOrderAbility, SelectedUnit, UnitBuildProgress, UnitCompositionVisibility, UnitProto,
    UnitRegistry, UnitTag, UnitType,
};
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};
use bevy_tokio_tasks::TokioTasksRuntime;

#[derive(SystemParam)]
pub(crate) struct HudParams<'w, 's> {
    docker_status: Res<'w, DockerStatus>,
    activity: Res<'w, ProtocolActivityState>,
    player_res: Res<'w, PlayerResources>,
    chat_overlay: Res<'w, ChatOverlay>,
    map: Option<Res<'w, MapResource>>,
    entity_system: Res<'w, EntitySystem>,
    current_frame: Res<'w, CurrentFrame>,
    cameras: Query<'w, 's, (&'static Camera, &'static GlobalTransform)>,
    render_view_mode: ResMut<'w, RenderViewMode>,
    terrain_3d_settings: ResMut<'w, Terrain3dSettings>,
    game_controls_visibility: ResMut<'w, GameControlsVisibility>,
    start_screen: ResMut<'w, StartScreenPanel>,
    in_progress_query: Query<'w, 's, (&'static UnitType, &'static UnitBuildProgress)>,
    game_result: Res<'w, GameResultState>,
    replay_save_state: ResMut<'w, ReplaySaveState>,
    runtime: Res<'w, TokioTasksRuntime>,
}

fn render_game_screen(
    ui: &mut egui::Ui,
    selected: &Res<SelectedUnit>,
    registry: &Res<UnitRegistry>,
    unit_query: &Query<(&UnitProto, &UnitTag, &CurrentOrderAbility, &UnitType)>,
    vision_mode_channel: &mut ResMut<VisionModeChannel>,
    layer_registry: &mut ResMut<LayerRegistry>,
    unit_visibility: &mut ResMut<UnitCompositionVisibility>,
    playback: &mut ResMut<crate::controller::LoopPlaybackState>,
    proxy_pause: &mut ResMut<ProxyStreamPause>,
    render_view_mode: &mut ResMut<RenderViewMode>,
    terrain_3d_settings: &mut ResMut<Terrain3dSettings>,
    game_controls_visibility: &mut ResMut<GameControlsVisibility>,
) {
    crate::ui::game_controls::render_game_controls(
        ui.ctx(),
        game_controls_visibility,
        selected,
        registry,
        unit_query,
        vision_mode_channel,
        layer_registry,
        unit_visibility,
        playback,
        proxy_pause,
        render_view_mode,
        terrain_3d_settings,
    );
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
    mut playback: ResMut<crate::controller::LoopPlaybackState>,
    mut proxy_pause: ResMut<ProxyStreamPause>,
    mut hud: HudParams,
) {
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };

    let mut viewport_ui = egui::Ui::new(
        ctx.clone(),
        "viewport".into(),
        egui::UiBuilder::new()
            .layer_id(egui::LayerId::background())
            .max_rect(ctx.viewport_rect()),
    );

    match *app_state {
        AppState::StartScreen => {
            render_status_bar(&mut viewport_ui, hud.docker_status, hud.activity);
            render_start_screen(
                &mut viewport_ui,
                &mut app_state,
                &mut game_config_panel,
                &mut game_created,
                &mut pending_request,
                &app_settings,
                &mut pending_bot_start,
                &mut hud.start_screen,
                &hud.runtime,
            );
        }
        AppState::GameScreen => render_game_screen(
            &mut viewport_ui,
            &selected,
            &registry,
            &unit_query,
            &mut vision_mode_channel,
            &mut layer_registry,
            &mut unit_visibility,
            &mut playback,
            &mut proxy_pause,
            &mut hud.render_view_mode,
            &mut hud.terrain_3d_settings,
            &mut hud.game_controls_visibility,
        ),
    }

    if *app_state == AppState::GameScreen {
        render_hud(
            &ctx,
            hud.player_res,
            hud.in_progress_query,
            hud.chat_overlay,
        );
        render_game_result(
            &ctx,
            &hud.game_result,
            &mut hud.replay_save_state,
            &hud.runtime,
        );
        render_debug_texts(
            &ctx,
            hud.current_frame
                .frame
                .as_ref()
                .map(|frame| frame.debug_texts.as_slice())
                .unwrap_or(&[]),
            &layer_registry,
            hud.map.as_deref(),
            &hud.entity_system,
            &hud.cameras,
            *hud.render_view_mode,
        );
    }
}
