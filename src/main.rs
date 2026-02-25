// src/main.rs
//mod proxy_ws;
mod app_settings;
mod bot_runner;
mod controller;
mod create_game_request;
mod entity_system;
mod helpers;
mod map;
mod net_helpers;
mod observation_pipeline;
mod proxy_channel;
mod ui;
mod units;

use bevy::prelude::*;
use bevy_health_bar3d::prelude::*;

use crate::app_settings::{
    get_assets_dir, get_maps_dir, load_settings, AppSettings, StarcraftConfig,
};
use crate::bot_runner::{bot_process_system, BotProcessStatus, StartBotProcessesEvent};
use crate::controller::{
    map_init_system, response_controller_system, setup_proxies, GameInfoEvent, LastVisionMode,
    MapResource, ObservationEvent,
};
use crate::entity_system::setup_entity_system;
use crate::proxy_channel::ProxyReadySignal;
use crate::ui::game_config_panel::list_maps_folder;
use crate::ui::GameType;
use crate::ui::{
    build_create_game_request, camera_controls, setup_camera, status_bar_system, ui_system,
    AppState, CameraPanState, DockerStatus, GameConfigPanel, GameCreated, PendingBotStart,
    PendingCreateGameRequest, VisionModeChannel,
};
use crate::units::draw_unit_orders;
use crate::units::{
    cleanup_dead_units, unit_selection_system, ObservationUnitTags, SelectedUnit,
    UnitBuildProgress, UnitHealth, UnitRegistry, UnitShield,
};
use bevy::asset::AssetPlugin;
use bevy::color::palettes::basic::{GREEN, RED};
use bevy_ecs_tilemap::TilemapPlugin;
use bevy_egui::{EguiPlugin, EguiPrimaryContextPass};
use bevy_tokio_tasks::{TokioTasksPlugin, TokioTasksRuntime};
use clap::{Parser, Subcommand};
use sc2_proto::common::Race;
use std::process::exit;
use std::process::Command;
 

fn parse_game_type(mode: &str) -> Option<GameType> {
    match mode.to_lowercase().as_str() {
        "vsai" => Some(GameType::VsAI),
        "vsbot" => Some(GameType::VsBot),
        _ => None,
    }
}

fn parse_race(race: &str) -> Option<Race> {
    match race.to_lowercase().as_str() {
        "terran" => Some(Race::Terran),
        "zerg" => Some(Race::Zerg),
        "protoss" => Some(Race::Protoss),
        "random" => Some(Race::Random),
        _ => None,
    }
}

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<CliCommands>,
}

#[derive(Subcommand)]
enum CliCommands {
    /// Create a new game directly from the command line
    CreateGame {
        #[arg(long)]
        mode: Option<String>,
        #[arg(long)]
        race: Option<String>,
        // Add more options as needed
    },
}

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
        return Err(format!("docker pull failed with status: {pull_status}"));
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

/// Blocking Docker startup for CLI mode
fn startup_docker_blocking(config: &StarcraftConfig, multiplayer: bool) -> Result<(), String> {
    println!(
        "[startup_docker_blocking] Starting Docker container (multiplayer={})...",
        multiplayer
    );
    let result = start_server_container(config, multiplayer);
    match &result {
        Ok(_) => println!("[startup_docker_blocking] Docker container started successfully."),
        Err(e) => eprintln!("[startup_docker_blocking] Failed to start Docker: {e}"),
    }
    result
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
#[allow(clippy::too_many_arguments)]
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
    let config_dir = crate::app_settings::get_config_dir();
    let dev_override = std::env::var("AIURGAZE_LOCAL_RESOURCES").unwrap_or_default();
    println!(
        "[startup] assets_dir={} data_dir={} maps_dir={} config_dir={} AIURGAZE_LOCAL_RESOURCES={}",
        assets_dir.display(),
        data_dir.display(),
        maps_dir.display(),
        config_dir.display(),
        dev_override
    );
    let available_maps = list_maps_folder();
    let mut game_config_panel =
        GameConfigPanel::from_defaults(&app_settings.game_config_panel, available_maps);

    let cli = Cli::parse();

    // Default values for resources
    let mut app_state = AppState::StartScreen;
    let mut pending_request = PendingCreateGameRequest::default();
    let mut docker_status = DockerStatus::Idle;

    if let Some(CliCommands::CreateGame { mode, race }) = cli.command {
        // Check required params
        if mode.is_none() || race.is_none() {
            eprintln!("Error: --mode and --race are required for create_game\n");
            eprintln!("Usage: aiurgaze create_game --mode=<MODE> --race=<RACE>");
            exit(1);
        }
        let mode_val = mode.unwrap();
        let race_val = race.unwrap();
        let game_type = parse_game_type(&mode_val);
        let race_enum = parse_race(&race_val);
        if game_type.is_none() || race_enum.is_none() {
            eprintln!("Error: Invalid mode or race value\n");
            eprintln!("Allowed modes: vsAI, vsBot\nAllowed races: terran, zerg, protoss, random");
            exit(1);
        }
        // Start Docker synchronously in CLI mode
        if let Err(e) = startup_docker_blocking(
            &app_settings.starcraft,
            game_config_panel.game_type == GameType::VsBot,
        ) {
            eprintln!("Error: Could not start Docker container: {e}");
            exit(1);
        }
        docker_status = DockerStatus::Running;
        // Set up resources to skip the start screen
        app_state = AppState::GameScreen;
        game_config_panel.game_type = game_type.unwrap();
        game_config_panel.ai_race = Some(race_enum.unwrap());

        // Build the request and store it in the resource to be sent by ui_system
        match build_create_game_request(&game_config_panel) {
            Ok(req) => {
                println!("[CLI] CreateGame request built, will be sent by ui_system within Bevy");
                pending_request.0 = Some(req);
            }
            Err(e) => {
                eprintln!("Error: Failed to build create game request: {}", e);
                exit(1);
            }
        }
    }

    // Get assets directory for Bevy's AssetPlugin
    let assets_dir = get_assets_dir();

    App::new()
        .add_message::<StartBotProcessesEvent>()
        .add_message::<GameInfoEvent>()
        .add_message::<ObservationEvent>()
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
        .add_systems(Startup, setup_entity_system)
        .add_systems(Startup, setup_camera)
        .add_systems(Update, unit_selection_system)
        .add_systems(Update, camera_controls)
        .add_systems(Update, docker_startup_system)
        .add_systems(EguiPrimaryContextPass, ui_system)
        .add_systems(EguiPrimaryContextPass, status_bar_system)
        .add_systems(
            Update,
            map_init_system.run_if(not(resource_exists::<MapResource>)),
        )
        .add_systems(Update, response_controller_system)
        .add_systems(Update, cleanup_dead_units.after(response_controller_system))
        .add_systems(Update, proxy_connect_on_docker_ready)
        .add_systems(
            Update,
            emit_pending_bot_start.after(proxy_connect_on_docker_ready),
        )
        .add_systems(Update, bot_process_system.after(emit_pending_bot_start))
        .add_systems(Update, draw_unit_orders)
        .run();
}
