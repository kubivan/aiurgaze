use crate::frame::{Frame, FrameSource, StaticMap};
use crate::proxy_channel::{PlayerId, TaggedResponse};
use crate::sources::protocol::ResponseFrameAssembler;
use std::pin::Pin;
use tokio_stream::StreamExt;

pub type GameResponseStream = Pin<Box<dyn tokio_stream::Stream<Item = TaggedResponse> + Send>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VisionMode {
    #[default]
    Player1,
    Player2,
    All,
}

impl std::fmt::Display for VisionMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VisionMode::Player1 => write!(f, "Player 1"),
            VisionMode::Player2 => write!(f, "Player 2"),
            VisionMode::All => write!(f, "All"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct TaggedGameInfo {
    pub player_id: PlayerId,
    pub game_info: sc2_proto::sc2api::ResponseGameInfo,
}

fn game_info_only_stream(
    stream: GameResponseStream,
) -> impl tokio_stream::Stream<Item = TaggedGameInfo> {
    stream.filter_map(|tagged| {
        let game_info = match tagged.response.response.as_ref()? {
            sc2_proto::sc2api::Response_oneof_response::game_info(info) => info.clone(),
            _ => return None,
        };
        Some(TaggedGameInfo {
            player_id: tagged.player_id,
            game_info,
        })
    })
}

pub fn create_game_info_stream(
    p1_stream: GameResponseStream,
    p2_stream: Option<GameResponseStream>,
) -> impl tokio_stream::Stream<Item = TaggedGameInfo> {
    let p1 = game_info_only_stream(p1_stream);
    let p2: Pin<Box<dyn tokio_stream::Stream<Item = TaggedGameInfo> + Send>> = match p2_stream {
        Some(stream) => Box::pin(game_info_only_stream(stream)),
        None => Box::pin(tokio_stream::iter(std::iter::empty())),
    };
    p1.merge(p2)
}

#[derive(Default)]
pub(crate) struct FrameAssembler(ResponseFrameAssembler);

impl FrameAssembler {
    pub(crate) fn push(&mut self, response: TaggedResponse) -> Option<Frame> {
        self.0
            .push(FrameSource::Game, response.player_id, &response.response)
    }
}

pub(crate) fn spawn_frames(
    runtime: &bevy_tokio_tasks::TokioTasksRuntime,
    responses: tokio::sync::mpsc::Receiver<TaggedResponse>,
) {
    let mut assembler = FrameAssembler::default();
    super::spawn_frame_stream(runtime, responses, move |response| {
        assembler.push(response)
    });
}

pub(crate) fn static_map_from_info(
    game_info: &sc2_proto::sc2api::ResponseGameInfo,
) -> Option<StaticMap> {
    super::protocol::static_map_from_info(game_info)
}
