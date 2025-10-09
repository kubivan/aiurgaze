// src/main.rs
mod proxy;
mod render;
mod connection;
mod proxy_ws;
mod ui;
mod controller;
mod map;
mod helpers;
mod units;

use bevy::prelude::*;
use proxy::{start_proxy, ProxyEvent, ProxyCommand};
use render::{RenderPlugin, ProxyEventReceiver, ProxyCommandSender};

use tokio::runtime::Runtime;
use tokio::sync::mpsc;
use std::process::Command;
use std::net::TcpStream;
use std::time::{Duration, Instant};
use bevy_ecs_tilemap::{ TilemapPlugin};
use bevy_ecs_tilemap::prelude::TilemapRenderSettings;
use bevy_egui::{EguiPlugin, EguiPrimaryContextPass};
use bevy_tokio_tasks::TokioTasksPlugin;
use protobuf::Message;
use sc2_proto::sc2api::Response;
use tap::prelude::*;
use crate::controller::{response_controller_system, setup_proxy};
use crate::ui::{camera_controls, camera_pan_system, setup_camera, ui_system, AppState, CameraPanState};
use crate::units::{UnitRegistry, UnitIconAssets, preload_unit_icons};

/// Start the server inside Docker and wait until it's reachable.
/// Customize image/name/ports via env vars if needed.
fn start_server_container() -> Result<(), String> {
    let image = std::env::var("SC2_SERVER_IMAGE").unwrap_or_else(|_| "stephanzlatarev/starcraft:latest".into());
    let container_name = std::env::var("SC2_SERVER_CONTAINER").unwrap_or_else(|_| "starcraft".into());
    let host = std::env::var("SC2_SERVER_HOST").unwrap_or_else(|_| "127.0.0.1".into());
    let port: u16 = std::env::var("SC2_SERVER_PORT").ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(5000);

    // Best-effort cleanup of any stale container with same name
    let _ = Command::new("docker")
        .args(["rm", "-f", &container_name])
        .tap(|run| println!("Running: {:?}", run)).status().ok();

    let _ = Command::new("docker")
        .args(["pull", &image])
        .tap(|run| println!("Running: {:?}", run)).status().ok();

    // Run container detached, auto-remove on stop, bind to localhost
    let status = Command::new("docker")
        .args([
            "run", "-d", "--rm",
            "--name", &container_name,
            //"-p", &format!("{host}:{port}:{port}"),
            "-p", "5000:5000", "-p", "5001:5001",
            &image,
        ])
        .tap(|run| println!("Running: {:?}", run))
        .status()
        .map_err(|e| format!("Failed to execute docker run: {e}"))?;

    if !status.success() {
        return Err(format!("docker run failed with status: {status}"));
    }

    // Wait for port to become available
    let addr = format!("{host}:{port}");
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        if TcpStream::connect((&*host, port)).is_ok() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    std::thread::sleep(Duration::from_millis(10000));
    Err(format!("Timed out waiting for server at {addr}"))
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

/// Entry point
fn main() {
    test();
    // Start dockerized server first so it's ready for Connect
    // if let Err(e) = start_server_container() {
    //     eprintln!("Failed to start server container: {e}");
    //     // You can choose to exit or continue; exiting is safer:
    //     std::process::exit(1);
    // }


    // // Create channels:
    // // - commands: UI/View -> Control
    // // - events:   Control   -> Bevy/View/Model
    // let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<ProxyCommand>();
    // let (evt_tx, evt_rx) = mpsc::unbounded_channel::<ProxyEvent>();
    //
    // // Spawn tokio runtime
    // let rt = Runtime::new().expect("Failed to create tokio runtime");
    //
    // // Spawn the proxy task on the runtime
    // rt.spawn(start_proxy(cmd_rx, evt_tx, 5000, 50051));
    // // Kick off connection on startup (after proxy is spawned)
    // let server_addr = std::env::var("SC2_SERVER_ADDR").unwrap_or_else(|_| "127.0.0.1:50051".into());
    // let _ = cmd_tx.send(ProxyCommand::Connect { addr: server_addr });

    let rt = Runtime::new().unwrap();


    App::new()
        //.add_plugins(DefaultPlugins)
        .add_plugins(DefaultPlugins.set(ImagePlugin::default_nearest()))
        .add_plugins(TilemapPlugin)
        // .add_plugins((TilemapPlugin, helpers::tiled::TiledMapPlugin))

        .add_plugins(EguiPlugin::default())
        .add_plugins(TokioTasksPlugin::default())
        .insert_resource(AppState::StartScreen)
        .insert_resource(UnitRegistry::default())
        .insert_resource(UnitIconAssets::default())
        .add_startup_system(preload_unit_icons)
        .add_systems(Startup, setup_camera)
        .insert_resource(CameraPanState::default())
        .add_systems(Update, camera_controls)
        // .add_systems(Startup, spawn_test_entities)
        // .add_systems(Update, camera_pan_system)
        .add_systems(Startup, setup_proxy)
        .add_systems(EguiPrimaryContextPass, ui_system)
        .add_systems(Update, response_controller_system)
        .run();
}