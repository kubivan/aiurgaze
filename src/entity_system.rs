use crate::app_settings::{get_data_dir, MapConfig};
use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Resource that holds all entity data and pre-loaded assets
#[derive(Resource)]
pub struct EntitySystem {
    /// Map configuration (style, colors, etc.) from entities.toml [map] section
    pub map_config: MapConfig,
    /// Pre-loaded icon handles by unit type
    pub icon_handles: HashMap<u32, Handle<Image>>,
    /// Display configuration by unit ID (from entities.toml)
    pub display_config: HashMap<u32, EntityDisplayInfo>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct EntityDisplayInfo {
    pub name: Option<String>,
    pub icon: Option<String>,
    #[serde(default)]
    pub tile_size: Option<[f32; 2]>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct TomlEntity {
    pub id: u32,
    pub name: String,
    #[serde(default)]
    pub tile_size: Option<[f32; 2]>,
    #[serde(default)]
    pub icon: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct EntitiesConfig {
    #[serde(default)]
    pub map: Option<MapConfig>,
    #[serde(default)]
    pub entity: Vec<TomlEntity>,
}

impl EntitySystem {
    /// Load entity display settings and icons from entities.toml.
    pub fn load(asset_server: &AssetServer) -> Self {
        info!("Loading EntitySystem...");

        let data_dir = get_data_dir();

        // Display configuration by unit ID (from entities.toml)
        let mut map_config = MapConfig::default();
        let mut display_config: HashMap<u32, EntityDisplayInfo> = HashMap::new();

        let entities_path = data_dir.join("entities.toml");
        let toml_content = std::fs::read_to_string(&entities_path)
            .unwrap_or_else(|_| panic!("Failed to read {:?}", entities_path));
        let config = toml::de::from_str::<EntitiesConfig>(&toml_content)
            .expect("Failed to parse entities.toml");

        // Load map config from [map] section
        if let Some(map) = config.map {
            map_config = map;
        }

        for entity in config.entity {
            let info = EntityDisplayInfo {
                name: Some(entity.name.clone()),
                icon: entity.icon,
                tile_size: entity.tile_size,
            };
            display_config.insert(entity.id, info);
        }
        info!(
            "Loaded {} entities from entities.toml",
            display_config.len()
        );

        // Pre-load icons
        let mut icon_handles = HashMap::new();
        for (id, info) in &display_config {
            let handle = if let Some(icon_path) = &info.icon {
                asset_server.load(icon_path.clone())
            } else {
                info!("No icon for unit {}. Using default icon.", id);
                asset_server.load("units/default.png")
            };
            icon_handles.insert(*id, handle);
        }
        info!("Pre-loaded {} icon handles", icon_handles.len());

        Self {
            map_config,
            icon_handles,
            display_config,
        }
    }

    /// Get pre-loaded icon handle for a unit type
    pub fn get_icon_handle(&self, unit_id: u32, asset_server: &AssetServer) -> Handle<Image> {
        if let Some(handle) = self.icon_handles.get(&unit_id) {
            return handle.clone();
        }

        // Use units/default.png as the fallback default icon
        asset_server.load("units/default.png")
    }

    /// Get a custom tile size for a unit type [width, height], if specified in config
    pub fn get_custom_tile_size(&self, unit_id: u32) -> Option<[f32; 2]> {
        self.display_config
            .get(&unit_id)
            .and_then(|info| info.tile_size)
    }
}

/// Startup system to initialize EntitySystem
pub fn setup_entity_system(mut commands: Commands, asset_server: Res<AssetServer>) {
    let entity_system = EntitySystem::load(&asset_server);
    commands.insert_resource(entity_system);
}
