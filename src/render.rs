// src/render.rs
use bevy_egui::{EguiContexts, EguiPrimaryContextPass};
use bevy::prelude::*;
use bevy::ui::prelude::*;
use bevy::sprite::*;
use bevy_egui::EguiPlugin;
//use bevy_egui::egui::Style;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

use crate::proxy::{ProxyEvent, ProxyCommand};

/// Bevy component representing a unit from SC2
#[derive(Component)]
pub struct UnitMarker {
    pub tag: u64, // SC2 unit tag
}
/// Resource wrapping a channel receiver for proxy events
#[derive(Resource)]
pub struct ProxyEventReceiver {
    pub rx: UnboundedReceiver<ProxyEvent>,
}

#[derive(Resource, Clone)]
pub struct ProxyCommandSender {
    pub tx: UnboundedSender<ProxyCommand>,
}

/// Our render plugin
pub struct RenderPlugin;

impl Plugin for RenderPlugin {
    fn build(&self, app: &mut App) {
        app
            .insert_resource(ClearColor(Color::linear_rgb(0.05, 0.05, 0.1)))
            .add_plugins(EguiPlugin::default())
            .add_systems(Startup, setup_ui)
            .add_systems(EguiPrimaryContextPass, egui_ui)
            .add_systems(Main, process_proxy_events);
    }
}

// Simple egui panel with a "Create Game" button
fn egui_ui(
    mut contexts: bevy_egui::EguiContexts,
    cmd: Res<ProxyCommandSender>,
) {
    // Obtain the global egui context; in a single-window app this is sufficient
    let ctx = contexts.ctx_mut().unwrap();
    bevy_egui::egui::Window::new("SC2 Control").show(ctx, |ui| {
        if ui.button("Create Game").clicked() {
            // Example command; extend with real parameters as needed
            let _ = cmd.tx.send(ProxyCommand::CreateGame {
                map: "AbyssalReef".to_string(),
                players: vec![], // fill with PlayerConfig later
            });
        }
        if ui.button("Step").clicked() {
            let _ = cmd.tx.send(ProxyCommand::Step { frames: 1 });
        }
    });
}

/// Process incoming events from the proxy
fn process_proxy_events(
    mut commands: Commands,
    mut receiver: ResMut<ProxyEventReceiver>,
    mut query: Query<(Entity, &UnitMarker)>,
) {
    while let Ok(event) = receiver.rx.try_recv() {
        match event {
            ProxyEvent::GameStateDelta(delta) => {
                // TODO: apply delta to ECS. Minimal demo below:
                // Fake: ensure a unit with tag 123 exists for visualization
                let unit_tag = 123u64;

                // Check if unit already exists
                let exists = query.iter().any(|(_, marker)| marker.tag == unit_tag);

                if !exists {
                    // Spawn a 10x10 green sprite using a tinted Sprite
                    commands.spawn((
                        Sprite {
                            color: Color::BLACK,
                            custom_size: Some(Vec2::new(10.0, 10.0)),
                            ..Default::default()
                        },
                        Transform::from_xyz(0.0, 0.0, 0.0),
                        Visibility::default(),
                        UnitMarker { tag: unit_tag },
                    ));
                }
            }
            ProxyEvent::Connected => {
                info!("Connected to SC2 proxy");
            }
            ProxyEvent::Disconnected { reason } => {
                warn!("Disconnected: {}", reason);
            }
            ProxyEvent::GameCreated { map } => {
                info!("Game created on map: {}", map);
            }
            ProxyEvent::Error(msg) => {
                error!("Proxy error: {}", msg);
            }
        }
    }
}

#[derive(Component)]
struct CreateGameButton;

fn setup_ui(mut commands: Commands, _asset_server: Res<AssetServer>) {
    commands.spawn(Camera2d::default());
}
