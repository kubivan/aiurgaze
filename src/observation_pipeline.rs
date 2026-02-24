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

/// Merges P2's own units into P1's observation, re-tagging them as Enemy.
///
/// This produces a single unified observation from P1's perspective where:
/// - P1's own/allied/neutral units are preserved as-is from P1's obs
/// - P2's own units (which are the actual enemy units) are appended re-tagged as `Enemy`
///
/// This mimics "disable_fog" behaviour without requiring a real observer bot.
fn merge_p2_into_p1_obs(
    p1_obs: &ResponseObservation,
    p2_obs: &ResponseObservation,
) -> ResponseObservation {
    let mut merged = p1_obs.clone();
    if let Some(p1_inner) = merged.observation.as_mut() {
        if let Some(p1_raw) = p1_inner.raw_data.as_mut() {
            if let Some(p2_inner) = p2_obs.observation.as_ref() {
                if let Some(p2_raw) = p2_inner.raw_data.as_ref() {
                    // Take P2's own units and resurface them as Enemy in P1's view.
                    // Alliance::value_Self corresponds to proto value 1 ("Self").
                    for unit in p2_raw.units.iter() {
                        if unit.alliance == Some(Alliance::value_Self) {
                            let mut enemy_unit = unit.clone();
                            enemy_unit.set_alliance(Alliance::Enemy);
                            p1_raw.units.push(enemy_unit);
                        }
                    }
                }
            }
        }
    }
    merged
}

/// Create the merged observation pipeline.
///
/// ```text
/// p1$ ─┐                                                    ┌─► PipelineEvent
///      ├─ merge ─► filter_map(mode, withLatestFrom(p2_obs)) ┤
/// p2$ ─┘              │                                     └─► PipelineEvent
///               send(p2_obs_tx)
/// ```
///
/// `VisionMode::All`: P2 observations feed the watch slot (withLatestFrom source);
/// P1 observations read it via `.borrow()` and call `merge_p2_into_p1_obs`.
/// Other modes: standard accept/reject per player.
pub fn create_observation_pipeline(
    player1_rx: broadcast::Receiver<TaggedResponse>,
    player2_rx: Option<broadcast::Receiver<TaggedResponse>>,
    vision_mode_rx: watch::Receiver<VisionMode>,
) -> impl tokio_stream::Stream<Item = PipelineEvent> {
    use sc2_proto::sc2api::Response_oneof_response::*;

    let s1 = BroadcastStream::new(player1_rx).filter_map(|r| r.ok());
    let (p2_obs_tx, p2_obs_rx) = watch::channel::<Option<ResponseObservation>>(None);

    let merged: Box<dyn tokio_stream::Stream<Item = TaggedResponse> + Send + Unpin> =
        match player2_rx {
            Some(rx2) => Box::new(s1.merge(BroadcastStream::new(rx2).filter_map(|r| r.ok()))),
            None => Box::new(s1),
        };

    merged.filter_map(move |tagged| {
        let mode = *vision_mode_rx.borrow();
        match tagged.response.response.as_ref()? {
            observation(obs) => {
                if mode == VisionMode::All && tagged.player_id == PlayerId::Player2 {
                    let _ = p2_obs_tx.send(Some(obs.clone())); // drive withLatestFrom slot
                    return None;
                }
                if !mode.accepts(tagged.player_id) {
                    return None;
                }
                let obs_out = match (mode, p2_obs_rx.borrow().as_ref()) {
                    (VisionMode::All, Some(p2_obs)) => merge_p2_into_p1_obs(obs, p2_obs),
                    _ => obs.clone(),
                };
                Some(PipelineEvent::Observation(TaggedObservation {
                    player_id: tagged.player_id,
                    observation: obs_out,
                    vision_mode: mode,
                }))
            }
            game_info(gi) => Some(PipelineEvent::GameInfo(TaggedGameInfo {
                player_id: tagged.player_id,
                game_info: gi.clone(),
            })),
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
