use bevy::prelude::*;
use sc2_proto::common::Point;
use sc2_proto::debug::{
    Color as ProtoColor, DebugCommand, DebugCommand_oneof_command, DebugDraw,
};
use sc2_proto::sc2api::{Request, Request_oneof_request};

use crate::controller::MapResource;
use crate::proxy_channel::PlayerId;
use crate::render_layers::{LayerRegistry, RenderLayerKind};

#[derive(Debug, Clone)]
pub enum DebugDrawPrimitive {
    Line { start: Vec3, end: Vec3, color: Color },
    Box { min: Vec3, max: Vec3, color: Color },
    Sphere { center: Vec3, radius: f32, color: Color },
}

#[derive(Resource, Default, Clone)]
pub struct DebugDrawOverlay {
    pub enabled: bool,
    pub commands: Vec<DebugDrawPrimitive>,
}

impl DebugDrawOverlay {
    pub fn set_commands(&mut self, commands: Vec<DebugDrawPrimitive>) {
        self.commands = commands;
    }
}

#[derive(Message, Clone)]
pub struct DebugDrawEvent {
    pub player_id: PlayerId,
    pub commands: Vec<DebugDrawPrimitive>,
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
    let x = point.get_x();
    let y = point.get_y();
    let world_x = x * tile_size - (map_size.0 as f32) * tile_size / 2.0;
    let world_y = y * tile_size - (map_size.1 as f32) * tile_size / 2.0;
    Vec2::new(world_x, world_y)
}

pub fn debug_draw_message_system(
    mut events: MessageReader<DebugDrawEvent>,
    mut overlay: ResMut<DebugDrawOverlay>,
) {
    let mut all_commands = Vec::new();
    for event in events.read() {
        all_commands.extend(event.commands.iter().cloned());
    }

    overlay.commands = all_commands;
    overlay.enabled = true;
}

pub fn render_debug_draws(
    mut gizmos: Gizmos,
    overlay: Res<DebugDrawOverlay>,
    layer_registry: Res<LayerRegistry>,
    map: Option<Res<MapResource>>,
    entity_system: Option<Res<crate::entity_system::EntitySystem>>,
) {
    if map.is_none() || entity_system.is_none() || !overlay.enabled || !layer_registry.is_visible(RenderLayerKind::DebugOverlay) {
        return;
    }

    let map = map.unwrap();
    let entity_system = entity_system.unwrap();
    let map_size = map.static_layers.get_dimensions();
    let tile_size = entity_system.map_config.tile_size;

    for item in &overlay.commands {
        match item {
            DebugDrawPrimitive::Line { start, end, color } => {
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
            DebugDrawPrimitive::Sphere { center, radius, color } => {
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

pub fn debug_draw_request_from_sc2(request: &Request) -> Option<DebugDrawEvent> {
    let Some(Request_oneof_request::debug(debug_req)) = request.request.as_ref() else {
        return None;
    };

    for cmd in debug_req.get_debug() {
        if matches!(cmd.command.as_ref(), Some(DebugCommand_oneof_command::draw(_))) {
            return Some(DebugDrawEvent {
                player_id: PlayerId::Player1,
                commands: extract_debug_draw_commands(request),
            });
        }
    }

    None
}

pub fn debug_command_to_event(player_id: PlayerId, command: &DebugCommand) -> Option<DebugDrawEvent> {
    let Some(DebugCommand_oneof_command::draw(draw)) = command.command.as_ref() else {
        return None;
    };

    let commands = extract_draw_items(draw);
    if commands.is_empty() {
        return None;
    }

    Some(DebugDrawEvent {
        player_id,
        commands,
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
    for cmd in debug_req.get_debug() {
        let Some(DebugCommand_oneof_command::draw(draw)) = cmd.command.as_ref() else {
            continue;
        };
        commands.extend(extract_draw_items(draw));
    }

    if commands.is_empty() {
        return None;
    }

    Some(DebugDrawEvent { player_id, commands })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_debug_point_matches_map_space() {
        let point = Point {
            x: Some(2.0),
            y: Some(3.0),
            z: Some(0.0),
            ..Default::default()
        };

        let projected = project_debug_point(&point, (20, 18), 4.0);
        assert_eq!(projected, Vec2::new(2.0 * 4.0 - 20.0 * 4.0 / 2.0, 3.0 * 4.0 - 18.0 * 4.0 / 2.0));
    }
}
