//! Thread panel content — extracted from app.rs in slice S1.
//!
//! Slice 1 (read-only) rendered one row per `DebuggerState.threads` entry,
//! highlighting the current thread. Slice 2 adds interactivity: rows are
//! clickable only while paused, dispatching `Command::SelectThread`.

use eframe::egui::{self, Sense, Stroke, Vec2};

use crate::state::{DebuggerState, ThreadInfo};
use crate::ui::app::App;
use crate::ui::command::Command;
use crate::ui::theme::*;
use crate::ui::widgets::{hl, m, sec_hdr};

/// Whether thread rows are interactive (clickable) right now. GDB does not
/// read its stdin while the inferior runs, so there is no dispatch-side gate
/// to add — this UI-side gate alone is sufficient (design.md: "Paused-only
/// gate").
pub(crate) fn rows_interactive(state: &DebuggerState) -> bool {
    state.is_paused()
}

pub(crate) fn render(app: &mut App, ui: &mut egui::Ui) {
    let mut open = app.panels.contains(crate::ui::app::PanelState::THREAD);
    sec_hdr(ui, "Thread", &mut open);
    app.panels.set(crate::ui::app::PanelState::THREAD, open);
    if open {
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
                // across the loop body (precedent: `global_names.clone()`),
                // which would otherwise conflict with `app.send` below.
                let threads = app.state.threads.clone();
                let current = app.state.current_thread;
                let interactive = rows_interactive(&app.state);
                for t in &threads {
                    let is_current = current == Some(t.id);
                    let resp = thread_row(ui, t, is_current, interactive);
                    if interactive && resp.clicked() && !is_current {
                        app.send(Command::SelectThread(t.id));
                    }
                }
            }
        }
        ui.add_space(4.0);
    }
    hl(ui);
}

/// Renders one thread roster row. Clickable (`Sense::click()`, `PointingHand`
/// cursor on hover) only while `interactive` — otherwise `Sense::hover()`
/// only, mirroring `source_row`'s precedent (app.rs:331).
fn thread_row(
    ui: &mut egui::Ui,
    t: &ThreadInfo,
    is_current: bool,
    interactive: bool,
) -> egui::Response {
    let sense = if interactive {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 18.0), sense);

    if interactive && response.hovered() {
        ui.ctx()
            .output_mut(|o| o.cursor_icon = egui::CursorIcon::PointingHand);
    }

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{DebuggerState, ProgramState};

    #[test]
    fn rows_interactive_false_unless_paused() {
        let mut state = DebuggerState::new();

        state.program = ProgramState::NoProgramLoaded;
        assert!(!rows_interactive(&state));

        state.program = ProgramState::ProgramLoaded;
        assert!(!rows_interactive(&state));

        state.program = ProgramState::Running;
        assert!(!rows_interactive(&state));

        state.program = ProgramState::Exited { code: Some(0) };
        assert!(!rows_interactive(&state));

        state.program = ProgramState::Paused;
        assert!(rows_interactive(&state));
    }
}
