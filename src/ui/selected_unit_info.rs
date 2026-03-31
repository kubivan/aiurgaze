use crate::units::{
    get_set_fields, CurrentOrderAbility, SelectedUnit, UnitProto, UnitRegistry, UnitTag, UnitType,
};
use bevy::prelude::*;
use bevy_egui::egui;

pub(crate) fn render_selected_unit_info(
    ui: &mut egui::Ui,
    selected: &Res<SelectedUnit>,
    registry: &Res<UnitRegistry>,
    unit_query: &Query<(&UnitProto, &UnitTag, &CurrentOrderAbility, &UnitType)>,
) {
    ui.separator();
    ui.heading("Selected Unit Info");
    ui.separator();

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

    egui::CollapsingHeader::new("Unit Details")
        .default_open(true)
        .show(ui, |ui| {
            ui.label(format!("Tag: {}", unit_tag.0));
            ui.separator();
            for (field, value) in get_set_fields(&unit_proto.0) {
                ui.label(format!("{}: {}", field, value));
            }
        });
}
