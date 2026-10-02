use crate::render_view::RenderViewMode;
use bevy::ecs::message::MessageReader;
use bevy::input::mouse::{MouseMotion, MouseWheel};
use bevy::prelude::*;

pub fn setup_camera(mut commands: Commands) {
    commands.spawn((
        Camera2d,
        RenderCameraFor(RenderViewMode::TwoD),
        Transform::from_xyz(0.0, 0.0, 1000.0),
    ));
    let mut orthographic = OrthographicProjection::default_3d();
    orthographic.scale = 3.0;
    orthographic.far = 10_000.0;
    commands.spawn((
        Camera3d::default(),
        Camera {
            is_active: false,
            ..default()
        },
        Projection::Orthographic(orthographic),
        RenderCameraFor(RenderViewMode::ThreeD),
        Transform::from_xyz(2_400.0, 2_400.0, 2_400.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    commands.spawn((
        DirectionalLight {
            illuminance: 9000.0,
            shadow_maps_enabled: false,
            ..default()
        },
        Transform::from_xyz(500.0, 800.0, 300.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
}

#[derive(Component)]
pub struct RenderCameraFor(pub RenderViewMode);

pub fn switch_render_camera(
    view_mode: Res<RenderViewMode>,
    mut cameras: Query<(&mut Camera, &RenderCameraFor)>,
) {
    if !view_mode.is_changed() {
        return;
    }

    for (mut camera, marker) in &mut cameras {
        camera.is_active = marker.0 == *view_mode;
    }
}

#[derive(Resource, Default)]
pub struct CameraPanState {
    dragging: bool,
}

pub fn camera_controls_2d(
    mut state: ResMut<CameraPanState>,
    buttons: Res<ButtonInput<MouseButton>>,
    mut motion_evr: MessageReader<MouseMotion>,
    mut scroll_evr: MessageReader<MouseWheel>,
    mut q_camera: Query<(&mut Transform, &mut Projection), With<Camera2d>>,
) {
    for (mut transform, mut projection) in &mut q_camera {
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

pub fn camera_controls_3d(
    mut state: ResMut<CameraPanState>,
    buttons: Res<ButtonInput<MouseButton>>,
    mut motion_evr: MessageReader<MouseMotion>,
    mut scroll_evr: MessageReader<MouseWheel>,
    mut q_camera: Query<(&mut Transform, &mut Projection), With<Camera3d>>,
) {
    for (mut transform, mut projection) in &mut q_camera {
        if buttons.just_pressed(MouseButton::Middle) {
            state.dragging = true;
        }
        if buttons.just_released(MouseButton::Middle) {
            state.dragging = false;
        }

        if state.dragging {
            for ev in motion_evr.read() {
                let right = (transform.rotation * Vec3::X)
                    .with_y(0.0)
                    .normalize_or_zero();
                let up = (transform.rotation * Vec3::Y)
                    .with_y(0.0)
                    .normalize_or_zero();
                transform.translation +=
                    (right * -ev.delta.x + up * ev.delta.y) * projection_scale(&projection);
            }
        }

        for ev in scroll_evr.read() {
            if let Projection::Orthographic(ref mut ortho) = *projection {
                ortho.scale = (ortho.scale * (1.0 - ev.y * 0.1)).clamp(0.5, 10.0);
            }
        }
    }
}

fn projection_scale(projection: &Projection) -> f32 {
    match projection {
        Projection::Orthographic(orthographic) => orthographic.scale,
        _ => 1.0,
    }
}
