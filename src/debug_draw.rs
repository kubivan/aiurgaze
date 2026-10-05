use bevy::prelude::*;
use bevy_egui::egui;
use sc2_proto::common::Point;
use sc2_proto::debug::{Color as ProtoColor, DebugCommand, DebugCommand_oneof_command, DebugDraw};
use sc2_proto::sc2api::{Request, Request_oneof_request};

use crate::controller::{CurrentFrame, LoopPlaybackMode, LoopPlaybackState, MapResource};
use crate::map::{map_position_2d, map_position_3d};
use crate::proxy_channel::PlayerId;
use crate::render_layers::{LayerRegistry, RenderLayerKind};
use crate::render_view::RenderViewMode;

#[derive(Debug, Clone)]
pub enum DebugDrawPrimitive {
    Line {
        start: Vec3,
        end: Vec3,
        color: Color,
    },
    Box {
        min: Vec3,
        max: Vec3,
        color: Color,
    },
    Sphere {
        center: Vec3,
        radius: f32,
        color: Color,
    },
}

#[derive(Debug, Clone)]
pub enum DebugTextPosition {
    Virtual(Vec2),
    World(Vec3),
}

#[derive(Debug, Clone)]
pub struct DebugTextPrimitive {
    pub text: String,
    pub position: DebugTextPosition,
    pub color: Color,
    pub size: f32,
}

#[derive(Message, Clone)]
pub struct DebugDrawEvent {
    pub player_id: PlayerId,
    pub commands: Vec<DebugDrawPrimitive>,
    pub texts: Vec<DebugTextPrimitive>,
}

pub fn extract_debug_draw_commands(request: &Request) -> Vec<DebugDrawPrimitive> {
    let Some(req) = request.request.as_ref() else {
        return Vec::new();
    };

    let Request_oneof_request::debug(debug_req) = req else {
        return Vec::new();
    };

    let mut items = Vec::new();
    for cmd in debug_req.get_debug() {
        let Some(DebugCommand_oneof_command::draw(draw)) = cmd.command.as_ref() else {
            continue;
        };
        items.extend(extract_draw_items(draw));
    }

    items
}

fn extract_draw_items(draw: &DebugDraw) -> Vec<DebugDrawPrimitive> {
    let mut items = Vec::new();

    for line in draw.get_lines() {
        let Some((start, end)) = line
            .line
            .as_ref()
            .and_then(|line| line.p0.as_ref().zip(line.p1.as_ref()))
        else {
            continue;
        };

        items.push(DebugDrawPrimitive::Line {
            start: project_point(start),
            end: project_point(end),
            color: color_to_bevy(line.get_color()),
        });
    }

    for box_cmd in draw.get_boxes() {
        let Some(min) = box_cmd.min.as_ref() else {
            continue;
        };
        let Some(max) = box_cmd.max.as_ref() else {
            continue;
        };

        items.push(DebugDrawPrimitive::Box {
            min: project_point(min),
            max: project_point(max),
            color: color_to_bevy(box_cmd.get_color()),
        });
    }

    for sphere in draw.get_spheres() {
        let Some(center) = sphere.p.as_ref() else {
            continue;
        };

        items.push(DebugDrawPrimitive::Sphere {
            center: project_point(center),
            radius: sphere.get_r(),
            color: color_to_bevy(sphere.get_color()),
        });
    }

    items
}

fn extract_draw_texts(draw: &DebugDraw) -> Vec<DebugTextPrimitive> {
    draw.get_text()
        .iter()
        .filter_map(|item| {
            let text = item.text.as_ref()?.clone();
            let position = item
                .virtual_pos
                .as_ref()
                .map(|point| DebugTextPosition::Virtual(Vec2::new(point.get_x(), point.get_y())))
                .or_else(|| {
                    item.world_pos
                        .as_ref()
                        .map(|point| DebugTextPosition::World(project_point(point)))
                })?;

            Some(DebugTextPrimitive {
                text,
                position,
                color: item
                    .color
                    .as_ref()
                    .map(color_to_bevy)
                    .unwrap_or(Color::WHITE),
                size: item.size.unwrap_or(8) as f32,
            })
        })
        .collect()
}

fn extract_draw_content(draw: &DebugDraw) -> (Vec<DebugDrawPrimitive>, Vec<DebugTextPrimitive>) {
    (extract_draw_items(draw), extract_draw_texts(draw))
}

fn color_to_bevy(color: &ProtoColor) -> Color {
    Color::srgba(
        (color.get_r() as f32) / 255.0,
        (color.get_g() as f32) / 255.0,
        (color.get_b() as f32) / 255.0,
        1.0,
    )
}

fn project_point(point: &Point) -> Vec3 {
    Vec3::new(point.get_x(), point.get_y(), point.get_z())
}

pub fn project_debug_point(point: &Point, map_size: (u32, u32), tile_size: f32) -> Vec2 {
    map_position_2d(
        point.get_x(),
        point.get_y(),
        (map_size.0 as f32, map_size.1 as f32),
        tile_size,
    )
}

pub fn project_debug_point_3d(point: Vec3, map_size: (u32, u32), tile_size: f32) -> Vec3 {
    map_position_3d(point.x, point.y, point.z, map_size, tile_size)
}

pub fn debug_draw_message_system(
    mut events: MessageReader<DebugDrawEvent>,
    mut playback: ResMut<LoopPlaybackState>,
    mut current_frame: ResMut<CurrentFrame>,
) {
    let mut all_commands = Vec::new();
    let mut all_texts = Vec::new();
    let mut received_event = false;
    for event in events.read() {
        received_event = true;
        all_commands.extend(event.commands.iter().cloned());
        all_texts.extend(event.texts.iter().cloned());
    }

    if received_event && playback.mode == LoopPlaybackMode::Live {
        let live_loop = playback.live_loop;
        if let Some(frame) = playback.history.frame_for_loop_mut(live_loop) {
            frame.debug_draws.clone_from(&all_commands);
            frame.debug_texts.clone_from(&all_texts);
        }
        if let Some(frame) = current_frame
            .frame
            .as_mut()
            .filter(|frame| frame.game_loop == live_loop)
        {
            frame.debug_draws = all_commands;
            frame.debug_texts = all_texts;
        }
    }
}

pub fn render_debug_texts(
    ctx: &egui::Context,
    texts: &[DebugTextPrimitive],
    layer_registry: &LayerRegistry,
    map: Option<&MapResource>,
    entity_system: &crate::entity_system::EntitySystem,
    cameras: &Query<(&Camera, &GlobalTransform)>,
    view_mode: RenderViewMode,
) {
    if !layer_registry.is_visible(RenderLayerKind::DebugOverlay) {
        return;
    }

    let viewport = ctx.viewport_rect();
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new("sc2_debug_text"),
    ));
    let pre_layout_bounds = viewport.expand(512.0);
    let active_camera = cameras.iter().find(|(camera, _)| camera.is_active);

    for item in texts {
        let position = match &item.position {
            DebugTextPosition::Virtual(position) => Some(egui::pos2(
                viewport.left() + position.x * viewport.width(),
                viewport.top() + position.y * viewport.height(),
            )),
            DebugTextPosition::World(position) => {
                let (Some(map), Some((camera, transform))) = (map, active_camera) else {
                    continue;
                };
                let map_size = map.static_layers.get_dimensions();
                let tile_size = entity_system.map_config.tile_size;
                let world_position = if view_mode == RenderViewMode::ThreeD {
                    project_debug_point_3d(*position, map_size, tile_size)
                } else {
                    project_debug_point(
                        &Point {
                            x: Some(position.x),
                            y: Some(position.y),
                            z: Some(position.z),
                            ..Default::default()
                        },
                        map_size,
                        tile_size,
                    )
                    .extend(0.0)
                };
                camera
                    .world_to_viewport(transform, world_position)
                    .ok()
                    .map(|position| {
                        egui::pos2(viewport.left() + position.x, viewport.top() + position.y)
                    })
            }
        };
        let Some(position) = position else {
            continue;
        };
        if !pre_layout_bounds.contains(position) {
            continue;
        }
        let color = item.color.to_srgba().to_u8_array();
        let color = egui::Color32::from_rgb(color[0], color[1], color[2]);
        let galley = painter.layout_no_wrap(
            item.text.clone(),
            egui::FontId::proportional(item.size * ctx.pixels_per_point()),
            color,
        );
        let text_bounds = egui::Rect::from_min_size(position, galley.size());
        if viewport.intersects(text_bounds) {
            painter.galley(position, galley, color);
        }
    }
}

pub fn render_debug_draws_2d(
    gizmos: Gizmos,
    current_frame: Res<CurrentFrame>,
    layer_registry: Res<LayerRegistry>,
    map: Option<Res<MapResource>>,
    entity_system: Option<Res<crate::entity_system::EntitySystem>>,
) {
    render_debug_draws_for_view(
        gizmos,
        current_frame,
        layer_registry,
        map,
        entity_system,
        RenderViewMode::TwoD,
    );
}

pub fn render_debug_draws_3d(
    gizmos: Gizmos,
    current_frame: Res<CurrentFrame>,
    layer_registry: Res<LayerRegistry>,
    map: Option<Res<MapResource>>,
    entity_system: Option<Res<crate::entity_system::EntitySystem>>,
) {
    render_debug_draws_for_view(
        gizmos,
        current_frame,
        layer_registry,
        map,
        entity_system,
        RenderViewMode::ThreeD,
    );
}

fn render_debug_draws_for_view(
    mut gizmos: Gizmos,
    current_frame: Res<CurrentFrame>,
    layer_registry: Res<LayerRegistry>,
    map: Option<Res<MapResource>>,
    entity_system: Option<Res<crate::entity_system::EntitySystem>>,
    view_mode: RenderViewMode,
) {
    if map.is_none()
        || entity_system.is_none()
        || !layer_registry.is_visible(RenderLayerKind::DebugOverlay)
    {
        return;
    }

    let Some(frame) = current_frame.frame.as_ref() else {
        return;
    };

    let map = map.unwrap();
    let entity_system = entity_system.unwrap();
    let map_size = map.static_layers.get_dimensions();
    let tile_size = entity_system.map_config.tile_size;

    for item in &frame.debug_draws {
        match item {
            DebugDrawPrimitive::Line { start, end, color } => {
                if view_mode == RenderViewMode::ThreeD {
                    gizmos.line(
                        project_debug_point_3d(*start, map_size, tile_size),
                        project_debug_point_3d(*end, map_size, tile_size),
                        *color,
                    );
                    continue;
                }
                let p0 = project_debug_point(
                    &Point {
                        x: Some(start.x),
                        y: Some(start.y),
                        z: Some(start.z),
                        ..Default::default()
                    },
                    map_size,
                    tile_size,
                );
                let p1 = project_debug_point(
                    &Point {
                        x: Some(end.x),
                        y: Some(end.y),
                        z: Some(end.z),
                        ..Default::default()
                    },
                    map_size,
                    tile_size,
                );
                gizmos.line_2d(p0, p1, *color);
            }
            DebugDrawPrimitive::Box { min, max, color } => {
                if view_mode == RenderViewMode::ThreeD {
                    draw_debug_box_3d(&mut gizmos, *min, *max, map_size, tile_size, *color);
                    continue;
                }
                let rect_min = project_debug_point(
                    &Point {
                        x: Some(min.x),
                        y: Some(min.y),
                        z: Some(min.z),
                        ..Default::default()
                    },
                    map_size,
                    tile_size,
                );
                let rect_max = project_debug_point(
                    &Point {
                        x: Some(max.x),
                        y: Some(max.y),
                        z: Some(max.z),
                        ..Default::default()
                    },
                    map_size,
                    tile_size,
                );
                let x_min = rect_min.x.min(rect_max.x);
                let x_max = rect_min.x.max(rect_max.x);
                let y_min = rect_min.y.min(rect_max.y);
                let y_max = rect_min.y.max(rect_max.y);
                let p0 = Vec2::new(x_min, y_min);
                let p1 = Vec2::new(x_max, y_min);
                let p2 = Vec2::new(x_max, y_max);
                let p3 = Vec2::new(x_min, y_max);
                gizmos.line_2d(p0, p1, *color);
                gizmos.line_2d(p1, p2, *color);
                gizmos.line_2d(p2, p3, *color);
                gizmos.line_2d(p3, p0, *color);
            }
            DebugDrawPrimitive::Sphere {
                center,
                radius,
                color,
            } => {
                if view_mode == RenderViewMode::ThreeD {
                    let center = project_debug_point_3d(*center, map_size, tile_size);
                    draw_debug_sphere_3d(&mut gizmos, center, *radius * tile_size, *color);
                    continue;
                }
                let center2 = project_debug_point(
                    &Point {
                        x: Some(center.x),
                        y: Some(center.y),
                        z: Some(center.z),
                        ..Default::default()
                    },
                    map_size,
                    tile_size,
                );
                gizmos.circle_2d(center2, *radius * tile_size, *color);
            }
        }
    }
}

fn draw_debug_box_3d(
    gizmos: &mut Gizmos,
    min: Vec3,
    max: Vec3,
    map_size: (u32, u32),
    tile_size: f32,
    color: Color,
) {
    let corners = [
        map_position_3d(min.x, min.y, min.z, map_size, tile_size),
        map_position_3d(max.x, min.y, min.z, map_size, tile_size),
        map_position_3d(min.x, max.y, min.z, map_size, tile_size),
        map_position_3d(max.x, max.y, min.z, map_size, tile_size),
        map_position_3d(min.x, min.y, max.z, map_size, tile_size),
        map_position_3d(max.x, min.y, max.z, map_size, tile_size),
        map_position_3d(min.x, max.y, max.z, map_size, tile_size),
        map_position_3d(max.x, max.y, max.z, map_size, tile_size),
    ];
    for (start, end) in [
        (0, 1),
        (0, 2),
        (1, 3),
        (2, 3),
        (4, 5),
        (4, 6),
        (5, 7),
        (6, 7),
        (0, 4),
        (1, 5),
        (2, 6),
        (3, 7),
    ] {
        gizmos.line(corners[start], corners[end], color);
    }
}

fn draw_debug_sphere_3d(gizmos: &mut Gizmos, center: Vec3, radius: f32, color: Color) {
    const SEGMENTS: usize = 16;
    for plane in 0..3 {
        for index in 0..SEGMENTS {
            let a = index as f32 * std::f32::consts::TAU / SEGMENTS as f32;
            let b = (index + 1) as f32 * std::f32::consts::TAU / SEGMENTS as f32;
            let point = |angle: f32| {
                let (sin, cos) = angle.sin_cos();
                match plane {
                    0 => center + Vec3::new(cos * radius, sin * radius, 0.0),
                    1 => center + Vec3::new(cos * radius, 0.0, sin * radius),
                    _ => center + Vec3::new(0.0, cos * radius, sin * radius),
                }
            };
            gizmos.line(point(a), point(b), color);
        }
    }
}

pub fn debug_draw_request_from_sc2(request: &Request) -> Option<DebugDrawEvent> {
    let Some(Request_oneof_request::debug(debug_req)) = request.request.as_ref() else {
        return None;
    };

    let mut commands = Vec::new();
    let mut texts = Vec::new();
    for cmd in debug_req.get_debug() {
        if let Some(DebugCommand_oneof_command::draw(draw)) = cmd.command.as_ref() {
            let (draw_commands, draw_texts) = extract_draw_content(draw);
            commands.extend(draw_commands);
            texts.extend(draw_texts);
        }
    }
    (!commands.is_empty() || !texts.is_empty()).then_some(DebugDrawEvent {
        player_id: PlayerId::Player1,
        commands,
        texts,
    })
}

pub fn debug_command_to_event(
    player_id: PlayerId,
    command: &DebugCommand,
) -> Option<DebugDrawEvent> {
    let Some(DebugCommand_oneof_command::draw(draw)) = command.command.as_ref() else {
        return None;
    };

    let (commands, texts) = extract_draw_content(draw);
    if commands.is_empty() && texts.is_empty() {
        return None;
    }

    Some(DebugDrawEvent {
        player_id,
        commands,
        texts,
    })
}

pub fn request_contains_debug_draw(request: &Request) -> bool {
    matches!(
        request.request.as_ref(),
        Some(sc2_proto::sc2api::Request_oneof_request::debug(_))
    )
}

pub fn debug_draw_from_request(player_id: PlayerId, raw: &[u8]) -> Option<DebugDrawEvent> {
    let Ok(request) = <Request as protobuf::Message>::parse_from_bytes(raw) else {
        return None;
    };

    let Some(req) = request.request.as_ref() else {
        return None;
    };

    let Request_oneof_request::debug(debug_req) = req else {
        return None;
    };

    let mut commands = Vec::new();
    let mut texts = Vec::new();
    for cmd in debug_req.get_debug() {
        let Some(DebugCommand_oneof_command::draw(draw)) = cmd.command.as_ref() else {
            continue;
        };
        let (draw_commands, draw_texts) = extract_draw_content(draw);
        commands.extend(draw_commands);
        texts.extend(draw_texts);
    }

    if commands.is_empty() && texts.is_empty() {
        return None;
    }

    Some(DebugDrawEvent {
        player_id,
        commands,
        texts,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use protobuf::Message;

    #[test]
    fn project_debug_point_matches_map_space() {
        let point = Point {
            x: Some(2.0),
            y: Some(3.0),
            z: Some(0.0),
            ..Default::default()
        };

        let projected = project_debug_point(&point, (20, 18), 4.0);
        assert_eq!(
            projected,
            Vec2::new(2.0 * 4.0 - 20.0 * 4.0 / 2.0, 3.0 * 4.0 - 18.0 * 4.0 / 2.0)
        );
    }

    #[test]
    fn debug_projection_centers_xy_and_preserves_z() {
        assert_eq!(
            project_debug_point_3d(Vec3::new(2.0, 3.0, 4.0), (10, 8), 2.0),
            Vec3::new(-6.0, 8.0, -2.0)
        );
    }

    #[test]
    fn extracts_virtual_debug_text_without_draw_primitives() {
        let mut request = Request::new();
        let mut command = DebugCommand::new();
        let mut draw = DebugDraw::new();
        let mut text = sc2_proto::debug::DebugText::new();
        text.set_text("hello".to_owned());
        text.set_virtual_pos(Point {
            x: Some(0.25),
            y: Some(0.75),
            ..Default::default()
        });
        draw.mut_text().push(text);
        command.set_draw(draw);
        request.mut_debug().mut_debug().push(command);

        let raw = request.write_to_bytes().unwrap();
        let event = debug_draw_from_request(PlayerId::Player2, &raw).unwrap();

        assert!(event.commands.is_empty());
        assert_eq!(event.texts.len(), 1);
        assert_eq!(event.texts[0].text, "hello");
        assert_eq!(event.texts[0].size, 8.0);
        assert_eq!(event.player_id, PlayerId::Player2);
        assert!(matches!(
            event.texts[0].position,
            DebugTextPosition::Virtual(position) if position == Vec2::new(0.25, 0.75)
        ));
    }
}
