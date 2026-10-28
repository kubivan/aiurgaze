use crate::frame::{Frame, FrameSource, PlayerId};
use sc2_proto::sc2api::{Response, Response_oneof_response};

#[derive(Default)]
pub(super) struct ResponseFrameAssembler {
    current: Option<Frame>,
}

impl ResponseFrameAssembler {
    pub(super) fn push(
        &mut self,
        source: FrameSource,
        player_id: PlayerId,
        response: Response,
    ) -> Option<Frame> {
        match response.response? {
            Response_oneof_response::observation(observation) => {
                let game_loop = observation
                    .observation
                    .as_ref()
                    .map(|data| data.get_game_loop())?;
                let terminal = observation
                    .player_result
                    .iter()
                    .any(|result| result.has_result());
                let frame = Frame {
                    source,
                    player_id,
                    game_loop,
                    observation,
                    debug_draws: Vec::new(),
                    debug_texts: Vec::new(),
                };
                self.current = Some(frame);

                if terminal {
                    self.current.take()
                } else {
                    None
                }
            }
            Response_oneof_response::step(_) => self.current.take(),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sc2_proto::sc2api::{PlayerResult, ResponseObservation, Result as GameResult};

    #[test]
    fn observation_storage_is_moved_into_frame() {
        let mut assembler = ResponseFrameAssembler::default();
        let mut observation = ResponseObservation::new();
        observation.mut_observation().set_game_loop(7);
        let mut unit = sc2_proto::raw::Unit::new();
        unit.set_tag(123);
        observation
            .mut_observation()
            .mut_raw_data()
            .mut_units()
            .push(unit);
        let units_ptr = observation
            .get_observation()
            .get_raw_data()
            .get_units()
            .as_ptr();
        let mut response = Response::new();
        response.set_observation(observation);
        assert!(assembler
            .push(FrameSource::Replay, PlayerId::Player2, response)
            .is_none());
        let mut step = Response::new();
        step.set_step(sc2_proto::sc2api::ResponseStep::new());
        let frame = assembler
            .push(FrameSource::Replay, PlayerId::Player2, step)
            .unwrap();
        assert_eq!(
            frame.observation.get_observation().get_raw_data().get_units().as_ptr(),
            units_ptr
        );
        assert_eq!(
            frame.observation.get_observation().get_raw_data().get_units()[0].get_tag(),
            123
        );
    }

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
            .push(FrameSource::Game, PlayerId::Player1, response)
            .expect("terminal observation should complete its frame");

        assert_eq!(frame.game_loop, 42);
        assert!(frame.observation.player_result[0].has_result());
        assert!(assembler.current.is_none());
    }
}
