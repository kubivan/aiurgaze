use bevy::prelude::{Res, Resource, SystemSet};

#[derive(Resource, Default, Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderViewMode {
    #[default]
    TwoD,
    ThreeD,
}

#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RenderViewSet {
    TwoD,
    ThreeD,
}

pub fn render_view_is_2d(mode: Res<RenderViewMode>) -> bool {
    *mode == RenderViewMode::TwoD
}

pub fn render_view_is_3d(mode: Res<RenderViewMode>) -> bool {
    *mode == RenderViewMode::ThreeD
}
