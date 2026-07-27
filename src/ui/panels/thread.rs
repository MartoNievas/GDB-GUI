//! Thread panel content — extracted from app.rs in slice S1.
//!
//! Slice 1 (read-only): renders one row per `DebuggerState.threads` entry,
//! highlighting the current thread. Rows are not yet interactive — clicking
//! and dispatching `Command::SelectThread` lands in slice 2.

use eframe::egui::{self, Sense, Stroke, Vec2};

use crate::state::ThreadInfo;
use crate::ui::app::App;
use crate::ui::theme::*;
use crate::ui::widgets::{hl, m, sec_hdr};

pub(crate) fn render(app: &mut App, ui: &mut egui::Ui) {
    sec_hdr(ui, "Thread", &mut app.open_thread);
    if app.open_thread {
        if let Some(pause) = &app.state.pause {
            if app.state.threads.is_empty() {
                // Fallback: no roster fetched yet (or -thread-info hasn't
                // replied), keep today's single-line display.
                let thread_id = pause.thread_id;
                ui.horizontal(|ui| {
                    ui.add_space(8.0);
                    let (r, _) = ui.allocate_exact_size(Vec2::splat(8.0), Sense::hover());
                    ui.painter().circle_filled(r.center(), 4.0, ACCENT);
                    ui.add_space(4.0);
                    ui.label(m(&format!("Thread {thread_id}"), 11.0, TXT_MUTED));
                });
            } else {
                // Clone to avoid holding an immutable borrow of `app.state`
                // across the loop body (precedent: `global_names.clone()`).
                let threads = app.state.threads.clone();
                let current = app.state.current_thread;
                for t in &threads {
                    thread_row(ui, t, current == Some(t.id));
                }
            }
        }
        ui.add_space(4.0);
    }
    hl(ui);
}

/// Renders one thread roster row. Not yet clickable this slice — always
/// allocated with `Sense::hover()`; the paused-only click gate arrives in
/// slice 2.
fn thread_row(ui: &mut egui::Ui, t: &ThreadInfo, is_current: bool) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 18.0), Sense::hover());

    let p = ui.painter();

    if is_current {
        p.rect_filled(rect, 0.0, BG_LINE_HL);
        p.line_segment(
            [rect.left_top(), rect.left_bottom()],
            Stroke::new(2.0_f32, ACCENT),
        );
    }

    p.text(
        rect.left_center() + Vec2::new(8.0, 0.0),
        egui::Align2::LEFT_CENTER,
        format!("Thread {} — {}", t.id, t.target_id),
        egui::FontId::monospace(11.0),
        TXT_MUTED,
    );

    response
}
