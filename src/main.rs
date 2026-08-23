#![allow(dead_code)]
#![allow(deprecated)]
#![allow(clippy::too_many_arguments)]

// src/main.rs
//mod proxy_ws;
mod app_settings;
mod bot_runner;
mod controller;
mod entity_system;
mod fog_material;
mod helpers;
mod map;
mod net_helpers;
mod observation_pipeline;
mod proxy_channel;
mod render_layers;
mod ui;
mod units;
use bevy::mesh::Mesh2d;
use bevy::prelude::*;
use bevy::sprite_render::{Material2dPlugin, MeshMaterial2d};
use bevy::time::Real;
use bevy_health_bar3d::prelude::*;
use fog_material::{FogOfWarMaterial, FogUniforms};

use crate::app_settings::{
    get_assets_dir, get_maps_dir, load_settings, AppSettings, StarcraftConfig,
};
use crate::bot_runner::{bot_process_system, BotProcessStatus, StartBotProcessesEvent};
use crate::controller::{
    map_init_system, protocol_activity_system, refresh_map_colors_on_layer_change,
    response_controller_system, setup_proxies, update_player_resources, FogMaterialHandle,
    FogOfWarData, FogOfWarHandle, GameInfoEvent, LastVisionMode, MapResource, ObservationEvent,
    PlayerResources, ProtocolActivityEvent, ProtocolActivityState,
};
use crate::entity_system::{setup_entity_system, EntitySystem};
use crate::proxy_channel::ProxyReadySignal;
use crate::render_layers::{
    layer_visibility_system, LayerRegistry, RenderLayerKind, RenderLayerMarker,
};
use crate::ui::game_config_panel::list_maps_folder;
use crate::ui::GameType;
use crate::ui::{
    camera_controls, hud_system, setup_camera, status_bar_system, ui_system, AppState,
    CameraPanState, DockerStatus, GameConfigPanel, GameCreated, PendingBotStart,
    PendingCreateGameRequest, VisionModeChannel,
};
use crate::units::draw_unit_orders;
use crate::units::{
    cleanup_dead_units, unit_selection_system, ObservationUnitTags, SelectedUnit,
    UnitBuildProgress, UnitCompositionVisibility, UnitHealth, UnitRegistry, UnitShield,
};
use bevy::asset::AssetPlugin;
use bevy::color::palettes::basic::{GREEN, RED};
use bevy_ecs_tilemap::TilemapPlugin;
use bevy_egui::{EguiPlugin, EguiPrimaryContextPass};
use bevy_tokio_tasks::{TokioTasksPlugin, TokioTasksRuntime};
use std::process::Command;

/// Start the server inside Docker and wait until it's reachable.
/// If `multiplayer` is true, sets SC2_MULTIPLAYER=1 inside the container
/// so that two SC2 instances are started (on upstream_port and upstream_port+1).
fn start_server_container(
    docker_config: &StarcraftConfig,
    multiplayer: bool,
) -> Result<(), String> {
    let image = &docker_config.image;
    let container_name = &docker_config.container_name;

    // Remove any existing container with the same name
    let _ = Command::new("docker")
        .args(["rm", "-f", container_name])
        .status();

    // Pull configured image so release users only need Docker + binary tarball
    let pull_status = Command::new("docker")
        .args(["pull", image])
        .status()
        .map_err(|e| format!("Failed to execute docker pull: {e}"))?;

    if !pull_status.success() {
        eprintln!("docker pull failed; attempting to use local image '{image}'");
    }

    // Get absolute path to maps directory
    let maps_dir = get_maps_dir();
    let maps_mount = format!("{}:/StarCraftII/maps", maps_dir.display());

    // Build port mappings, env vars, and multiplayer ports
    let port1_map = format!(
        "{}:{}",
        docker_config.upstream_port, docker_config.upstream_port
    );
    let port2_map = format!(
        "{}:{}",
        docker_config.upstream_port + 1,
        docker_config.upstream_port + 1
    );

    // Also map the SC2 internal sync ports (server_ports + client_ports).
    // These are derived from upstream_port + 2..+7 and must be reachable between
    // the host and the container so the two SC2 instances can sync.
    let mut port_maps: Vec<String> = vec![port1_map, port2_map];
    if multiplayer {
        for offset in 2..8 {
            let p = docker_config.upstream_port + offset;
            port_maps.push(format!("{}:{}", p, p));
        }
    }

    let mut args: Vec<String> = vec![
        "run".into(),
        "-d".into(),
        "--rm".into(),
        "-it".into(),
        "--name".into(),
        container_name.clone(),
    ];
    for pm in &port_maps {
        args.push("-p".into());
        args.push(pm.clone());
    }
    if multiplayer {
        args.push("-e".into());
        args.push("SC2_MULTIPLAYER=1".into());
    }
    args.push("-v".into());
    args.push(maps_mount);
    args.push(image.clone());

    // Run container detached, auto-remove on stop, bind to localhost
    let status = Command::new("docker")
        .args(&args)
        .status()
        .map_err(|e| format!("Failed to execute docker run: {e}"))?;

    if !status.success() {
        return Err(format!("docker run failed with status: {status}"));
    }

    // Mark as running immediately, skip port check
    Ok(())
}

/// System to check/start Docker and update status.
/// Runs as an Update system — only triggers once when the game is created
/// and Docker hasn't been started yet (status == Idle).
fn docker_startup_system(
    runtime: Res<TokioTasksRuntime>,
    mut docker_status: ResMut<DockerStatus>,
    docker_config: Res<AppSettings>,
    game_created: Res<GameCreated>,
    game_config: Res<GameConfigPanel>,
) {
    if *docker_status != DockerStatus::Idle || !game_created.0 {
        return;
    }
    docker_status.clone_from(&DockerStatus::Starting);
    // Clone config to own it in the task
    let starcraft_config = docker_config.starcraft.clone();
    let multiplayer = game_config.game_type == GameType::VsBot;
    runtime.spawn_background_task(move |mut ctx| async move {
        // Use spawn_blocking for blocking code
        //let result = tokio::task::spawn_blocking(move ||
        //    start_server_container(&starcraft_config)
        //).await.unwrap_or_else(|_| Err("Thread panicked".to_string()));
        let result = start_server_container(&starcraft_config, multiplayer);
        let status = match result {
            Ok(_) => DockerStatus::Running,
            Err(e) => {
                if e.contains("docker run failed") || e.contains("Failed to execute docker run") {
                    DockerStatus::NotFound
                } else {
                    DockerStatus::Error(e)
                }
            }
        };
        ctx.run_on_main_thread(move |world| {
            let Some(mut status_res) = world.world.get_resource_mut::<DockerStatus>() else {
                println!("[docker_startup_system] DockerStatus resource not found!");
                return;
            };

            status_res.clone_from(&status);
            println!(
                "[docker_startup_system] Updated DockerStatus to: {:?}",
                status
            );
            if status == DockerStatus::Running {
                println!("Docker running, should start proxy connection now");
            }
        })
        .await;
    });
}

/// Resource wrapper for ProxyReadySignal.
#[derive(Resource, Default, Clone)]
pub struct ProxyReadyResource(pub Option<ProxyReadySignal>);

/// System to emit StartBotProcessesEvent once the proxy ready signal is available.
/// This ensures bots are only started after proxies are listening.
fn emit_pending_bot_start(
    proxy_ready: Res<ProxyReadyResource>,
    mut pending_bot_start: ResMut<PendingBotStart>,
    mut bot_events: MessageWriter<StartBotProcessesEvent>,
) {
    if proxy_ready.0.is_some() {
        if let Some(event) = pending_bot_start.0.take() {
            println!("[emit_pending_bot_start] Proxy ready signal available, emitting StartBotProcessesEvent");
            bot_events.write(event);
        }
    }
}

/// System to start proxy connections when Docker is running and game is created.
/// Sets up one or two proxy channels depending on game mode (VsAI = 1 proxy, VsBot = 2 proxies).
fn proxy_connect_on_docker_ready(
    docker_status: Res<DockerStatus>,
    mut has_connected: Local<bool>,
    runtime: Res<TokioTasksRuntime>,
    game_created: ResMut<GameCreated>,
    settings: Res<AppSettings>,
    game_config: Res<GameConfigPanel>,
    vision_mode_channel: Res<VisionModeChannel>,
    mut proxy_ready: ResMut<ProxyReadyResource>,
    mut pending_request: ResMut<PendingCreateGameRequest>,
) {
    if !*has_connected && *docker_status == DockerStatus::Running && game_created.0 {
        let is_vs_bot = game_config.game_type == GameType::VsBot;
        let vision_rx = vision_mode_channel.sender.subscribe();
        let create_req = pending_request.0.take();
        let ready_signal = setup_proxies(&runtime, &settings, is_vs_bot, vision_rx, create_req);
        proxy_ready.0 = Some(ready_signal);
        *has_connected = true;
        println!(
            "Proxy connection started after Docker became ready and game was created (VsBot={})",
            is_vs_bot
        );
    }
}

/// Entry point
fn main() {
    let app_settings = load_settings();
    // Print resolved resource directories for debugging
    let assets_dir = get_assets_dir();
    let data_dir = crate::app_settings::get_data_dir();
    let maps_dir = get_maps_dir();
    let config_path = crate::app_settings::get_config_path();
    println!(
        "[startup] assets_dir={} data_dir={} maps_dir={} config_path={}",
        assets_dir.display(),
        data_dir.display(),
        maps_dir.display(),
        config_path.display(),
    );
    let available_maps = list_maps_folder();
    let game_config_panel =
        GameConfigPanel::from_defaults(&app_settings.game_config_panel, available_maps);

    // Default values for resources
    let app_state = AppState::StartScreen;
    let pending_request = PendingCreateGameRequest::default();
    let docker_status = DockerStatus::Idle;

    // Get assets directory for Bevy's AssetPlugin
    let assets_dir = get_assets_dir();

    App::new()
        .add_message::<StartBotProcessesEvent>()
        .add_message::<GameInfoEvent>()
        .add_message::<ObservationEvent>()
        .add_message::<ProtocolActivityEvent>()
        .register_type::<UnitHealth>()
        .register_type::<UnitShield>()
        .register_type::<UnitBuildProgress>()
        .add_plugins(
            DefaultPlugins
                .set(AssetPlugin {
                    file_path: assets_dir.to_string_lossy().to_string(),
                    ..default()
                })
                .set(ImagePlugin::default_nearest())
                .set(WindowPlugin {
                    primary_window: Some(Window {
                        title: "aiurgaze - SC2 AI Observer".to_string(),
                        resolution: (
                            app_settings.window.width as u32,
                            app_settings.window.height as u32,
                        )
                            .into(),
                        resizable: app_settings.window.resizable,
                        ..default()
                    }),
                    ..default()
                }),
        )
        .add_plugins(TilemapPlugin)
        .add_plugins(EguiPlugin::default())
        .add_plugins(TokioTasksPlugin::default())
        .add_plugins(Material2dPlugin::<FogOfWarMaterial>::default())
        .add_plugins(HealthBarPlugin::<UnitHealth>::default())
        .add_plugins(HealthBarPlugin::<UnitShield>::default())
        .add_plugins(HealthBarPlugin::<UnitBuildProgress>::default())
        .insert_resource(
            ColorScheme::<UnitHealth>::new()
                .foreground_color(ForegroundColor::Static(GREEN.into()))
                .background_color(RED.into()),
        )
        .insert_resource(
            ColorScheme::<UnitShield>::new()
                .foreground_color(ForegroundColor::Static(Color::srgb(0.3, 0.6, 1.0)))
                .background_color(Color::srgb(0.1, 0.1, 0.3)),
        )
        .insert_resource(
            ColorScheme::<UnitBuildProgress>::new()
                .foreground_color(ForegroundColor::Static(Color::srgb(1.0, 0.9, 0.2)))
                .background_color(Color::srgb(0.3, 0.3, 0.1)),
        )
        .insert_resource(GameCreated(false))
        .insert_resource(UnitRegistry::default())
        .insert_resource(SelectedUnit::default())
        .insert_resource(ObservationUnitTags::default())
        .insert_resource(CameraPanState::default())
        .insert_resource(BotProcessStatus::default())
        .insert_resource(game_config_panel)
        .insert_resource(docker_status)
        .insert_resource(pending_request)
        .insert_resource(app_settings) // use loaded settings
        .insert_resource(app_state)
        .insert_resource(VisionModeChannel::default())
        .insert_resource(LastVisionMode::default())
        .insert_resource(ProxyReadyResource::default())
        .insert_resource(PendingBotStart::default())
        .insert_resource(FogOfWarData::default())
        .insert_resource(LayerRegistry::default())
        .insert_resource(UnitCompositionVisibility::default())
        .insert_resource(ProtocolActivityState::default())
        .insert_resource(PlayerResources::default())
        .add_systems(Startup, setup_entity_system)
        .add_systems(Startup, setup_camera)
        .add_systems(Update, unit_selection_system)
        .add_systems(Update, camera_controls)
        .add_systems(Update, docker_startup_system)
        .add_systems(EguiPrimaryContextPass, ui_system)
        .add_systems(EguiPrimaryContextPass, status_bar_system)
        .add_systems(EguiPrimaryContextPass, hud_system)
        .add_systems(
            Update,
            map_init_system.run_if(not(resource_exists::<MapResource>)),
        )
        .add_systems(Update, response_controller_system)
        .add_systems(Update, protocol_activity_system)
        .add_systems(Update, update_player_resources)
        .add_systems(
            Update,
            refresh_map_colors_on_layer_change.after(response_controller_system),
        )
        .add_systems(Update, cleanup_dead_units.after(response_controller_system))
        .add_systems(Update, proxy_connect_on_docker_ready)
        .add_systems(
            Update,
            emit_pending_bot_start.after(proxy_connect_on_docker_ready),
        )
        .add_systems(Update, bot_process_system.after(emit_pending_bot_start))
        .add_systems(Update, draw_unit_orders)
        .add_systems(PostUpdate, layer_visibility_system)
        .add_systems(
            Update,
            spawn_fog_overlay
                .run_if(resource_added::<FogOfWarHandle>)
                .after(map_init_system),
        )
        .add_systems(Update, update_fog_texture.after(response_controller_system))
        .add_systems(
            Update,
            update_fog_uniforms
                .after(update_fog_texture)
                .run_if(resource_exists::<FogMaterialHandle>),
        )
        .run();
}

/// Spawns the fog-of-war overlay mesh once the FogOfWarHandle resource exists
/// (i.e., on the frame after map_init_system inserts it).
fn spawn_fog_overlay(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<FogOfWarMaterial>>,
    fog_handle: Res<FogOfWarHandle>,
    map_res: Res<MapResource>,
    entity_system: Res<EntitySystem>,
) {
    let (width, height) = map_res.static_layers.get_dimensions();
    let tile_size = entity_system.map_config.tile_size;
    let w = width as f32 * tile_size;
    let h = height as f32 * tile_size;

    let material = materials.add(FogOfWarMaterial {
        uniforms: FogUniforms {
            camera_pos: Vec3::new(0.0, 0.0, 1.0),
            time: 0.0,
            // Default sun direction: upper-right, slightly from above.
            light_dir: Vec3::new(0.6, 0.7, -0.4).normalize(),
            _pad0: 0.0,
            world_size: Vec2::new(w, h),
            world_origin: Vec2::ZERO,
        },
        fog_texture: fog_handle.handle.clone(),
    });

    // Store the material handle so update_fog_texture can invalidate
    // the material's bind group when the underlying Image changes.
    commands.insert_resource(FogMaterialHandle {
        handle: material.clone(),
    });

    commands.spawn((
        Mesh2d(meshes.add(Rectangle::new(w, h))),
        MeshMaterial2d(material),
        Transform::from_xyz(0.0, 0.0, 1000.0),
        RenderLayerMarker(RenderLayerKind::Terrain),
    ));
    println!(
        "[fog] spawned fog overlay: {}x{} px, texture {}x{}",
        w, h, width, height
    );
}

/// Flush pending fog-of-war CPU data to the GPU texture asset.
///
/// After writing new pixel data to the `Image`, we also touch the
/// `FogOfWarMaterial` asset so Bevy re-creates its bind group with
/// the freshly-uploaded `GpuImage`.  Without this the material keeps
/// sampling the texture that was current when its bind group was
/// first built.
fn update_fog_texture(
    fog_handle: Option<Res<FogOfWarHandle>>,
    mat_handle: Option<Res<FogMaterialHandle>>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<FogOfWarMaterial>>,
    mut fog_data: Option<ResMut<FogOfWarData>>,
) {
    let Some(fog_handle) = fog_handle else { return };
    let Some(fog_data) = fog_data.as_mut() else {
        return;
    };
    if !fog_data.dirty {
        return;
    }
    if let Some(mut image) = images.get_mut(&fog_handle.handle) {
        image.data = Some(fog_data.data.clone());
    }
    // Invalidate the material so its bind group is rebuilt with the new GpuImage.
    if let Some(ref mat_handle) = mat_handle {
        // get_mut marks the asset as Modified → triggers re-extraction + re-preparation.
        let _ = materials.get_mut(&mat_handle.handle);
    }
    fog_data.dirty = false;
}

/// Update fog material uniforms every frame (time, camera position).
///
/// Runs only when FogMaterialHandle exists. Updates the uniform block
/// so the shader has current time for animation and camera position
/// for depth fade. This marks the material as Modified.
fn update_fog_uniforms(
    mat_handle: Res<FogMaterialHandle>,
    mut materials: ResMut<Assets<FogOfWarMaterial>>,
    time: Res<Time<Real>>,
    camera_query: Query<(&Transform, &Projection), With<Camera>>,
) {
    let Some(mut mat) = materials.get_mut(&mat_handle.handle) else {
        return;
    };

    mat.uniforms.time = time.elapsed_secs();

    // Read camera transform + ortho scale.
    if let Ok((cam_tf, projection)) = camera_query.single() {
        mat.uniforms.camera_pos = Vec3::new(
            cam_tf.translation.x,
            cam_tf.translation.y,
            match projection {
                Projection::Orthographic(ortho) => ortho.scale,
                _ => 1.0,
            },
        );
    }
}
