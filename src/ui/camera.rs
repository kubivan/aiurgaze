use bevy::input::mouse::{MouseMotion, MouseWheel};
use bevy::prelude::*;

pub fn setup_camera(mut commands: Commands) {
    commands.spawn((Camera2d, Transform::from_xyz(0.0, 0.0, 1000.0)));
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
    if let Ok((mut transform, mut projection)) = q_camera.single_mut() {
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

        for ev in scroll_evr.read() {
            if let Projection::Orthographic(ref mut ortho) = *projection {
                ortho.scale = (ortho.scale * (1.0 - ev.y * 0.1)).clamp(0.1, 10.0);
            }
        }
    }
}
