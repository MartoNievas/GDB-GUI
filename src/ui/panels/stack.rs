//! Stack panel content — extracted from app.rs in slice S1.

use eframe::egui::{self, Sense, Vec2};

use super::util::frame_location;
use crate::ui::app::App;
use crate::ui::theme::*;
use crate::ui::widgets::{hl, m, sec_hdr};

pub(crate) fn render(app: &mut App, ui: &mut egui::Ui) {
    sec_hdr(ui, "Stack", &mut app.open_stack);
    if app.open_stack {
        if let Some(pause) = &app.state.pause {
            egui::Grid::new("stack_grid")
                .num_columns(3)
                .spacing([6.0, 2.0])
                .show(ui, |ui| {
                    for h in ["#", "Function", "Location"] {
                        ui.label(m(h, 11.0, TXT_DIM));
                    }
                    ui.end_row();

                    for (idx, frame) in pause.stack.iter().enumerate() {
                        let active = idx == 0;

                        let (stripe, _) =
                            ui.allocate_exact_size(Vec2::new(2.0, 14.0), Sense::hover());
                        if active {
                            ui.painter().rect_filled(stripe, 0.0, BLUE);
                        }

                        let fn_col = if active { BLUE } else { TXT_CYAN };
                        ui.label(m(&idx.to_string(), 11.0, TXT_DIM));
                        ui.label(m(&frame.function, 11.0, fn_col));

                        let loc = frame_location(frame.file.as_deref(), frame.line, frame.addr);
                        ui.label(m(&loc, 11.0, TXT_MUTED));
                        ui.end_row();
                    }
                });
        } else {
            ui.label(m("Not paused", 11.0, TXT_DIM).italics());
        }
        ui.add_space(4.0);
    }
    hl(ui);
}
