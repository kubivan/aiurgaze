// src/main.rs
mod connection;
mod proxy_ws;
mod ui;
mod controller;
mod map;
mod helpers;
mod units;
mod create_game_request;
mod net_helpers;
mod app_settings;
mod entity_system;

use bevy::prelude::*;
use bevy_health_bar3d::prelude::*;

use std::process::Command;
use bevy_ecs_tilemap::{ TilemapPlugin};
use bevy_egui::{EguiPlugin, EguiPrimaryContextPass};
use bevy_tokio_tasks::{TokioTasksPlugin, TokioTasksRuntime};
use protobuf::Message;
use sc2_proto::sc2api::Response;
use tap::prelude::*;
use crate::controller::{response_controller_system, setup_proxy, ProxyResponseEvent};
use crate::ui::{camera_controls, setup_camera, ui_system, AppState, CameraPanState, DockerStatus, status_bar_system, GameConfigPanel, GameCreated, build_create_game_request, PendingCreateGameRequest};
use crate::units::{UnitRegistry, SelectedUnit, unit_selection_system, UnitHealth, UnitShield, UnitBuildProgress, ObservationUnitTags, cleanup_dead_units};
use crate::units::CurrentOrderAbility;
use crate::units::draw_unit_orders;
use futures_util::StreamExt;
use clap::{Parser, Subcommand};
use std::process::exit;
use bevy::color::palettes::basic::{GREEN, RED};
use sc2_proto::common::Race;
use crate::ui::GameType;
use crate::app_settings::{AppSettings, load_settings, StarcraftConfig};
use crate::entity_system::setup_entity_system;
use crate::ui::game_config_panel::list_maps_folder;

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
fn start_server_container(docker_config : &StarcraftConfig) -> Result<(), String> {
    let image = &docker_config.image;
    let container_name = &docker_config.container_name;

    // Remove any existing container with the same name
    let _ = Command::new("docker")
        .args(["rm", "-f", &container_name])
        .status();

    // // Pull image
    // let _ = Command::new("docker")
    //     .args(["pull", &image])
    //     .status();

    // Get absolute path to maps directory
    let maps_dir = std::env::current_dir()
        .map_err(|e| format!("Failed to get current directory: {e}"))?
        .join("maps");
    let maps_mount = format!("{}:/StarCraftII/Maps", maps_dir.display());

    // Run container detached, auto-remove on stop, bind to localhost
    let status = Command::new("docker")
        .args([
            "run", "-d", "--rm", "-it",
            "--name", &container_name,
            "-p", format!("{}:{}", docker_config.upstream_port, docker_config.upstream_port).as_str(),
            "-v", &maps_mount,
            &image,
        ])
        .status()
        .map_err(|e| format!("Failed to execute docker run: {e}"))?;

    if !status.success() {
        return Err(format!("docker run failed with status: {status}"));
    }

    // Mark as running immediately, skip port check
    Ok(())
}

/// Blocking Docker startup for CLI mode
fn startup_docker_blocking(config: &StarcraftConfig) -> Result<(), String> {
    println!("[startup_docker_blocking] Starting Docker container...");
    let result = start_server_container(&config);
    match &result {
        Ok(_) => println!("[startup_docker_blocking] Docker container started successfully."),
        Err(e) => eprintln!("[startup_docker_blocking] Failed to start Docker: {e}"),
    }
    result
}

/// System to check/start Docker and update status
fn docker_startup_system(
    runtime: Res<TokioTasksRuntime>,
    mut docker_status: ResMut<DockerStatus>,
    docker_config: Res<AppSettings>
) {
    docker_status.clone_from(&DockerStatus::Starting);
    // Clone config to own it in the task
    let starcraft_config = docker_config.starcraft.clone();
    runtime.spawn_background_task(|mut ctx| async move {
        // Use spawn_blocking for blocking code
        let result = tokio::task::spawn_blocking(move ||
            start_server_container(&starcraft_config)
        ).await.unwrap_or_else(|_| Err("Thread panicked".to_string()));
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
            if let Some(mut status_res) = world.world.get_resource_mut::<DockerStatus>() {
                status_res.clone_from(&status);
                println!("[docker_startup_system] Updated DockerStatus to: {:?}", status);
                if status == DockerStatus::Running {
                    println!("Docker running, should start proxy connection now");
                }
            } else {
                println!("[docker_startup_system] DockerStatus resource not found!");
            }
        }).await;
    });
}

/// System to start proxy connection when Docker is running
fn proxy_connect_on_docker_ready(
    docker_status: Res<DockerStatus>,
    mut has_connected: Local<bool>,
    runtime: Res<TokioTasksRuntime>,
    game_created: ResMut<GameCreated>,
    settings: Res<AppSettings>,
) {
    if !*has_connected && *docker_status == DockerStatus::Running && game_created.0 {
        setup_proxy(runtime, settings);
        *has_connected = true;
        // game_created.0 = false;
        println!("Proxy connection started after Docker became ready and game was created");
    }
}

/// Entry point
fn main() {
    let app_settings = load_settings();
    let available_maps = list_maps_folder();
    let mut game_config_panel = GameConfigPanel::from_defaults(&app_settings.game_config_panel, available_maps);

    let cli = Cli::parse();

    // Default values for resources
    let mut app_state = AppState::StartScreen;
    let mut pending_request = PendingCreateGameRequest::default();

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
        if let Err(e) = startup_docker_blocking(&app_settings.starcraft) {
            eprintln!("Error: Could not start Docker container: {e}");
            exit(1);
        }
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


    App::new()
        .add_event::<ProxyResponseEvent>()
        .register_type::<UnitHealth>()
        .register_type::<UnitShield>()
        .register_type::<UnitBuildProgress>()
        .add_plugins(
            DefaultPlugins
                .set(ImagePlugin::default_nearest())
                .set(WindowPlugin {
                    primary_window: Some(Window {
                        title: "aiurgaze - SC2 AI Observer".to_string(),
                        resolution: (app_settings.window.width, app_settings.window.height).into(),
                        resizable: app_settings.window.resizable,
                        ..default()
                    }),
                    ..default()
                })
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
        .insert_resource(game_config_panel)
        .insert_resource(DockerStatus::Starting)
        .insert_resource(pending_request)
        .insert_resource(app_settings) // use loaded settings
        .insert_resource(app_state)
        .add_systems(Startup, setup_entity_system)
        .add_systems(Startup, setup_camera)
        .add_systems(Update, unit_selection_system)
        .add_systems(Update, camera_controls)
        .add_systems(Startup, docker_startup_system)
        .add_systems(EguiPrimaryContextPass, ui_system)
        .add_systems(EguiPrimaryContextPass, status_bar_system)
        .add_systems(Update, cleanup_dead_units.before(response_controller_system))
        .add_systems(Update, response_controller_system)
        .add_systems(Update, proxy_connect_on_docker_ready)
        .add_systems(Update, draw_unit_orders)
        .run();
}
