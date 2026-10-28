use crate::debug_draw::{DebugDrawPrimitive, DebugTextPrimitive};
use bevy::prelude::{Message, Resource, World};
use sc2_proto::sc2api::ResponseObservation;
use std::collections::VecDeque;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameSource {
    Game,
    Replay,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayerId {
    Player1,
    Player2,
}

impl std::fmt::Display for PlayerId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PlayerId::Player1 => write!(f, "Player1"),
            PlayerId::Player2 => write!(f, "Player2"),
        }
    }
}

#[derive(Debug)]
pub struct Frame {
    pub source: FrameSource,
    pub player_id: PlayerId,
    pub game_loop: u32,
    // Own the protocol payload directly; consumers borrow it from frame history.
    pub observation: ResponseObservation,
    pub debug_draws: Vec<DebugDrawPrimitive>,
    pub debug_texts: Vec<DebugTextPrimitive>,
}

#[derive(Message)]
pub struct FrameEvent;

/// Own incoming frames until the frame system moves them into playback history.
/// Notifications stay independently readable through Bevy's message readers.
#[derive(Resource, Default)]
pub(crate) struct PendingFrames(pub VecDeque<Frame>);

pub(crate) fn publish_frame(world: &mut World, frame: Frame) {
    world.resource_mut::<PendingFrames>().0.push_back(frame);
    world.write_message(FrameEvent);
}

#[derive(Debug)]
pub struct FrameHistory {
    capacity: usize,
    frames: VecDeque<Frame>,
}

impl Default for FrameHistory {
    fn default() -> Self {
        Self::new(512)
    }
}

impl FrameHistory {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            frames: VecDeque::new(),
        }
    }

    pub fn push_frame(&mut self, frame: Frame) {
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

    pub fn frames(&self) -> impl Iterator<Item = &Frame> {
        self.frames.iter()
    }

    pub fn latest_frame(&self) -> Option<&Frame> {
        self.frames.back()
    }

    pub fn frame_for_loop_mut(&mut self, loop_id: u32) -> Option<&mut Frame> {
        self.frames
            .iter_mut()
            .rev()
            .find(|frame| frame.game_loop == loop_id)
    }

    pub fn clear(&mut self) {
        self.frames.clear();
    }
}

impl Frame {
    pub fn new(source: FrameSource, player_id: PlayerId, game_loop: u32) -> Self {
        Self {
            source,
            player_id,
            game_loop,
            observation: ResponseObservation::new(),
            debug_draws: Vec::new(),
            debug_texts: Vec::new(),
        }
    }
}
