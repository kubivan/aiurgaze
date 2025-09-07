// src/main.rs
mod proxy;
mod render;

use bevy::prelude::*;
use proxy::{start_proxy, ProxyEvent};
use render::{RenderPlugin, ProxyEventReceiver};

use tokio::runtime::Runtime;
use tokio::sync::mpsc;

/// Entry point
fn main() {
    // Channel for proxy -> renderer communication
    let (tx, rx) = mpsc::unbounded_channel::<ProxyEvent>();

    // Spawn tokio runtime
    let rt = Runtime::new().expect("Failed to create tokio runtime");

    // Spawn the proxy task on the runtime
    rt.spawn(start_proxy(tx, 5000));

    // Start Bevy
    App::new()
        .insert_resource(ProxyEventReceiver { rx })
        .add_plugins(DefaultPlugins)
        .add_plugins(RenderPlugin)
        .run();
}
