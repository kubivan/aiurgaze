use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};
use crossbeam_channel::Receiver;

// === Messages from proxy to renderer ===
#[derive(Debug, Clone)]
pub enum ProxyToRenderMsg {
    MapLayers(MapLayers),
    Units(Vec<UnitMarker>),
}


#[derive(Debug, Clone)]
pub struct MapLayers {
    pub width: u32,
    pub height: u32,
    /// Unpacked bytes (0/1) per cell
    pub pathing: Vec<u8>,
    pub placement: Vec<u8>,
}


#[derive(Debug, Clone, Copy)]
pub struct UnitMarker {
    pub x: f32,
    pub y: f32,
    pub owner: u32,
}


#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GridLayerKind { Pathing, Placement }


// === Bevy resources ===
#[derive(Resource)]
pub struct ProxyStateRx(pub Receiver<ProxyToRenderMsg>);


#[derive(Resource)]
struct UiState {
    layer: GridLayerKind,
    cell_px: f32,
}


#[derive(Resource)]
struct MapGpu {
    dims: UVec2,
    pathing_image: Handle<Image>,
    placement_image: Handle<Image>,
}


#[derive(Component)]
struct MapQuad;


#[derive(Component)]
struct UnitsLayer;


pub struct MapRenderPlugin;
impl Plugin for MapRenderPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(UiState { layer: GridLayerKind::Pathing, cell_px: 4.0 })
            .add_systems(Startup, setup)
            .add_systems(Update, (ui_layer_selector, pump_proxy_msgs, draw_units, pan_zoom_camera));
    }
}


fn setup(mut commands: Commands, mut images: ResMut<Assets<Image>>) {
    commands.spawn(Camera2dBundle::default());


    // Placeholder 1×1 textures until proxy sends real grids
    let placeholder = || {
        let mut img = Image::new_fill(
            Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
            TextureDimension::D2,
            &[255, 255, 255, 255],
            bevy::render::texture::ImageType::Extension("png".into()),
        );
        img.sampler_descriptor = ImageSampler::nearest();
        images.add(img)
    };


    let pathing_image = placeholder();
    let placement_image = placeholde