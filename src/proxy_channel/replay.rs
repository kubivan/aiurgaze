use super::{PlayerId, TaggedResponse};
use protobuf::Message;
use sc2_proto::sc2api::{Response, ResponseGameInfo, ResponseObservation};
use std::collections::VecDeque;
use tokio::sync::broadcast;

/// Single snapshot for a given SC2 loop step.
#[derive(Debug, Clone)]
pub struct ReplayFrame {
    pub player_id: PlayerId,
    pub game_loop: u32,
    pub observation: Option<ResponseObservation>,
    pub game_info: Option<ResponseGameInfo>,
    pub debug: Option<Vec<u8>>,
}

/// Accumulates response data until a step response closes the frame.
#[derive(Debug, Clone, Default)]
struct FrameAccumulator {
    pub player_id: Option<PlayerId>,
    pub game_loop: u32,
    pub observation: Option<ResponseObservation>,
    pub game_info: Option<ResponseGameInfo>,
    pub debug: Option<Vec<u8>>,
}

impl FrameAccumulator {
    pub fn update(&mut self, player_id: PlayerId, response: &Response) {
        self.player_id = Some(player_id);
        match response.response.as_ref() {
            Some(sc2_proto::sc2api::Response_oneof_response::observation(observation)) => {
                self.game_loop = observation
                    .observation
                    .as_ref()
                    .map(|inner| inner.get_game_loop())
                    .unwrap_or(self.game_loop);
                self.observation = Some(observation.clone());
            }
            Some(sc2_proto::sc2api::Response_oneof_response::game_info(game_info)) => {
                self.game_info = Some(game_info.clone());
            }
            Some(sc2_proto::sc2api::Response_oneof_response::debug(_)) => {
                self.debug = response.write_to_bytes().ok();
            }
            _ => {}
        }
    }

    pub fn finish(&mut self) -> Option<ReplayFrame> {
        let player_id = self.player_id?;
        if self.observation.is_none() && self.game_info.is_none() && self.debug.is_none() {
            self.reset();
            return None;
        }

        let frame = ReplayFrame::new(
            player_id,
            self.game_loop,
            self.observation.take(),
            self.game_info.take(),
            self.debug.take(),
        );
        self.reset();
        Some(frame)
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

impl ReplayFrame {
    pub fn new(
        player_id: PlayerId,
        game_loop: u32,
        observation: Option<ResponseObservation>,
        game_info: Option<ResponseGameInfo>,
        debug: Option<Vec<u8>>,
    ) -> Self {
        Self {
            player_id,
            game_loop,
            observation,
            game_info,
            debug,
        }
    }

    pub fn from_response(player_id: PlayerId, response: &Response) -> Self {
        let game_loop = match response.response.as_ref() {
            Some(sc2_proto::sc2api::Response_oneof_response::observation(obs)) => obs
                .observation
                .as_ref()
                .map(|inner| inner.get_game_loop())
                .unwrap_or(0),
            Some(sc2_proto::sc2api::Response_oneof_response::game_info(_)) => 0,
            Some(sc2_proto::sc2api::Response_oneof_response::debug(_)) => 0,
            _ => 0,
        };

        let observation = match response.response.as_ref() {
            Some(sc2_proto::sc2api::Response_oneof_response::observation(obs)) => Some(obs.clone()),
            _ => None,
        };
        let game_info = match response.response.as_ref() {
            Some(sc2_proto::sc2api::Response_oneof_response::game_info(game_info)) => {
                Some(game_info.clone())
            }
            _ => None,
        };
        let debug = match response.response.as_ref() {
            Some(sc2_proto::sc2api::Response_oneof_response::debug(_)) => response
                .write_to_bytes()
                .ok()
                .filter(|bytes| !bytes.is_empty()),
            _ => None,
        };

        Self {
            player_id,
            game_loop,
            observation,
            game_info,
            debug,
        }
    }
}

/// Bounded replay history for live loop scrubbing.
#[derive(Debug, Clone, Default)]
pub struct ReplayBuffer {
    capacity: usize,
    frames: VecDeque<ReplayFrame>,
}

impl ReplayBuffer {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            frames: VecDeque::new(),
        }
    }

    pub fn push_frame(&mut self, frame: ReplayFrame) {
        if self.frames.len() == self.capacity {
            self.frames.pop_front();
        }
        self.frames.push_back(frame);
    }

    pub fn len(&self) -> usize {
        self.frames.len()
    }

    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    pub fn latest_loop(&self) -> Option<u32> {
        self.frames.back().map(|frame| frame.game_loop)
    }

    pub fn oldest_loop(&self) -> Option<u32> {
        self.frames.front().map(|frame| frame.game_loop)
    }

    pub fn frames(&self) -> impl Iterator<Item = &ReplayFrame> {
        self.frames.iter()
    }

    pub fn latest_frame(&self) -> Option<&ReplayFrame> {
        self.frames.back()
    }

    pub fn clear(&mut self) {
        self.frames.clear();
    }
}

#[derive(Default)]
pub(crate) struct ReplayAssembler {
    current: FrameAccumulator,
}

impl ReplayAssembler {
    pub(crate) fn push(&mut self, tagged: TaggedResponse) -> Option<ReplayFrame> {
        let is_step = matches!(
            tagged.response.response,
            Some(sc2_proto::sc2api::Response_oneof_response::step(_))
        );
        self.current.update(tagged.player_id, &tagged.response);
        if is_step {
            self.current.finish()
        } else {
            None
        }
    }
}

#[derive(Clone)]
pub(super) struct ProxyPublisher {
    pub(super) response_sender: broadcast::Sender<TaggedResponse>,
    pub(super) replay_response_sender: tokio::sync::mpsc::Sender<TaggedResponse>,
}

impl ProxyPublisher {
    pub(super) async fn publish(&self, player_id: PlayerId, response: Response) {
        self.publish_to(&self.response_sender, player_id, response)
            .await;
    }

    pub(super) async fn publish_to(
        &self,
        sender: &broadcast::Sender<TaggedResponse>,
        player_id: PlayerId,
        response: Response,
    ) {
        let tagged = TaggedResponse {
            player_id,
            response,
        };
        let _ = sender.send(tagged.clone());
        if self.replay_response_sender.send(tagged).await.is_err() {
            eprintln!("[ProxyPublisher] Replay consumer has stopped");
        }
    }
}
