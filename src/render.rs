// src/render.rs
use bevy::prelude::*;
use bevy::ui::prelude::*;
use bevy::sprite::*;
//use bevy_egui::egui::Style;
use tokio::sync::mpsc::UnboundedReceiver;

use crate::proxy::ProxyEvent;

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

/// Our render plugin
pub struct RenderPlugin;

impl Plugin for RenderPlugin {
    fn build(&self, app: &mut App) {
        app
            .insert_resource(ClearColor(Color::linear_rgb(0.05, 0.05, 0.1)))
            .add_systems(Startup, setup_ui)
            .add_systems(Main, process_proxy_events);
    }
}

/// Process incoming events from the proxy
fn process_proxy_events(
    mut commands: Commands,
    mut receiver: ResMut<ProxyEventReceiver>,
    mut query: Query<(Entity, &UnitMarker)>,
) {
    while let Ok(event) = receiver.rx.try_recv() {
        match event {
            ProxyEvent::GameState(response) => {
                // TODO: parse response.observation
                // Here we just fake one unit for now
                let unit_tag = 123u64;

                // Check if unit already exists
                let exists = query.iter().any(|(_, marker)| marker.tag == unit_tag);

                if !exists {
                    commands.spawn((Sprite {
                        image: Default::default(),
                        texture_atlas: None,
                        color: Color::WHITE,
                        flip_x: false,
                        flip_y: false,
                                custom_size: Some(Vec2::new(10.0, 10.0)),
                        rect: None,
                        anchor: Default::default(),
                        image_mode: Default::default(),
                    })

                        // SpriteBundle {
                        //     sprite: Sprite {
                        //         color: Color::GREEN,
                        //         custom_size: Some(Vec2::new(10.0, 10.0)),
                        //         ..default()
                        //     },
                        //     transform: Transform::from_xyz(0.0, 0.0, 0.0),
                        //     ..default()
                        // },
                        // UnitMarker { tag: unit_tag },
                    );
                }
            }
            ProxyEvent::BotStep(_) => {
                // You can visualize bot actions here
            }
        }
    }
}

#[derive(Component)]
struct CreateGameButton;

fn setup_ui(mut commands: Commands, asset_server: Res<AssetServer>) {
    commands.spawn(Camera2d::default());

    // Full-screen UI root, centered content
    commands
        .spawn((
            Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..Default::default()
            },
            BackgroundColor(Color::NONE),
        ))
        .with_children(|parent| {
            // Button with label "Create Game"
            parent
                .spawn((
                    Button,
                    CreateGameButton,
                    BackgroundColor(Color::srgb(0.15, 0.45, 0.85)),
                    BorderRadius::all(Val::Px(6.0)),
                    //Padding::all(Val::Px(12.0)),
                ))
                .with_children(|btn| {
                    btn.spawn((
                        Text::new("Create Game"),
                        TextColor(Color::WHITE),
                        TextFont {
                            // Uses default font; replace with a custom asset if desired:
                            // font: asset_server.load("fonts/FiraSans-Bold.ttf"),
                            font_size: 18.0,
                            ..Default::default()
                        },
                    ));
                });
        });
}
