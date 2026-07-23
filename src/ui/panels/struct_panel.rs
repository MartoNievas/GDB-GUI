//! Struct panel content — extracted from app.rs in slice S1.

use eframe::egui::{self, FontId, RichText};

use crate::ui::app::App;
use crate::ui::theme::TXT_DIM;
use crate::ui::widgets::{hl, sec_hdr};

pub(crate) fn render(app: &mut App, ui: &mut egui::Ui) {
    sec_hdr(ui, "Struct", &mut app.open_struct);
    if app.open_struct {
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            ui.label(
                RichText::new("No struct selected")
                    .color(TXT_DIM)
                    .font(FontId::monospace(11.0))
                    .italics(),
            );
        });
        ui.add_space(4.0);
    }
    hl(ui);
}
