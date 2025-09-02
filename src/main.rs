use bevy::input::mouse::{MouseMotion, MouseWheel};
use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiPlugin, egui};


const CELL_SIZE: f32 = 8.0; // pixel size of each cell

#[derive(Resource, PartialEq, Eq, Clone, Copy, Debug)]
enum Layer {
    Pathing,
    Placement,
    Terrain,
}

#[derive(Resource)]
struct MapData {
    width: usize,
    height: usize,
    pathing: Vec<u8>,   // 0/1 values
    placement: Vec<u8>, // 0/1 values
    terrain: Vec<u8>,   // grayscale 0..255
}

#[derive(Resource)]
struct UiState {
    selected_layer: Layer,
}

fn main() {
    App::new()
        .add_plugins((DefaultPlugins, EguiPlugin))
        .insert_resource(MapData::mock())
        .insert_resource(UiState { selected_layer: Layer::Pathing })
        .add_systems(Startup, setup_camera)
        .add_systems(Update, (draw_map, ui_layer_selector, pan_zoom_camera))
        .run();
}

fn setup_camera(mut commands: Commands) {
    commands.spawn(Camera2dBundle::default());
}

/// Draws the map grid for the selected layer
fn draw_map(
    mut commands: Commands,
    map: Res<MapData>,
    ui: Res<UiState>,
    mut q_clear: Query<Entity, With<Sprite>>,
) {
    // clear previous sprites
    for e in q_clear.iter_mut() {
        commands.entity(e).despawn();
    }

    let (w, h) = (map.width, map.height);

    let data = match ui.selected_layer {
        Layer::Pathing => &map.pathing,
        Layer::Placement => &map.placement,
        Layer::Terrain => &map.terrain,
    };

    for y in 0..h {
        for x in 0..w {
            let idx = y * w + x;
            let val = data[idx];

            // choose color depending on layer type
            let color = match ui.selected_layer {
                Layer::Terrain => {
                    let g = val as f32 / 255.0;
                    Color::rgb(g, g, g)
                }
                _ => {
                    if val == 0 { Color::WHITE } else { Color::BLACK }
                }
            };

            commands.spawn(SpriteBundle {
                sprite: Sprite {
                    color,
                    custom_size: Some(Vec2::splat(CELL_SIZE)),
                    ..default()
                },
                transform: Transform::from_translation(Vec3::new(
                    x as f32 * CELL_SIZE,
                    -(y as f32) * CELL_SIZE,
                    0.0,
                )),
                ..default()
            });
        }
    }
}

/// UI dropdown for selecting layer
fn ui_layer_selector(mut contexts: EguiContexts, mut ui: ResMut<UiState>) {
    egui::Window::new("Layers").show(contexts.ctx_mut(), |ui_egui| {
        egui::ComboBox::from_label("Select Layer")
            .selected_text(format!("{:?}", ui.selected_layer))
            .show_ui(ui_egui, |cb| {
                cb.selectable_value(&mut ui.selected_layer, Layer::Pathing, "Pathing");
                cb.selectable_value(&mut ui.selected_layer, Layer::Placement, "Placement");
                cb.selectable_value(&mut ui.selected_layer, Layer::Terrain, "Terrain");
            });
    });
}

/// Pan & zoom camera with mouse
fn pan_zoom_camera(
    mut q_cam: Query<&mut Transform, With<Camera>>,
    mut scroll: EventReader<MouseWheel>,
    buttons: Res<ButtonInput<MouseButton>>,
    mut motion: EventReader<MouseMotion>,
) {
    let mut cam = q_cam.single_mut();

    // zoom
    for ev in scroll.read() {
        let scale = 1.0 + ev.y * 0.1;
        cam.scale *= Vec3::splat(scale.clamp(0.1, 10.0));
    }

    // pan with middle mouse
    if buttons.pressed(MouseButton::Middle) {
        for ev in motion.read() {
            cam.translation.x -= ev.delta.x;
            cam.translation.y += ev.delta.y;
        }
    }
}

impl MapData {
    fn mock() -> Self {
        let w = 32;
        let h = 32;
        let mut pathing = vec![0; w * h];
        let mut placement = vec![0; w * h];
        let mut terrain = vec![0; w * h];

        for y in 0..h {
            for x in 0..w {
                let idx = y * w + x;
                pathing[idx] = if (x + y) % 2 == 0 { 1 } else { 0 };
                placement[idx] = if (x * y) % 7 == 0 { 1 } else { 0 };
                terrain[idx] = ((x ^ y) % 256) as u8;
            }
        }

        Self { width: w, height: h, pathing, placement, terrain }
    }
}
