use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

/// Resource that holds all entity data and pre-loaded assets
#[derive(Resource)]
pub struct EntitySystem {
    /// Unit data by ID (from data.json)
    pub units: HashMap<u32, UnitData>,
    /// Ability data by ID (from data.json)
    pub abilities: HashMap<u32, AbilityData>,
    /// Display configuration by unit ID (from entities.toml/json)
    pub display_config: HashMap<u32, EntityDisplayInfo>,
    /// Pre-loaded icon handles by unit type
    pub icon_handles: HashMap<u32, Handle<Image>>,
    /// Available icon basenames (without extension)
    icon_name_set: std::collections::HashSet<String>,
    /// Tile size for calculations
    pub tile_size: f32,
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
    pub label: Option<String>,
    pub color: Option<String>,
    pub size: Option<f32>,
    pub radius: Option<f32>,
    #[serde(default)]
    pub fields: Option<Vec<String>>,
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
    pub radius: Option<f32>,
    #[serde(default)]
    pub icon: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct EntitiesConfig {
    pub entity: Vec<TomlEntity>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct EntitiesJson {
    pub units: HashMap<u32, EntityDisplayInfo>,
}

impl EntitySystem {
    /// Load entity system from data.json and entities.toml
    pub fn load(asset_server: &AssetServer, tile_size: f32) -> Self {
        info!("Loading EntitySystem...");

        // Load data.json
        let data_json = std::fs::read_to_string("data/data.json")
            .unwrap_or_else(|e| {
                warn!("Failed to load data/data.json: {}", e);
                String::from(r#"{"Unit":[],"Ability":[]}"#)
            });
        let data: DataJson = serde_json::from_str(&data_json)
            .unwrap_or_else(|e| {
                warn!("Failed to parse data.json: {}", e);
                DataJson { unit: vec![], ability: vec![] }
            });

        // Build unit and ability maps
        let mut units = HashMap::new();
        for unit in data.unit {
            units.insert(unit.id, unit);
        }

        let mut abilities = HashMap::new();
        for ability in data.ability {
            abilities.insert(ability.id, ability);
        }

        // Scan available icons
        let mut icon_name_set = std::collections::HashSet::new();
        if let Ok(entries) = std::fs::read_dir("assets/icons") {
            for entry in entries.flatten() {
                if let Some(name) = entry.file_name().to_str() {
                    if let Some((base, _ext)) = name.rsplit_once('.') {
                        icon_name_set.insert(base.to_string());
                    }
                }
            }
        }

        // Load entities.toml (legacy config)
        let mut display_config: HashMap<u32, EntityDisplayInfo> = HashMap::new();
        if Path::new("config/entities.toml").exists() {
            if let Ok(toml_content) = std::fs::read_to_string("config/entities.toml") {
                if let Ok(config) = toml::de::from_str::<EntitiesConfig>(&toml_content) {
                    for entity in config.entity {
                        let mut info = EntityDisplayInfo::default();
                        info.name = Some(entity.name.clone());
                        info.label = Some(entity.name.clone());
                        info.radius = entity.radius;
                        if let Some(r) = entity.radius {
                            info.size = Some(r * 2.0 * tile_size);
                        }
                        info.icon = entity.icon;
                        display_config.insert(entity.id, info);
                    }
                    info!("Loaded {} entities from entities.toml", display_config.len());
                }
            }
        }

        // Merge with data from data.json and auto-generate missing entries
        for (id, unit) in &units {
            display_config.entry(*id).and_modify(|info| {
                // Fill in missing data from data.json
                if info.radius.is_none() {
                    info.radius = unit.radius;
                }
                if info.size.is_none() {
                    if let Some(r) = info.radius.or(unit.radius) {
                        info.size = Some(r * 2.0 * tile_size);
                    }
                }
                if info.name.is_none() {
                    info.name = Some(unit.name.clone());
                }
                if info.label.is_none() {
                    info.label = Some(unit.name.clone());
                }
                if info.icon.is_none() {
                    if icon_name_set.contains(&unit.name) {
                        info.icon = Some(format!("icons/{}.webp", unit.name));
                    }
                }
            }).or_insert_with(|| {
                // Create new entry from data.json
                let mut info = EntityDisplayInfo::default();
                info.name = Some(unit.name.clone());
                info.label = Some(unit.name.clone());
                info.radius = unit.radius;
                if let Some(r) = unit.radius {
                    info.size = Some(r * 2.0 * tile_size);
                }
                if icon_name_set.contains(&unit.name) {
                    info.icon = Some(format!("icons/{}.webp", unit.name));
                }
                info
            });
        }

        // Save to entities.json for caching
        let entities_json = EntitiesJson {
            units: display_config.clone(),
        };
        if let Ok(serialized) = serde_json::to_string_pretty(&entities_json) {
            let _ = std::fs::create_dir_all("config");
            let _ = std::fs::write("config/entities.json", serialized);
            info!("Saved entities.json with {} entries", display_config.len());
        }

        // Pre-load icons
        let mut icon_handles = HashMap::new();
        for (id, info) in &display_config {
            if let Some(icon_path) = &info.icon {
                let handle = asset_server.load(icon_path.clone());
                icon_handles.insert(*id, handle);
            }
        }
        info!("Pre-loaded {} icon handles", icon_handles.len());

        Self {
            units,
            abilities,
            display_config,
            icon_handles,
            icon_name_set,
            tile_size,
        }
    }

    /// Get unit data by ID
    pub fn get_unit(&self, unit_id: u32) -> Option<&UnitData> {
        self.units.get(&unit_id)
    }

    /// Get ability data by ID
    pub fn get_ability(&self, ability_id: u32) -> Option<&AbilityData> {
        self.abilities.get(&ability_id)
    }

    /// Get ability name by ID
    pub fn ability_name(&self, ability_id: u32) -> Option<&str> {
        self.abilities.get(&ability_id).map(|a| a.name.as_str())
    }

    /// Get display info for a unit type
    pub fn get_display_info(&self, unit_id: u32) -> EntityDisplayInfo {
        self.display_config.get(&unit_id).cloned().unwrap_or_default()
    }

    /// Get pre-loaded icon handle for a unit type
    pub fn get_icon_handle(&self, unit_id: u32, asset_server: &AssetServer) -> Handle<Image> {
        if let Some(handle) = self.icon_handles.get(&unit_id) {
            handle.clone()
        } else {
            // Fallback to loading on demand
            let info = self.get_display_info(unit_id);
            if let Some(icon_path) = info.icon {
                asset_server.load(icon_path)
            } else {
                // Default fallback image
                asset_server.load("png/mineral.png")
            }
        }
    }

    /// Get unit name by ID
    pub fn unit_name(&self, unit_id: u32) -> Option<&str> {
        self.units.get(&unit_id).map(|u| u.name.as_str())
    }

    /// Get unit radius by ID
    pub fn unit_radius(&self, unit_id: u32) -> Option<f32> {
        self.display_config.get(&unit_id)
            .and_then(|d| d.radius)
            .or_else(|| self.units.get(&unit_id).and_then(|u| u.radius))
    }

    /// Get unit size (in pixels) by ID
    pub fn unit_size(&self, unit_id: u32) -> f32 {
        self.display_config.get(&unit_id)
            .and_then(|d| d.size)
            .unwrap_or(32.0) // Default size
    }
}

/// Startup system to initialize EntitySystem
pub fn setup_entity_system(mut commands: Commands, asset_server: Res<AssetServer>) {
    let tile_size = 16.0; // Could be configured
    let entity_system = EntitySystem::load(&asset_server, tile_size);
    commands.insert_resource(entity_system);
}
