//! Observation pipeline: merges bot response streams, applies vision filtering,
//! and emits a unified stream for entity rendering.

use sc2_proto::sc2api::ResponseObservation;
use tokio::sync::{broadcast, watch};
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;

use crate::proxy_channel::{PlayerId, TaggedResponse};

/// Vision mode controlling which bot's perspective to render.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VisionMode {
    /// Show only Player1's vision
    #[default]
    Player1,
    /// Show only Player2's vision
    Player2,
    /// Show combined vision from all bots
    All,
}

impl VisionMode {
    /// Check if a given player's observations should be processed.
    pub fn accepts(&self, player_id: PlayerId) -> bool {
        match self {
            VisionMode::Player1 => player_id == PlayerId::Player1,
            VisionMode::Player2 => player_id == PlayerId::Player2,
            VisionMode::All => true,
        }
    }
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

/// Tagged observation with source player info.
#[derive(Debug, Clone)]
pub struct TaggedObservation {
    pub player_id: PlayerId,
    pub observation: ResponseObservation,
    pub vision_mode: VisionMode,
}

/// Tagged game info with source player.
#[derive(Debug, Clone)]
pub struct TaggedGameInfo {
    pub player_id: PlayerId,
    pub game_info: sc2_proto::sc2api::ResponseGameInfo,
}

/// Event emitted by the observation pipeline.
#[derive(Debug, Clone)]
pub enum PipelineEvent {
    Observation(TaggedObservation),
    GameInfo(TaggedGameInfo),
}

/// Settings for the observation pipeline.
pub struct PipelineSettings {
    /// Receiver for vision mode changes from UI.
    pub vision_mode_rx: watch::Receiver<VisionMode>,
}

/// Create the merged observation pipeline.
///
/// Takes receivers from bot channels and a vision mode watch channel,
/// returns a stream of pipeline events filtered by current vision mode.
pub fn create_observation_pipeline(
    player1_rx: broadcast::Receiver<TaggedResponse>,
    player2_rx: Option<broadcast::Receiver<TaggedResponse>>,
    vision_mode_rx: watch::Receiver<VisionMode>,
) -> impl tokio_stream::Stream<Item = PipelineEvent> {
    // Convert broadcast receivers to streams
    let stream1 = BroadcastStream::new(player1_rx).filter_map(|r| r.ok());
    
    // Merge streams based on whether we have 1 or 2 players
    let merged: Box<dyn tokio_stream::Stream<Item = TaggedResponse> + Send + Unpin> = 
        if let Some(rx2) = player2_rx {
            let stream2 = BroadcastStream::new(rx2).filter_map(|r| r.ok());
            Box::new(StreamExt::merge(stream1, stream2))
        } else {
            Box::new(stream1)
        };

    // Filter and transform based on vision mode
    let vision_mode = vision_mode_rx;
    merged.filter_map(move |tagged| {
        let current_mode = *vision_mode.borrow();
        
        // Extract observation or game_info from response
        if let Some(ref resp) = tagged.response.response {
            use sc2_proto::sc2api::Response_oneof_response::*;
            match resp {
                observation(obs) => {
                    // Filter by vision mode
                    if current_mode.accepts(tagged.player_id) {
                        Some(PipelineEvent::Observation(TaggedObservation {
                            player_id: tagged.player_id,
                            observation: obs.clone(),
                            vision_mode: current_mode,
                        }))
                    } else {
                        None
                    }
                }
                game_info(gi) => {
                    // Game info is always passed through (for map initialization)
                    Some(PipelineEvent::GameInfo(TaggedGameInfo {
                        player_id: tagged.player_id,
                        game_info: gi.clone(),
                    }))
                }
                _ => None,
            }
        } else {
            None
        }
    })
}

/// Create a vision mode watch channel.
///
/// Returns (sender for UI, receiver for pipeline).
pub fn create_vision_mode_channel() -> (watch::Sender<VisionMode>, watch::Receiver<VisionMode>) {
    watch::channel(VisionMode::default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vision_mode_accepts() {
        assert!(VisionMode::Player1.accepts(PlayerId::Player1));
        assert!(!VisionMode::Player1.accepts(PlayerId::Player2));
        
        assert!(!VisionMode::Player2.accepts(PlayerId::Player1));
        assert!(VisionMode::Player2.accepts(PlayerId::Player2));
        
        assert!(VisionMode::All.accepts(PlayerId::Player1));
        assert!(VisionMode::All.accepts(PlayerId::Player2));
    }

    #[test]
    fn test_vision_mode_display() {
        assert_eq!(format!("{}", VisionMode::Player1), "Player 1");
        assert_eq!(format!("{}", VisionMode::Player2), "Player 2");
        assert_eq!(format!("{}", VisionMode::All), "All");
    }
}
