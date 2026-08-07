//! Attach panel content — PID input + Attach button (design D1-D5).
//!
//! Mirrors `panels::catchpoints`' buffer-field + Add-button pattern: a
//! `String` buffer bound to a `TextEdit::singleline`, parsed on submit, plus
//! a persistent (never a transient toast) line for `attach_error`.

use eframe::egui::{self, TextEdit};

use crate::state::ProgramState;
use crate::ui::app::App;
use crate::ui::command::Command;
use crate::ui::theme::*;
use crate::ui::widgets::{hl, m, sec_hdr};

pub(crate) fn render(app: &mut App, ui: &mut egui::Ui) {
    let mut open = app.panels.contains(crate::ui::app::PanelState::ATTACH);
    sec_hdr(ui, "Attach", &mut open);
    app.panels.set(crate::ui::app::PanelState::ATTACH, open);
    if open {
        ui.horizontal(|ui| {
            ui.add(
                TextEdit::singleline(&mut app.attach_pid_buffer)
                    .font(egui::FontId::monospace(11.0))
                    .desired_width(80.0)
                    .hint_text("PID"),
            );

            let can_attach = attach_enabled(
                &app.state.program,
                app.state.attached_pid,
                &app.attach_pid_buffer,
            );
            if ui
                .add_enabled(can_attach, egui::Button::new(m("Attach", 12.0, ACCENT)))
                .clicked()
            {
                if let Ok(pid) = app.attach_pid_buffer.trim().parse::<u32>() {
                    app.send(Command::AttachToProcess(pid));
                    app.attach_pid_buffer.clear();
                }
            }
        });

        // Persistent error line (mirrors catchpoints.rs's "Pending Errors"
        // section / D1: a rejected attach never got a row of its own to
        // attach the error to — this is the single slot for it).
        if let Some(err) = &app.state.errors.attach_error {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                ui.label(m(err, 11.0, RED));
            });
        }

        ui.add_space(4.0);
    }
    hl(ui);
}

/// D3: pure gating predicate for the Attach button. Enabled iff no program
/// is loaded/attached (all of `ProgramLoaded`/`Running`/`Paused`/`Exited`/
/// `Attached` disable it) AND `attached_pid` is `None` AND `buffer` parses
/// to a non-zero `u32`. `u32::from_str` on its own already rejects
/// non-numeric content, negative numbers, overflow, and any embedded
/// whitespace/newline (the MI-injection threat-matrix row) — no separate
/// sanitizer is needed.
pub(crate) fn attach_enabled(program: &ProgramState, attached_pid: Option<u32>, buffer: &str) -> bool {
    matches!(program, ProgramState::NoProgramLoaded)
        && attached_pid.is_none()
        && buffer.trim().parse::<u32>().is_ok_and(|pid| pid != 0)
}

// The full RED-first gating predicate truth table (task 4.3) lives in
// `app.rs`'s test module, next to the rest of the attach-cascade tests —
// see `attach_enabled_truth_table`.
