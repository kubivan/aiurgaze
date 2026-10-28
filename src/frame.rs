use crate::debug_draw::{DebugDrawPrimitive, DebugTextPrimitive};
use bevy::prelude::Message;
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

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Position {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Alliance {
    Self_,
    Ally,
    Neutral,
    Enemy,
    Unknown(i32),
}

#[derive(Debug, Clone, PartialEq)]
pub enum OrderTarget {
    Position(Position),
    Unit(u64),
}

#[derive(Debug, Clone, PartialEq)]
pub struct UnitOrder {
    pub ability_id: Option<u32>,
    pub target: Option<OrderTarget>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct UnitSnapshot {
    pub tag: u64,
    pub unit_type: u32,
    pub alliance: Alliance,
    pub position: Position,
    pub health: f32,
    pub health_max: f32,
    pub shield: f32,
    pub shield_max: f32,
    pub build_progress: f32,
    pub radius: f32,
    pub orders: Vec<UnitOrder>,
    pub display_fields: Vec<(String, String)>,
}

impl Default for Alliance {
    fn default() -> Self {
        Self::Unknown(0)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RasterLayer {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MapSnapshot {
    pub visibility: Option<RasterLayer>,
    pub creep: Option<RasterLayer>,
    pub energy: Option<RasterLayer>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StaticMap {
    pub pathing: RasterLayer,
    pub placement: RasterLayer,
    pub height: RasterLayer,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlayerSnapshot {
    pub minerals: u32,
    pub vespene: u32,
    pub food_used: u32,
    pub food_cap: u32,
    pub army_count: u32,
    pub worker_count: u32,
    pub idle_workers: u32,
}

#[derive(Debug, Clone)]
pub struct Frame {
    pub source: FrameSource,
    pub player_id: PlayerId,
    pub game_loop: u32,
    pub units: Vec<UnitSnapshot>,
    pub map: Option<MapSnapshot>,
    pub static_map: Option<StaticMap>,
    pub player: Option<PlayerSnapshot>,
    pub player_results: Vec<String>,
    pub debug_draws: Vec<DebugDrawPrimitive>,
    pub debug_texts: Vec<DebugTextPrimitive>,
}

#[derive(Message, Clone)]
pub struct FrameEvent {
    pub frame: Frame,
}

#[derive(Debug, Clone)]
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
            units: Vec::new(),
            map: None,
            static_map: None,
            player: None,
            player_results: Vec::new(),
            debug_draws: Vec::new(),
            debug_texts: Vec::new(),
        }
    }
}
