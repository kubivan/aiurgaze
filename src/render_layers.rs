use bevy::prelude::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RenderLayerKind {
    Terrain,
    Pathing,
    Placement,
    HeightMap,
    Creep,
    Energy,
    DebugOverlay,
    Minimap,
}

impl RenderLayerKind {
    pub const ALL: [RenderLayerKind; 8] = [
        RenderLayerKind::Terrain,
        RenderLayerKind::Pathing,
        RenderLayerKind::Placement,
        RenderLayerKind::HeightMap,
        RenderLayerKind::Creep,
        RenderLayerKind::Energy,
        RenderLayerKind::DebugOverlay,
        RenderLayerKind::Minimap,
    ];

    pub fn label(self) -> &'static str {
        match self {
            RenderLayerKind::Terrain => "Terrain",
            RenderLayerKind::Pathing => "Pathing",
            RenderLayerKind::Placement => "Placement",
            RenderLayerKind::HeightMap => "Height Map",
            RenderLayerKind::Creep => "Creep",
            RenderLayerKind::Energy => "Energy",
            RenderLayerKind::DebugOverlay => "Debug Overlay",
            RenderLayerKind::Minimap => "Minimap",
        }
    }
}

#[derive(Debug, Clone)]
pub struct LayerState {
    pub visible: bool,
}

#[derive(Resource, Debug, Clone)]
pub struct LayerRegistry {
    states: std::collections::HashMap<RenderLayerKind, LayerState>,
}

impl LayerRegistry {
    pub fn is_visible(&self, layer: RenderLayerKind) -> bool {
        self.states
            .get(&layer)
            .map(|state| state.visible)
            .unwrap_or(true)
    }

    pub fn set_visible(&mut self, layer: RenderLayerKind, visible: bool) {
        let entry = self
            .states
            .entry(layer)
            .or_insert(LayerState { visible: true });
        entry.visible = visible;
    }
}

impl Default for LayerRegistry {
    fn default() -> Self {
        let mut states = std::collections::HashMap::new();
        for layer in RenderLayerKind::ALL {
            states.insert(layer, LayerState { visible: true });
        }

        Self { states }
    }
}

#[derive(Component, Debug, Clone, Copy)]
pub struct RenderLayerMarker(pub RenderLayerKind);

pub fn layer_visibility_system(
    registry: Res<LayerRegistry>,
    mut query: Query<(&RenderLayerMarker, &mut Visibility)>,
) {
    for (marker, mut visibility) in &mut query {
        *visibility = if registry.is_visible(marker.0) {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
    }
}
