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
    pub style: StyleConfig,
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
            window: WindowConfig::default(),
            starcraft: StarcraftConfig::default(),
            config_path: PathBuf::from("config/entities.toml"),
            data_source: DataSource::default(),
            entity_display: EntityDisplayConfig::default(),
            style: StyleConfig::default(),
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
        .unwrap_or_default();

    // Set config path for entities
    settings.config_path = PathBuf::from("config/entities.toml");

    // Read TOML (legacy) entities if exists
    let entity_display: EntityDisplayConfig = config::Config::builder()
        .add_source(config::File::with_name(settings.config_path.to_str().unwrap()).required(false))
        .build()
        .and_then(|c| c.try_deserialize().map_err(|e| e.into()))
        .unwrap_or_default();

    settings.entity_display = entity_display;

    // Load data.json
    let data_json = std::fs::read_to_string("data/data.json").unwrap_or_default();
    let data_source: DataSource = serde_json::from_str(&data_json).unwrap_or_default();

    // Prepare entities.json path
    let entities_json_path = std::path::Path::new("config/entities.json");
    let mut units_map: std::collections::HashMap<u32, DisplayInfo> = std::collections::HashMap::new();

    let tile_size = settings.style.tile_size;

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
    // for u in &data_source.unit {
    //     units_map.entry(u.id).and_modify(|info| {
    //         if info.radius.is_none() { info.radius = u.radius; }
    //         if info.size.is_none() { if let Some(r) = info.radius.or(u.radius) { info.size = Some(r * 2.0 * 16.0); } }
    //         if info.name.is_none() { info.name = Some(u.name.clone()); }
    //         // Fill missing radius/size/icon/label
    //         if info.label.is_none() { info.label = Some(u.name.clone()); }
    //         if info.icon.is_none() {
    //             let base = &u.name;
    //             if icon_name_set.contains(base) { info.icon = Some(format!("icons/{}.webp", base)); }
    //         }
    //     }).or_insert_with(|| {
    //         let mut info = DisplayInfo::default();
    //         info.name = Some(u.name.clone());
    //         info.label = Some(u.name.clone());
    //         info.radius = u.radius;
    //         if let Some(r) = u.radius { info.size = Some(r * 2.0 * tile_size); }
    //         let base = &u.name;
    //         if icon_name_set.contains(base) { info.icon = Some(format!("icons/{}.webp", base)); }
    //         info
    //     });
    // }

    // let entities_json = EntitiesJson { units: units_map.clone() };
    // if let Ok(serialized) = serde_json::to_string_pretty(&entities_json) {
    //     let _ = std::fs::create_dir_all("config");
    //     let _ = std::fs::write(entities_json_path, serialized);
    // }

    // Build unit_by_id (override legacy TOML) from units_map
    let unit_by_id = units_map;

    AppSettings {
        entity_display: settings.entity_display,
        data_source,
        unit_by_id,
        style: settings.style,
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
pub struct StyleConfig {
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

impl Default for StyleConfig {
    fn default() -> Self {
        Self {
            tile_size: 32.0,
            terrain_blocked: [0.05, 0.05, 0.05],
            terrain_pathable: [0.12, 0.12, 0.13],
            terrain_placeable: [0.18, 0.18, 0.20],
            terrain_both: [0.22, 0.22, 0.24],
            creep: [0.4, 0.1, 0.5],
            energy: [0.1, 0.3, 0.6],
            height_intensity: [0.6, 1.0],
        }
    }
}

impl StyleConfig {
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
        //color
        let intensity = self.height_intensity[0] + (height as f32 / 255.0) * (self.height_intensity[1] - self.height_intensity[0]);
        let srgba = color.to_srgba();
        Color::srgb(
            srgba.red * intensity,
            srgba.green * intensity,
            srgba.blue * intensity,
        )
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
    pub ws_url: String,
    pub ws_port: u16,
    pub image: String,
    pub container_name: String,
}

impl Default for StarcraftConfig {
    fn default() -> Self {
        Self {
            ws_url: "ws://127.0.0.1".to_string(),
            ws_port: 5555,
            image: "sc2:latest".to_string(),
            container_name: "sc2-tweak".to_string(),
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

