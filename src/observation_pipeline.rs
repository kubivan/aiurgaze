//! Observation pipeline: merges bot response streams, applies vision filtering,
//! and emits a unified stream for entity rendering.

use sc2_proto::sc2api::ResponseObservation;
use sc2_proto::raw::Alliance;
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

/// Returns a copy of the observation with neutral units removed.
/// This is useful for hiding neutral units from the perspective of Player2 in "All" vision mode.
fn strip_neutral_units(obs: &ResponseObservation) -> ResponseObservation {
    let mut obs = obs.clone();
    if let Some(o) = obs.observation.as_mut() {
        if let Some(raw) = o.raw_data.as_mut() {
            raw.units.retain(|unit| unit.alliance != Some(Alliance::Neutral));
        }
    }
    obs
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

        use sc2_proto::sc2api::Response_oneof_response::*;
        let resp = tagged.response.response.as_ref()?;

        match resp {
            observation(obs) => {
                if !current_mode.accepts(tagged.player_id) {
                    return None;
                }
                // Strip neutral units for Player2 in All vision mode
                let obs_final = if current_mode == VisionMode::All && tagged.player_id == PlayerId::Player2 {
                    strip_neutral_units(obs)
                } else {
                    obs.clone()
                };
                Some(PipelineEvent::Observation(TaggedObservation {
                    player_id: tagged.player_id,
                    observation: obs_final,
                    vision_mode: current_mode,
                }))
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
