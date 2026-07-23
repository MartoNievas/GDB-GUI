//! Commands panel content — extracted from app.rs in slice S1.

use eframe::egui::{self, Color32, Stroke, Vec2};

use crate::ui::app::App;
use crate::ui::command::Command;
use crate::ui::theme::*;
use crate::ui::widgets::{hl, m, sec_hdr};

pub(crate) fn render(app: &mut App, ui: &mut egui::Ui) {
    sec_hdr(ui, "Commands", &mut app.open_cmd);
    if app.open_cmd {
        for cmd_str in &["info locals", "bt full", "info registers", "info threads"] {
            if ui
                .add(
                    egui::Button::new(m(cmd_str, 11.0, TXT_CYAN))
                        .fill(Color32::TRANSPARENT)
                        .stroke(Stroke::NONE)
                        .min_size(Vec2::new(ui.available_width(), 18.0)),
                )
                .clicked()
            {
                app.send(Command::Raw(cmd_str.to_string()));
            }
        }
        ui.add_space(4.0);
    }
    hl(ui);
}
