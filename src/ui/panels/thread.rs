//! Thread panel content — extracted from app.rs in slice S1.

use eframe::egui::{self, Sense, Vec2};

use crate::ui::app::App;
use crate::ui::theme::*;
use crate::ui::widgets::{hl, m, sec_hdr};

pub(crate) fn render(app: &mut App, ui: &mut egui::Ui) {
    sec_hdr(ui, "Thread", &mut app.open_thread);
    if app.open_thread {
        if let Some(pause) = &app.state.pause {
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                let (r, _) = ui.allocate_exact_size(Vec2::splat(8.0), Sense::hover());
                ui.painter().circle_filled(r.center(), 4.0, ACCENT);
                ui.add_space(4.0);
                ui.label(m(&format!("Thread {}", pause.thread_id), 11.0, TXT_MUTED));
            });
        }
        ui.add_space(4.0);
    }
    hl(ui);
}
