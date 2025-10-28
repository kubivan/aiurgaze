use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::File;
use std::path::Path;
use serde_json::from_reader;
use crate::app_settings::{MapConfig};

/// Resource that holds all entity data and pre-loaded assets
#[derive(Resource)]
pub struct EntitySystem {
    /// Map configuration (style, colors, etc.) from entities.toml [map] section
    pub map_config: MapConfig,
    /// Unit data by ID (from data.json)
    pub unit_traits: HashMap<u32, UnitData>,
    /// Ability data by ID (from data.json)
    pub abilities: HashMap<u32, AbilityData>,
    /// Pre-loaded icon handles by unit type
    pub icon_handles: HashMap<u32, Handle<Image>>,
    /// Display configuration by unit ID (from entities.toml)
    pub display_config: HashMap<u32, EntityDisplayInfo>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct UnitData {
    pub id: u32,
    pub name: String,
    #[serde(default)]
    pub radius: Option<f32>,
    #[serde(default)]
    pub food_required: Option<f32>,
    #[serde(default)]
    pub mineral_cost: Option<u32>,
    #[serde(default)]
    pub vespene_cost: Option<u32>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AbilityData {
    pub id: u32,
    pub name: String,
    #[serde(default)]
    pub cast_range: Option<f32>,
    #[serde(default)]
    pub energy_cost: Option<u32>,
    #[serde(default)]
    pub cooldown: Option<f32>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct EntityDisplayInfo {
    pub name: Option<String>,
    pub icon: Option<String>,
    #[serde(default)]
    pub tile_size: Option<[f32; 2]>,
}

#[derive(Debug, Clone, Deserialize)]
struct DataJson {
    #[serde(rename = "Unit", default)]
    pub unit: Vec<UnitData>,
    #[serde(rename = "Ability", default)]
    pub ability: Vec<AbilityData>,
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
    /// Load entity system from data.json and entities.toml
    pub fn load(asset_server: &AssetServer) -> Self {
        info!("Loading EntitySystem...");

        // Load data.json
        let data_json_file = File::open("data/data.json")
            .expect("Failed to open data.json");

        let data: DataJson = from_reader(data_json_file)
            .expect( "Failed to parse data.json");

        // Build unit and ability maps
        let units = data.unit.into_iter()
            .map( |u| (u.id, u)).collect::<HashMap<u32, UnitData>>();

        let mut abilities = HashMap::new();
        for ability in data.ability {
            abilities.insert(ability.id, ability);
        }

        /// Display configuration by unit ID (from entities.toml)
        let mut map_config = MapConfig::default();
        let mut display_config: HashMap<u32, EntityDisplayInfo> = HashMap::new();

        let toml_content = std::fs::read_to_string("data/entities.toml")
            .expect("Failed to read entities.toml");
        let config = toml::de::from_str::<EntitiesConfig>(&toml_content)
            .expect("Failed to parse entities.toml");
        // Load map config from [map] section
        if let Some(map) = config.map {
            map_config = map;
        }

        for entity in config.entity {
            let mut info = EntityDisplayInfo::default();
            info.name = Some(entity.name.clone());
            info.icon = entity.icon;
            info.tile_size = entity.tile_size;
            display_config.insert(entity.id, info);
        }
        info!("Loaded {} entities from entities.toml", display_config.len());

        // Pre-load icons
        let mut icon_handles = HashMap::new();
        for (id, info) in &display_config {
            if let Some(icon_path) = &info.icon {
                let handle = asset_server.load(icon_path.clone());
                icon_handles.insert(*id, handle);
            }else {
                let handle = asset_server.load("units/default.png");
                info!("No icon for unit {}. Using default icon.", id);
                icon_handles.insert(*id, handle);
            }
        }
        info!("Pre-loaded {} icon handles", icon_handles.len());

        Self {
            map_config,
            unit_traits: units,
            abilities,
            icon_handles,
            display_config,
        }
    }

    /// Get unit data by ID
    pub fn get_unit(&self, unit_id: u32) -> Option<&UnitData> {
        self.unit_traits.get(&unit_id)
    }

    /// Get ability data by ID
    pub fn get_ability(&self, ability_id: u32) -> Option<&AbilityData> {
        self.abilities.get(&ability_id)
    }

    /// Get ability name by ID
    pub fn ability_name(&self, ability_id: u32) -> Option<&str> {
        self.abilities.get(&ability_id).map(|a| a.name.as_str())
    }

    /// Get pre-loaded icon handle for a unit type
    pub fn get_icon_handle(&self, unit_id: u32, asset_server: &AssetServer) -> Handle<Image> {
        if let Some(handle) = self.icon_handles.get(&unit_id) {
            handle.clone()
        } else {
            //TODO: another default icon?
            asset_server.load("icons/default.png")
        }
    }

    /// Get a custom tile size for a unit type [width, height], if specified in config
    pub fn get_custom_tile_size(&self, unit_id: u32) -> Option<[f32; 2]> {
        self.display_config.get(&unit_id).and_then(|info| info.tile_size)
    }
    pub fn unit_name(&self, unit_id: u32) -> Option<&str> {
        self.unit_traits.get(&unit_id).map(|u| u.name.as_str())
    }
}

/// Startup system to initialize EntitySystem
pub fn setup_entity_system(mut commands: Commands, asset_server: Res<AssetServer>) {
    let entity_system = EntitySystem::load(&asset_server);
    commands.insert_resource(entity_system);
}
