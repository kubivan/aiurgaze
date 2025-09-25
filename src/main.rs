
// src/main.rs
mod proxy;
mod render;
mod connection;
mod proxy_ws;

use bevy::prelude::*;
use proxy::{start_proxy, ProxyEvent, ProxyCommand};
use render::{RenderPlugin, ProxyEventReceiver, ProxyCommandSender};

use tokio::runtime::Runtime;
use tokio::sync::mpsc;
use std::process::Command;
use std::net::TcpStream;
use std::time::{Duration, Instant};

use tap::prelude::*;

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

/// Entry point
fn main() {
    // Start dockerized server first so it's ready for Connect
    if let Err(e) = start_server_container() {
        eprintln!("Failed to start server container: {e}");
        // You can choose to exit or continue; exiting is safer:
        std::process::exit(1);
    }

    // Create channels:
    // - commands: UI/View -> Control
    // - events:   Control   -> Bevy/View/Model
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<ProxyCommand>();
    let (evt_tx, evt_rx) = mpsc::unbounded_channel::<ProxyEvent>();

    // Spawn tokio runtime
    let rt = Runtime::new().expect("Failed to create tokio runtime");

    // Spawn the proxy task on the runtime
    rt.spawn(start_proxy(cmd_rx, evt_tx, 5000, 50051));
    // Kick off connection on startup (after proxy is spawned)
    let server_addr = std::env::var("SC2_SERVER_ADDR").unwrap_or_else(|_| "127.0.0.1:50051".into());
    let _ = cmd_tx.send(ProxyCommand::Connect { addr: server_addr });

    // Start Bevy
    App::new()
        .insert_resource(ProxyEventReceiver { rx: evt_rx })
        .insert_resource(ProxyCommandSender { tx: cmd_tx })
        .add_plugins(DefaultPlugins)
        .add_plugins(RenderPlugin)
        .run();
}