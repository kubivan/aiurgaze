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

use bevy::prelude::*;

use tokio::runtime::Runtime;
use tokio::sync::mpsc;
use std::process::Command;
use std::net::TcpStream;
use std::time::{Duration, Instant};
use bevy_ecs_tilemap::{ TilemapPlugin};
use bevy_ecs_tilemap::prelude::TilemapRenderSettings;
use bevy_egui::{EguiPlugin, EguiPrimaryContextPass};
use bevy_tokio_tasks::{TokioTasksPlugin, TokioTasksRuntime};
use protobuf::Message;
use sc2_proto::sc2api::Response;
use tap::prelude::*;
use crate::controller::{response_controller_system, setup_proxy};
use crate::ui::{camera_controls, camera_pan_system, setup_camera, ui_system, AppState, CameraPanState, DockerStatus, status_bar_system, GameConfigPanel, GameCreated};
use crate::units::{UnitRegistry, UnitIconAssets, preload_unit_icons, SelectedUnit, unit_selection_system};
use crate::ui::selected_unit_panel_system;
use futures_util::StreamExt;
use clap::{Parser, Subcommand};
use std::process::exit;
use sc2_proto::common::Race;
use crate::ui::GameType;

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
    CreateGame {
        #[arg(long)]
        mode: Option<String>,
        #[arg(long)]
        race: Option<String>,
    },
}

/// Start the server inside Docker and wait until it's reachable.
fn start_server_container() -> Result<(), String> {
    let image = std::env::var("SC2_SERVER_IMAGE").unwrap_or_else(|_| "sc2:latest".into());
    let container_name = std::env::var("SC2_SERVER_CONTAINER").unwrap_or_else(|_| "sc2-tweak".into());
    let port: u16 = 5555;
    let host = "127.0.0.1";

    // Remove any existing container with the same name
    let _ = Command::new("docker")
        .args(["rm", "-f", &container_name])
        .status();

    // // Pull image
    // let _ = Command::new("docker")
    //     .args(["pull", &image])
    //     .status();

    // Run container detached, auto-remove on stop, bind to localhost
    let status = Command::new("docker")
        .args([
            "run", "-d", "--rm", "-it",
            "--name", &container_name,
            "-p", "5555:5555",
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

fn test()
{
    // let s = "\u{12}\r\u{10}\u{74}\u{6C}\u{81}\u{C0}\u{FC}\u{FF}\u{FF}\u{FF}\u{FF}\u{01}\u{1A}\0\u{88}\u{06}\0\u{98}\u{06}\u{02}";
    // let bytes = s.as_bytes();
    // let resp = Response::parse_from_bytes(bytes);

    let bytes: Vec<u8> = vec![
        18, 13, 16, 116, 108, 129, 192, 252,
        255, 255, 255, 255, 1, 26, 0,
        136, 6, 0, 152, 6, 2,
    ];
    let resp = Response::parse_from_bytes(&*bytes);
    println!("{:?}", resp);

}


/// Spawns a few colored shapes so you can verify the renderer works.
pub fn spawn_test_entities(mut commands: Commands, asset_server: Res<AssetServer>) {
    // 🟥 Red square
    commands.spawn((
        Sprite::from_color(Color::srgb(1.0, 0.0, 0.0), Vec2::splat(100.0)),
        Transform::from_xyz(-200.0, 0.0, 0.0),
    ));

    // 🟩 Green square
    commands.spawn((
        Sprite::from_color(Color::srgb(0.0, 1.0, 0.0), Vec2::splat(100.0)),
        Transform::from_xyz(0.0, 0.0, 0.0),
    ));

    // 🟦 Blue square
    commands.spawn((
        Sprite::from_color(Color::srgb(0.0, 0.0, 1.0), Vec2::splat(100.0)),
        Transform::from_xyz(200.0, 0.0, 0.0),
    ));

    // Use your own helpers::tiled module
    // let map_handle = helpers::tiled::TiledMapHandle(asset_server.load("iso_map.tmx"));

    // Note: You'll need to verify that TiledMapBundle exists in your helpers::tiled module
    // If it doesn't, you may need to implement it or remove this code
    // commands.spawn(helpers::tiled::TiledMapBundle {
    //     tiled_map: map_handle,
    //     render_settings: TilemapRenderSettings {
    //         render_chunk_size: UVec2::new(3, 1),
    //         y_sort: true,
    //     },
    //     ..Default::default()
    // });

    info!("✅ Spawned test sprites to verify rendering.");
}

/// System to check/start Docker and update status
fn docker_startup_system(
    mut commands: Commands,
    runtime: Res<TokioTasksRuntime>,
    mut docker_status: ResMut<DockerStatus>,
) {
    docker_status.clone_from(&DockerStatus::Starting);
    runtime.spawn_background_task(|mut ctx| async move {
        // Use spawn_blocking for blocking code
        let result = tokio::task::spawn_blocking(start_server_container).await.unwrap_or_else(|_| Err("Thread panicked".to_string()));
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
    commands: Commands,
    runtime: Res<TokioTasksRuntime>,
    game_created: ResMut<GameCreated>,
) {
    if !*has_connected && *docker_status == DockerStatus::Running && game_created.0 {
        setup_proxy(commands, runtime);
        *has_connected = true;
        // game_created.0 = false;
        println!("Proxy connection started after Docker became ready and game was created");
    }
}

/// Entry point
fn main() {
    let cli = Cli::parse();

    // Default values for resources
    let mut game_created = false;
    let mut app_state = AppState::StartScreen;
    let mut game_config_panel = GameConfigPanel::new();

    if let Some(CliCommands::CreateGame { mode, race }) = cli.command {
        // Check required params
        if mode.is_none() || race.is_none() {
            eprintln!("Error: --mode and --race are required for create_game\n");
            eprintln!("Usage: sc2view create_game --mode=<MODE> --race=<RACE>");
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
        // Set up resources to skip start screen
        game_created = true;
        app_state = AppState::GameScreen;
        game_config_panel.game_type = game_type.unwrap();
        game_config_panel.ai_race = Some(race_enum.unwrap());
    }

    let rt = Runtime::new().unwrap();

    App::new()
        .add_plugins(DefaultPlugins.set(ImagePlugin::default_nearest()))
        .add_plugins(TilemapPlugin)
        .add_plugins(EguiPlugin::default())
        .add_plugins(TokioTasksPlugin::default())
        .insert_resource(GameCreated(game_created))
        .insert_resource(UnitRegistry::default())
        .insert_resource(UnitIconAssets::default())
        .insert_resource(SelectedUnit::default())
        .insert_resource(CameraPanState::default())
        .insert_resource(game_config_panel)
        .insert_resource(DockerStatus::Starting)
        .insert_resource(app_state)
        .add_systems(Startup, preload_unit_icons)
        .add_systems(Startup, setup_camera)
        .add_systems(Update, unit_selection_system)
        .add_systems(EguiPrimaryContextPass, selected_unit_panel_system)
        .add_systems(Update, camera_controls)
        .add_systems(Startup, docker_startup_system)
        .add_systems(EguiPrimaryContextPass, ui_system)
        .add_systems(EguiPrimaryContextPass, status_bar_system)
        .add_systems(Update, response_controller_system)
        .add_systems(Update, proxy_connect_on_docker_ready)
        .run();
}
