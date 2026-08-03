//! Restore report modal (Phase 3 task 3.10).
//!
//! Shown only when `App.restore_report` holds a non-empty list of failures
//! from the most recently finalized restore replay (design decision D8:
//! failed entries are reported to the user, never silently dropped from the
//! project file). Offers exactly two actions: `Keep` (dismiss, leaving the
//! failed entries on disk for a retry on the next launch) and
//! `Remove N failed` (explicitly rewrite the project file, which naturally
//! drops the failures since they were never added to in-memory state).

use eframe::egui::{self, Modal};

use crate::ui::app::App;
use crate::ui::theme::*;
use crate::ui::widgets::m;

pub(crate) fn render(app: &mut App, ctx: &egui::Context) {
    let count = match &app.restore_report {
        Some(failures) if !failures.is_empty() => failures.len(),
        _ => return,
    };

    let mut keep_clicked = false;
    let mut remove_clicked = false;

    Modal::new(egui::Id::new("restore_report_modal")).show(ctx, |ui| {
        ui.set_min_width(360.0);
        ui.label(
            m(
                &format!("{count} entr{} failed to restore:", if count == 1 { "y" } else { "ies" }),
                13.0,
                TXT_YELLOW,
            )
            .strong(),
        );
        ui.add_space(6.0);

        if let Some(failures) = &app.restore_report {
            for failure in failures {
                ui.horizontal(|ui| {
                    ui.label(m(&failure.label, 12.0, TXT_CYAN));
                    ui.label(m(": ", 12.0, TXT_DIM));
                    ui.label(m(&failure.message, 12.0, RED));
                });
            }
        }

        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui.button("Keep").clicked() {
                keep_clicked = true;
            }
            if ui.button(format!("Remove {count} failed")).clicked() {
                remove_clicked = true;
            }
        });
    });

    if keep_clicked {
        app.dismiss_restore_report();
    }
    if remove_clicked {
        app.remove_failed_restore_entries();
    }
}
