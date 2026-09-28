use super::{PlayerId, TaggedResponse};
use protobuf::Message;
use sc2_proto::sc2api::{Response, ResponseGameInfo, ResponseObservation};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
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

/// Proxy-owned frame accumulator for one simulation step.
#[derive(Debug, Clone, Default)]
pub struct CurrentFrame {
    pub player_id: Option<PlayerId>,
    pub game_loop: u32,
    pub observation: Option<ResponseObservation>,
    pub game_info: Option<ResponseGameInfo>,
    pub debug: Option<Vec<u8>>,
}

impl CurrentFrame {
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

#[derive(Clone)]
pub(super) struct ProxyPublisher {
    pub(super) response_sender: broadcast::Sender<TaggedResponse>,
    pub(super) frame_sender: broadcast::Sender<ReplayFrame>,
    pub(super) replay_buffer: Arc<Mutex<ReplayBuffer>>,
    pub(super) current_frame: Arc<Mutex<CurrentFrame>>,
}

impl ProxyPublisher {
    pub(super) fn publish(&self, player_id: PlayerId, response: Response) {
        self.publish_to(&self.response_sender, player_id, response);
    }

    pub(super) fn publish_to(
        &self,
        sender: &broadcast::Sender<TaggedResponse>,
        player_id: PlayerId,
        response: Response,
    ) {
        let response_clone = response.clone();
        let _ = sender.send(TaggedResponse {
            player_id,
            response,
        });

        let mut frame = self
            .current_frame
            .lock()
            .expect("current frame lock poisoned");
        frame.update(player_id, &response_clone);
        let is_step = matches!(
            response_clone.response,
            Some(sc2_proto::sc2api::Response_oneof_response::step(_))
        );
        if is_step {
            if let Some(completed) = frame.finish() {
                let _ = self.frame_sender.send(completed.clone());
                self.replay_buffer
                    .lock()
                    .expect("replay buffer lock poisoned")
                    .push_frame(completed);
            }
        }
    }
}
