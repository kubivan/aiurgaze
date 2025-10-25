use config::{Config, File};
use std::path::PathBuf;
use bevy::prelude::{Resource, Color};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Resource, Deserialize, Serialize)]
pub struct AppSettings {
    pub window: WindowConfig,
    pub starcraft: StarcraftConfig,
    #[serde(skip)]
    pub config_path: PathBuf,
    #[serde(skip)]
    pub data_source: DataSource,
    #[serde(skip)]
    pub entity_display: EntityDisplayConfig,
    #[serde(skip)]
    pub unit_by_id: std::collections::HashMap<u32, DisplayInfo>,
    pub game_config_panel: GameConfigPanelDefaults,
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
    #[serde(default)]
    pub map: Option<MapConfig>,
    pub unit: std::collections::HashMap<String, DisplayInfo>, // id-keyed as strings from TOML [unit.<id>]
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct MapConfig {
    pub tile_size: f32,
    // Terrain colors as [r, g, b] arrays
    pub terrain_blocked: [f32; 3],
    pub terrain_pathable: [f32; 3],
    pub terrain_placeable: [f32; 3],
    pub terrain_both: [f32; 3],
    // Overlay colors
    pub creep: [f32; 3],
    pub energy: [f32; 3],
    // Height intensity [min, max]
    pub height_intensity: [f32; 2],
}

impl MapConfig {
    pub fn get_terrain_color(&self, pathable: bool, placeable: bool) -> Color {
        let rgb = match (pathable, placeable) {
            (false, false) => self.terrain_blocked,
            (true, false) => self.terrain_pathable,
            (false, true) => self.terrain_placeable,
            (true, true) => self.terrain_both,
        };
        Color::srgb(rgb[0], rgb[1], rgb[2])
    }

    pub fn get_creep_color(&self) -> Color {
        Color::srgb(self.creep[0], self.creep[1], self.creep[2])
    }

    pub fn get_energy_color(&self) -> Color {
        Color::srgb(self.energy[0], self.energy[1], self.energy[2])
    }

    pub fn apply_height_intensity(&self, color: Color, height: u8) -> Color {
        let normalized_height = height as f32 / 255.0;
        let intensity = self.height_intensity[0] 
            + normalized_height * (self.height_intensity[1] - self.height_intensity[0]);
        
        let rgba = color.to_srgba();
        Color::srgba(rgba.red * intensity, rgba.green * intensity, rgba.blue * intensity, rgba.alpha)
    }
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

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            window: WindowConfig::default(),
            starcraft: StarcraftConfig::default(),
            config_path: PathBuf::from("config/entities.toml"),
            data_source: DataSource::default(),
            entity_display: EntityDisplayConfig::default(),
            unit_by_id: std::collections::HashMap::new(),
            game_config_panel: GameConfigPanelDefaults::default(),
        }
    }
}

pub fn load_settings() -> AppSettings {
    // Load main config from config.toml
    let mut settings: AppSettings = config::Config::builder()
        .add_source(config::File::with_name("config").required(false))
        .build()
        .and_then(|c| c.try_deserialize())
        .unwrap_or_else(|e| {
            eprintln!("[config] Failed to load config.toml: {}, using defaults", e);
            AppSettings::default()
        });

    // Set config path for entities
    settings.config_path = PathBuf::from("config/entities.toml");

    // Read entities.toml for both entity display and map/style configuration
    let entities_config = config::Config::builder()
        .add_source(config::File::with_name(settings.config_path.to_str().unwrap()).required(false))
        .build()
        .unwrap_or_default();

    // Extract entity_display config (which now includes map config)
    let entity_display: EntityDisplayConfig = entities_config
        .clone()
        .try_deserialize()
        .unwrap_or_default();

    settings.entity_display = entity_display.clone();

    // Load data.json
    let data_json = std::fs::read_to_string("data/data.json").unwrap_or_default();
    let data_source: DataSource = serde_json::from_str(&data_json).unwrap_or_default();

    // Build unit_by_id (empty for now, entities are loaded in EntitySystem)
    let unit_by_id = std::collections::HashMap::new();

    AppSettings {
        entity_display: settings.entity_display,
        data_source,
        unit_by_id,
        window: settings.window,
        starcraft: settings.starcraft,
        config_path: settings.config_path,
        game_config_panel: settings.game_config_panel,
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

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct WindowConfig {
    pub title: String,
    pub width: f32,
    pub height: f32,
    pub resizable: bool,
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            title: "SC2 View".to_string(),
            width: 1920.0,
            height: 1080.0,
            resizable: true,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct StarcraftConfig {
    pub upstream_url: String,
    pub upstream_port: u16,
    pub listen_url: String,
    pub listen_port: u16,
    pub image: String,
    pub container_name: String,
}

impl Default for StarcraftConfig {
    fn default() -> Self {
        Self {
            upstream_url: "ws://127.0.0.1".to_string(),
            upstream_port: 5555,
            listen_url: "127.0.0.1".to_string(),
            listen_port: 5000,
            image: "minimal-sc2:latest".to_string(),
            container_name: "aiurgaze-sc2".to_string(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct GameConfigPanelDefaults {
    pub game_type: Option<String>,
    pub map_name: Option<String>,
    pub player_name: Option<String>,
    pub ai_difficulty: Option<String>,
    pub ai_race: Option<String>,
    pub bot_name: Option<String>,
    pub disable_fog: Option<bool>,
    pub random_seed: Option<u32>,
    pub realtime: Option<bool>,
}
