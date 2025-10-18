use config::{Config, File};
use std::path::PathBuf;
use bevy::prelude::Resource;
use serde::{Deserialize, Serialize};
use std::fs;

#[derive(Debug, Clone, Resource, Deserialize, Serialize)]
pub struct AppSettings {
    pub ws_url: String,
    pub ws_port: u16,
    pub tile_size: f32,
    pub config_path: PathBuf,
    pub data_source: DataSource,
    pub entity_display: EntityDisplayConfig,
    #[serde(skip)]
    pub unit_by_id: std::collections::HashMap<u32, DisplayInfo>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct DataSource {
    #[serde(rename = "Ability")]
    pub ability: Vec<AbilityData>,
    #[serde(rename = "Unit")]
    pub unit: Vec<UnitData>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct AbilityData { pub id: u32, pub name: String }

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct UnitData { pub id: u32, pub name: String, pub radius: Option<f32> }

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct EntityDisplayConfig {
    pub unit: std::collections::HashMap<String, DisplayInfo>, // id-keyed as strings from TOML [unit.<id>]
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct DisplayInfo {
    pub name: Option<String>,
    pub icon: Option<String>,
    pub fields: Option<Vec<String>>,
    pub label: Option<String>,
    pub color: Option<String>,
    pub size: Option<f32>,
    pub radius: Option<f32>,
}

// JSON entities structure
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
struct EntitiesJson { pub units: std::collections::HashMap<u32, DisplayInfo> }

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            ws_url: "ws://127.0.0.1".to_string(),
            ws_port: 5555,
            tile_size: 32.0,
            config_path: PathBuf::from("config/entities.toml"),
            data_source: DataSource::default(),
            entity_display: EntityDisplayConfig::default(),
            //TODO: Load from data.json instead of hardcoding
            unit_by_id: std::collections::HashMap::new(),
        }
    }
}

pub fn load_settings() -> AppSettings {
    let default = AppSettings::default();
    let cfg_path = default.config_path.clone();

    // Read TOML (legacy) if exists
    let entity_display: EntityDisplayConfig = config::Config::builder()
        .add_source(config::File::with_name(cfg_path.to_str().unwrap()).required(false))
        .build()
        .and_then(|c| c.try_deserialize().map_err(|e| e.into()))
        .unwrap_or_default();

    // Load data.json
    let data_json = std::fs::read_to_string("data/data.json").unwrap_or_default();
    let data_source: DataSource = serde_json::from_str(&data_json).unwrap_or_default();

    // Prepare entities.json path
    let entities_json_path = std::path::Path::new("config/entities.json");
    let mut units_map: std::collections::HashMap<u32, DisplayInfo> = std::collections::HashMap::new();

    let tile_size = default.tile_size;

    if entities_json_path.exists() {
        if let Ok(text) = std::fs::read_to_string(entities_json_path) {
            if let Ok(parsed) = serde_json::from_str::<EntitiesJson>(&text) {
                units_map = parsed.units;
            }
        }
    }

    // List available icon basenames (without extension) from assets/icons
    let mut icon_name_set = std::collections::HashSet::new();
    if let Ok(entries) = std::fs::read_dir("assets/icons") {
        for e in entries.flatten() {
            if let Some(name) = e.file_name().to_str() {
                if let Some((base, _ext)) = name.rsplit_once('.') { icon_name_set.insert(base.to_string()); }
            }
        }
    }

    // Augment / create entries
    for u in &data_source.unit {
        units_map.entry(u.id).and_modify(|info| {
            if info.radius.is_none() { info.radius = u.radius; }
            if info.size.is_none() { if let Some(r) = info.radius.or(u.radius) { info.size = Some(r * 2.0 * tile_size); } }
            if info.name.is_none() { info.name = Some(u.name.clone()); }
            // Fill missing radius/size/icon/label
            if info.label.is_none() { info.label = Some(u.name.clone()); }
            if info.icon.is_none() {
                let base = &u.name;
                if icon_name_set.contains(base) { info.icon = Some(format!("icons/{}.webp", base)); }
            }
        }).or_insert_with(|| {
            let mut info = DisplayInfo::default();
            info.name = Some(u.name.clone());
            info.label = Some(u.name.clone());
            info.radius = u.radius;
            if let Some(r) = u.radius { info.size = Some(r * 2.0 * tile_size); }
            let base = &u.name;
            if icon_name_set.contains(base) { info.icon = Some(format!("icons/{}.webp", base)); }
            info
        });
    }

    let entities_json = EntitiesJson { units: units_map.clone() };
    if let Ok(serialized) = serde_json::to_string_pretty(&entities_json) {
        let _ = std::fs::create_dir_all("config");
        let _ = std::fs::write(entities_json_path, serialized);
    }

    // Build unit_by_id (override legacy TOML) from units_map
    let unit_by_id = units_map;

    AppSettings {
        entity_display,
        data_source,
        unit_by_id,
        ..default
    }
}

impl AppSettings {
    pub fn get_unit_display_by_id(&self, unit_type_id: u32) -> DisplayInfo {
        self.unit_by_id.get(&unit_type_id).cloned().unwrap_or_default()
    }
    pub fn ability_name_by_id(&self, ability_id: u32) -> Option<&str> {
        for ability in &self.data_source.ability { if ability.id == ability_id { return Some(ability.name.as_str()); } }
        None
    }
}
