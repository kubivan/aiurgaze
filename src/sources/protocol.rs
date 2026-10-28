use crate::frame::{
    Alliance, Frame, FrameSource, MapSnapshot, OrderTarget, PlayerId, PlayerSnapshot, Position,
    RasterLayer, StaticMap, UnitOrder, UnitSnapshot,
};
use protobuf::{reflect::ReflectFieldRef, Message};
use sc2_proto::{
    common::ImageData,
    raw::{Unit as ProtoUnit, UnitOrder_oneof_target},
    sc2api::Response,
};

#[derive(Default)]
pub(super) struct ResponseFrameAssembler {
    current: Option<Frame>,
    static_map: Option<StaticMap>,
}

impl ResponseFrameAssembler {
    pub(super) fn push(
        &mut self,
        source: FrameSource,
        player_id: PlayerId,
        response: &Response,
    ) -> Option<Frame> {
        match response.response.as_ref()? {
            sc2_proto::sc2api::Response_oneof_response::game_info(game_info) => {
                if let Some(start_raw) = game_info.start_raw.as_ref() {
                    if let (Some(pathing), Some(placement), Some(height)) = (
                        start_raw.pathing_grid.as_ref(),
                        start_raw.placement_grid.as_ref(),
                        start_raw.terrain_height.as_ref(),
                    ) {
                        self.static_map = Some(StaticMap {
                            pathing: raster_layer(pathing)?,
                            placement: raster_layer(placement)?,
                            height: raster_layer(height)?,
                        });
                    }
                }
                None
            }
            sc2_proto::sc2api::Response_oneof_response::observation(observation) => {
                let data = observation.observation.as_ref()?;
                let frame = self
                    .current
                    .get_or_insert_with(|| Frame::new(source, player_id, data.get_game_loop()));
                frame.player_id = player_id;
                frame.game_loop = data.get_game_loop();
                frame.units = data
                    .raw_data
                    .as_ref()
                    .map(|raw| raw.units.iter().map(unit_snapshot).collect())
                    .unwrap_or_default();
                frame.map = data
                    .raw_data
                    .as_ref()
                    .and_then(|raw| raw.map_state.as_ref())
                    .map(|map| MapSnapshot {
                        visibility: map.visibility.as_ref().and_then(raster_layer),
                        creep: map.creep.as_ref().and_then(raster_layer),
                        energy: None,
                    });
                frame.player = data.player_common.as_ref().map(|common| PlayerSnapshot {
                    minerals: common.get_minerals(),
                    vespene: common.get_vespene(),
                    food_used: common.get_food_used(),
                    food_cap: common.get_food_cap(),
                    army_count: common.get_army_count(),
                    worker_count: common.get_food_workers(),
                    idle_workers: common.get_idle_worker_count(),
                });
                frame.player_results = observation
                    .player_result
                    .iter()
                    .map(|result| {
                        let outcome = result
                            .result
                            .map(|result| format!("{result:?}"))
                            .unwrap_or_else(|| "Unknown".to_owned());
                        format!("Player {}: {outcome}", result.get_player_id())
                    })
                    .collect();
                if observation
                    .player_result
                    .iter()
                    .any(|result| result.has_result())
                {
                    let mut frame = self.current.take()?;
                    // frame.static_map = self.static_map.clone();
                    Some(frame)
                } else {
                    None
                }
            }
            sc2_proto::sc2api::Response_oneof_response::step(_) => {
                let mut frame = self.current.take()?;
                // frame.static_map = self.static_map.clone();
                Some(frame)
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sc2_proto::sc2api::{PlayerResult, ResponseObservation, Result as GameResult};

    #[test]
    fn terminal_observation_completes_frame_without_step_response() {
        let mut assembler = ResponseFrameAssembler::default();
        let mut observation = ResponseObservation::new();
        observation.mut_observation().set_game_loop(42);

        let mut player_result = PlayerResult::new();
        player_result.set_player_id(1);
        player_result.set_result(GameResult::Victory);
        observation.player_result.push(player_result);

        let mut response = Response::new();
        response.set_observation(observation);

        let frame = assembler
            .push(FrameSource::Game, PlayerId::Player1, &response)
            .expect("terminal observation should complete its frame");

        assert_eq!(frame.game_loop, 42);
        assert_eq!(frame.player_results, ["Player 1: Victory"]);
        assert!(assembler.current.is_none());
    }
}

pub(super) fn static_map_from_info(
    game_info: &sc2_proto::sc2api::ResponseGameInfo,
) -> Option<StaticMap> {
    let raw = game_info.start_raw.as_ref()?;
    Some(StaticMap {
        pathing: raster_layer(raw.pathing_grid.as_ref()?)?,
        placement: raster_layer(raw.placement_grid.as_ref()?)?,
        height: raster_layer(raw.terrain_height.as_ref()?)?,
    })
}

fn raster_layer(image: &ImageData) -> Option<RasterLayer> {
    let size = image.size.as_ref()?;
    let width = size.x? as u32;
    let height = size.y? as u32;
    let bits = image.bits_per_pixel.unwrap_or(8);
    let source = image.data.as_ref()?;
    let data = match bits {
        1 => {
            let mut pixels = Vec::with_capacity((width * height) as usize);
            for byte in source {
                for bit in (0..8).rev() {
                    pixels.push(if byte & (1 << bit) != 0 { 255 } else { 0 });
                    if pixels.len() == (width * height) as usize {
                        break;
                    }
                }
                if pixels.len() == (width * height) as usize {
                    break;
                }
            }
            pixels
        }
        8 => source.clone(),
        _ => return None,
    };
    (data.len() == (width * height) as usize).then_some(RasterLayer {
        width,
        height,
        data,
    })
}

fn unit_snapshot(unit: &ProtoUnit) -> UnitSnapshot {
    UnitSnapshot {
        tag: unit.tag.unwrap_or_default(),
        unit_type: unit.unit_type.unwrap_or_default(),
        alliance: match unit.alliance.unwrap_or_default() as i32 {
            1 => Alliance::Self_,
            2 => Alliance::Ally,
            3 => Alliance::Neutral,
            4 => Alliance::Enemy,
            value => Alliance::Unknown(value),
        },
        position: unit.pos.as_ref().map_or_else(Position::default, |pos| Position {
            x: pos.x.unwrap_or_default(),
            y: pos.y.unwrap_or_default(),
            z: pos.z.unwrap_or_default(),
        }),
        health: unit.health.unwrap_or(1.0),
        health_max: unit.health_max.unwrap_or(1.0),
        shield: unit.shield.unwrap_or_default(),
        shield_max: unit.shield_max.unwrap_or_default(),
        build_progress: unit.build_progress.unwrap_or_default(),
        radius: unit.radius.unwrap_or(1.0),
        orders: unit
            .orders
            .iter()
            .map(|order| UnitOrder {
                ability_id: order.ability_id,
                target: order.target.as_ref().map(|target| match target {
                    UnitOrder_oneof_target::target_world_space_pos(pos) => {
                        OrderTarget::Position(Position {
                            x: pos.x.unwrap_or_default(),
                            y: pos.y.unwrap_or_default(),
                            z: pos.z.unwrap_or_default(),
                        })
                    }
                    UnitOrder_oneof_target::target_unit_tag(tag) => OrderTarget::Unit(*tag),
                }),
            })
            .collect(),
        display_fields: unit_display_fields(unit),
    }
}

fn unit_display_fields(unit: &ProtoUnit) -> Vec<(String, String)> {
    let descriptor = unit.descriptor();
    let mut result = Vec::new();
    for field in descriptor.fields() {
        match field.get_reflect(unit) {
            ReflectFieldRef::Optional(value) if field.has_field(unit) => {
                if let Some(value) = value {
                    result.push((field.name().to_owned(), format!("{value:?}")));
                }
            }
            ReflectFieldRef::Repeated(values) if values.len() > 0 => {
                let values = (0..values.len())
                    .map(|index| format!("{:?}", values.get(index).as_ref()))
                    .collect::<Vec<_>>()
                    .join(", ");
                result.push((field.name().to_owned(), format!("[{values}]")));
            }
            _ => {}
        }
    }
    result
}
