//! Observation pipeline: merges bot response streams, applies vision filtering,
//! and emits typed streams for entity rendering.
//!
//! Two separate stream constructors:
//! - `create_observation_stream` — hot, continuous, vision-mode–aware
//! - `create_game_info_stream`   — cold, fires once per player at start

use crate::proxy_channel::{PlayerId, TaggedResponse};
use sc2_proto::raw::Alliance;
use sc2_proto::sc2api::ResponseObservation;
use std::pin::Pin;
use tokio::sync::watch;
use tokio_stream::StreamExt;

pub type TaggedResponseStream = Pin<Box<dyn tokio_stream::Stream<Item = TaggedResponse> + Send>>;

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

fn observation_only_stream(
    stream: TaggedResponseStream,
) -> impl tokio_stream::Stream<Item = (PlayerId, ResponseObservation)> {
    use sc2_proto::sc2api::Response_oneof_response::observation;
    stream.filter_map(|tagged| {
        let obs = match tagged.response.response.as_ref()? {
            observation(obs) => obs.clone(),
            _ => return None,
        };
        Some((tagged.player_id, obs))
    })
}

fn game_info_only_stream(
    stream: TaggedResponseStream,
) -> impl tokio_stream::Stream<Item = TaggedGameInfo> {
    use sc2_proto::sc2api::Response_oneof_response::game_info;
    stream.filter_map(|tagged| {
        let gi = match tagged.response.response.as_ref()? {
            game_info(gi) => gi.clone(),
            _ => return None,
        };
        Some(TaggedGameInfo {
            player_id: tagged.player_id,
            game_info: gi,
        })
    })
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

    merge_p2_visibility_into_p1_obs(&mut merged, p2_obs);

    merged
}

/// Merge P2 visibility into P1 map_state.visibility in-place (cell-wise max).
///
/// Visibility values per cell: 0 = Hidden, 1 = Fogged, 2 = Visible.
/// Taking the max gives the union of both players' explored areas.
fn merge_p2_visibility_into_p1_obs(merged: &mut ResponseObservation, p2_obs: &ResponseObservation) {
    // Drill down to P2's raw visibility bytes.
    let Some(p2_data) = p2_obs
        .observation
        .as_ref()
        .and_then(|o| o.raw_data.as_ref())
        .and_then(|r| r.map_state.as_ref())
        .and_then(|ms| ms.visibility.as_ref())
        .and_then(|vi| vi.data.as_ref())
        .filter(|d| !d.is_empty())
    else {
        return;
    };

    // Drill down to P1's mutable visibility bytes.
    let Some(p1_data) = merged
        .observation
        .as_mut()
        .and_then(|o| o.raw_data.as_mut())
        .and_then(|r| r.map_state.as_mut())
        .and_then(|ms| ms.visibility.as_mut())
        .and_then(|vi| vi.data.as_mut())
    else {
        return;
    };

    if p1_data.len() != p2_data.len() {
        return;
    }

    for (p1, p2) in p1_data.iter_mut().zip(p2_data.iter()) {
        *p1 = (*p1).max(*p2);
    }
}

/// Create the game info stream (cold — fires once per player at startup).
///
/// ```text
/// p1$ ──► game_info ──┐
///                     ├──► TaggedGameInfo
/// p2$ ──► game_info ──┘
/// ```
///
/// Simply extracts `ResponseGameInfo` from both players' response streams
/// and merges them. No vision filtering — map init needs both.
pub fn create_game_info_stream(
    p1_stream: TaggedResponseStream,
    p2_stream: Option<TaggedResponseStream>,
) -> impl tokio_stream::Stream<Item = TaggedGameInfo> {
    let gi1 = game_info_only_stream(p1_stream);

    let gi2: Box<dyn tokio_stream::Stream<Item = TaggedGameInfo> + Send + Unpin> = match p2_stream {
        Some(s2) => Box::new(game_info_only_stream(s2)),
        None => Box::new(tokio_stream::iter(std::iter::empty())),
    };

    gi1.merge(gi2)
}

/// Create the observation stream (hot — continuous during game).
///
/// ```text
/// p2$ ──► hold(latest_p2) ──────────────────────────────┐
///                  │                                    ├──► TaggedObservation
/// p1$ ──► obs.withLatestFrom(latest_p2) ────────────────┘
/// ```
///
/// P2 observations feed a `watch` (BehaviorSubject / hold-latest).
/// P1 observations read it via `.borrow()` and complement with
/// `merge_p2_into_p1_obs` when in `All` mode.
pub fn create_observation_stream(
    p1_stream: TaggedResponseStream,
    p2_stream: Option<TaggedResponseStream>,
    vision_mode_rx: watch::Receiver<VisionMode>,
) -> impl tokio_stream::Stream<Item = TaggedObservation> {
    let (hold_p2, latest_p2) = watch::channel::<Option<ResponseObservation>>(None);
    let p1_obs_source = observation_only_stream(p1_stream);

    // ── P2: update hold slot; emit obs only in Player2 mode ──
    let mode_p2 = vision_mode_rx.clone();
    let p2_obs: Box<dyn tokio_stream::Stream<Item = TaggedObservation> + Send + Unpin> =
        match p2_stream {
            Some(s2) => Box::new(observation_only_stream(s2).filter_map(
                move |(player_id, obs)| {
                    let mode = *mode_p2.borrow();
                    let _ = hold_p2.send(Some(obs.clone()));
                    (mode == VisionMode::Player2).then_some(TaggedObservation {
                        player_id,
                        observation: obs,
                        vision_mode: mode,
                    })
                },
            )),
            None => Box::new(tokio_stream::iter(std::iter::empty())),
        };

    // ── P1: obs.withLatestFrom(latest_p2) → complement in All mode ──
    let p1_obs = p1_obs_source.filter_map(move |(player_id, obs)| {
        let mode = *vision_mode_rx.borrow();
        if !mode.accepts(player_id) {
            return None;
        }
        let obs_out = match (mode, latest_p2.borrow().as_ref()) {
            (VisionMode::All, Some(p2)) => merge_p2_into_p1_obs(&obs, p2),
            _ => obs,
        };
        Some(TaggedObservation {
            player_id,
            observation: obs_out,
            vision_mode: mode,
        })
    });

    p1_obs.merge(p2_obs)
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
