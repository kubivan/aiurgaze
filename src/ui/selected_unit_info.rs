use crate::units::{
    CurrentOrderAbility, SelectedUnit, UnitFrameData, UnitRegistry, UnitTag, UnitType,
};
use bevy::prelude::*;
use bevy_egui::egui;

pub(crate) fn render_selected_unit_info(
    ui: &mut egui::Ui,
    selected: &Res<SelectedUnit>,
    registry: &Res<UnitRegistry>,
    unit_query: &Query<(&UnitFrameData, &UnitTag, &CurrentOrderAbility, &UnitType)>,
) {
    egui::CollapsingHeader::new(
        egui::RichText::new("SELECTED UNIT")
            .color(egui::Color32::from_rgb(205, 224, 238))
            .size(11.0)
            .strong(),
    )
    .default_open(false)
    .show(ui, |ui| {
        let Some(tag) = selected.tag else {
            ui.label("No unit selected.");
            return;
        };

        let Some(&entity) = registry.map.get(&tag) else {
            ui.label("No unit selected.");
            return;
        };

        let Ok((unit_proto, unit_tag, _, _)) = unit_query.get(entity) else {
            ui.label("Unit data not found.");
            return;
        };

        ui.label(format!("Tag: {}", unit_tag.0));
        ui.separator();
        for (field, value) in &unit_proto.0.display_fields {
            ui.label(format!("{}: {}", field, value));
        }
    });
}
