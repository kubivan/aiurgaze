use crate::observation_pipeline::VisionMode;
use bevy::prelude::*;
use tokio::sync::watch;

/// Resource to hold the vision mode watch channel sender.
/// UI updates this to change which bot's perspective is rendered.
#[derive(Resource)]
pub struct VisionModeChannel {
    pub sender: watch::Sender<VisionMode>,
    pub current: VisionMode,
}

impl VisionModeChannel {
    pub fn new() -> (Self, watch::Receiver<VisionMode>) {
        let (sender, receiver) = watch::channel(VisionMode::default());
        (
            Self {
                sender,
                current: VisionMode::default(),
            },
            receiver,
        )
    }

    pub fn set(&mut self, mode: VisionMode) {
        self.current = mode;
        let _ = self.sender.send(mode);
    }
}

impl Default for VisionModeChannel {
    fn default() -> Self {
        let (sender, _) = watch::channel(VisionMode::default());
        Self {
            sender,
            current: VisionMode::default(),
        }
    }
}
