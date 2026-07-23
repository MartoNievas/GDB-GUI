//! Files panel content — extracted from app.rs in slice S1.

use eframe::egui;

use crate::ui::app::App;
use crate::ui::theme::*;
use crate::ui::widgets::{hl, m, sec_hdr};

pub(crate) fn render(app: &mut App, ui: &mut egui::Ui) {
    sec_hdr(ui, "Files", &mut app.open_files);
    if app.open_files {
        if let Some(exe) = &app.state.persistent.executable {
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                ui.label(m(&format!("📄 {exe}"), 11.0, TXT_CYAN));
            });
        }
        ui.add_space(4.0);
    }
    hl(ui);
}
