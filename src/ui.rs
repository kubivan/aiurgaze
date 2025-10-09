use bevy::input::mouse::{MouseMotion, MouseWheel};
use bevy::prelude::*;
use bevy_egui::{egui, EguiPlugin, EguiContexts, EguiPrimaryContextPass};
use crate::controller::response_controller_system;
use crate::units::{SelectedUnit, UnitRegistry, UnitType, UnitHealth, UnitTag};

#[derive(Resource, PartialEq, Eq, Hash, Clone, Debug)]
pub enum AppState {
    StartScreen,
    GameScreen,
}

// pub fn setup_camera_system(mut commands: Commands) {
//     commands.spawn(Camera2d);
//     // commands.spawn(Camera2dBundle::default());
//
// }
#[derive(Component)]
pub struct MainCamera;

// pub fn setup_camera_system(mut commands: Commands) {
//     commands.spawn((
//         Camera2d {
//             //transform: Transform::from_xyz(0.0, 0.0, 999.9),
//             //Projection::default_2d(),
//             ..default()
//         },
//         MainCamera,
//     ));
// }
pub fn setup_camera(mut commands: Commands) {
    commands.spawn((
        Camera2d,
        Transform::from_xyz(0.0, 0.0, 1000.0),
    ));
}

pub fn camera_pan_system(
    mut query: Query<&mut Transform, With<MainCamera>>,
    mouse_input: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    mut mouse_motion_events: EventReader<MouseMotion>,
    time: Res<Time>,
) {
    let mut camera_transform = query.single_mut().unwrap();

    // === Keyboard pan (WASD) ===
    let mut direction = Vec2::ZERO;
    let speed = 500.0 * time.delta_secs();

    if keys.pressed(KeyCode::KeyW) {
        direction.y += 1.0;
    }
    if keys.pressed(KeyCode::KeyS) {
        direction.y -= 1.0;
    }
    if keys.pressed(KeyCode::KeyA) {
        direction.x -= 1.0;
    }
    if keys.pressed(KeyCode::KeyD) {
        direction.x += 1.0;
    }

    camera_transform.translation.x += direction.x * speed;
    camera_transform.translation.y += direction.y * speed;

    // === Mouse drag pan ===
    if mouse_input.pressed(MouseButton::Right) {
        for event in mouse_motion_events.read() {
            camera_transform.translation.x -= event.delta.x;
            camera_transform.translation.y += event.delta.y;
        }
    }
}

#[derive(Resource, Default)]
pub struct CameraPanState {
    dragging: bool,
}

pub fn camera_controls(
    mut state: ResMut<CameraPanState>,
    buttons: Res<ButtonInput<MouseButton>>,
    mut motion_evr: EventReader<MouseMotion>,
    mut scroll_evr: EventReader<MouseWheel>,
    mut q_camera: Query<(&mut Transform, &mut Projection), With<Camera2d>>,
) {
    if let Ok((mut transform, mut projection)) = q_camera.get_single_mut() {
        // 🖱️ Middle mouse drag to pan
        if buttons.just_pressed(MouseButton::Middle) {
            state.dragging = true;
        }
        if buttons.just_released(MouseButton::Middle) {
            state.dragging = false;
        }

        if state.dragging {
            for ev in motion_evr.read() {
                transform.translation.x -= ev.delta.x;
                transform.translation.y += ev.delta.y;
            }
        }

        // 🔍 Scroll to zoom
        for ev in scroll_evr.read() {
            if let Projection::Orthographic(ref mut ortho) = *projection {
                ortho.scale = (ortho.scale * (1.0 - ev.y * 0.1)).clamp(0.1, 10.0);
            }
        }
    }
}



pub fn ui_system(mut contexts: EguiContexts, mut app_state: ResMut<AppState>) {
    // println!("ui_system context + ");
    let ctx = match contexts.ctx_mut() {
        Ok(ctx) => ctx,
        _ => return,
    };
    // println!("ui_system context -- ");
    // let ctx = contexts.ctx_mut()?;

    match *app_state {
        AppState::StartScreen => {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.heading("SC2 Proxy");
                ui.separator();
                ui.label("Start Screen label");

                ui.add_space(10.0);
                if ui.button("Start Game").clicked() {
                    *app_state = AppState::GameScreen;
                }
            });

        }
        AppState::GameScreen => {
            egui::TopBottomPanel::top("top_panel").show(ctx, |ui| {
                ui.label("Top Panel");
            });

            egui::SidePanel::left("map_panel")
                .resizable(true)
                .default_width(500.0)
                .show(ctx, |ui| {
                    ui.heading("Map Panel");
                    ui.label("(RTS map rendering goes here)");
                });

            egui::SidePanel::right("debug_panel")
                .resizable(true)
                .default_width(250.0)
                .show(ctx, |ui| {
                    ui.heading("Debug Info");
                    ui.label("FPS, unit counts, AI state, etc.");
                });

            // egui::CentralPanel::default().show(ctx, |ui| {
            //     ui.heading("Game Controls");
            //     ui.label("Middle area or mini-console can go here.");
            // });

        }
    };
}

pub fn selected_unit_panel_system(
    mut contexts: EguiContexts,
    selected: Res<SelectedUnit>,
    registry: Res<UnitRegistry>,
    unit_query: Query<(&UnitType, &UnitHealth, &Transform, &UnitTag)>,
) {
    if let Ok(ctx) = contexts.ctx_mut() {
        egui::SidePanel::right("unit_info_panel").show(ctx, |ui| {
            ui.heading("Selected Unit Info");
            if let Some(tag) = selected.tag {
                if let Some(&entity) = registry.map.get(&tag) {
                    if let Ok((unit_type, health, transform, unit_tag)) = unit_query.get(entity) {
                        ui.label(format!("Tag: {}", unit_tag.0));
                        ui.label(format!("Type: {}", unit_type.0));
                        ui.label(format!("Health: {:.1}", health.0));
                        ui.label(format!("Position: ({:.1}, {:.1})", transform.translation.x, transform.translation.y));
                    } else {
                        ui.label("Unit data not found.");
                    }
                } else {
                    ui.label("No unit selected.");
                }
            } else {
                ui.label("No unit selected.");
            }
        });
    }
}
