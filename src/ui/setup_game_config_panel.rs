use bevy::prelude::*;
use crate::ui::game_config_panel::GameConfigPanel;

pub fn setup_game_config_panel(mut commands: Commands) {
    commands.insert_resource(GameConfigPanel::new());
}

