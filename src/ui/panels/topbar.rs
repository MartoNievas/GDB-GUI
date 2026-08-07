//! Top bar content — extracted from app.rs in slice S4.

use eframe::egui::{self, Align, Layout, Sense, Vec2};

use super::util::{attached_status_label, location_label, remote_status_label};
use crate::ui::app::App;
use crate::ui::command::Command;
use crate::ui::theme::*;
use crate::ui::widgets::{m, tbtn};

pub(crate) fn render(app: &mut App, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        ui.label(m("GDB GUI", 13.0, ACCENT).strong());
        ui.add(egui::Separator::default().vertical());

        if tbtn(ui, "Run", true).clicked() {
            app.send(Command::Run);
        }
        if tbtn(ui, "Continue", false).clicked() {
            app.send(Command::Continue);
        }
        if tbtn(ui, "Step", false).clicked() {
            app.send(Command::Step);
        }
        if tbtn(ui, "Next", false).clicked() {
            app.send(Command::Next);
        }
        if tbtn(ui, "Finish", false).clicked() {
            app.send(Command::Finish);
        }
        if tbtn(ui, "Interrupt", false).clicked() {
            app.send(Command::Interrupt);
        }
        if tbtn(ui, "Restart", false).clicked() {
            app.send(Command::Restart);
        }

        ui.add(egui::Separator::default().vertical());

        // Phase 3 task 3.11 (spec "Persistence Enable/Disable Setting"):
        // controls both the save hook and the load/restore hook — a full
        // opt-out, not just a save-side toggle.
        let mut persistence_enabled = app.settings.persistence_enabled;
        if ui
            .checkbox(&mut persistence_enabled, m("Persist", 12.0, TXT_MUTED))
            .on_hover_text("Save/restore breakpoints, watchpoints, and catchpoints across sessions")
            .changed()
        {
            app.set_persistence_enabled(persistence_enabled);
        }

        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let (r, _) = ui.allocate_exact_size(Vec2::splat(12.0), Sense::hover());
            let color = if app.state.is_running() {
                ACCENT
            } else if app.state.is_paused() {
                TXT_YELLOW
            } else {
                TXT_DIM
            };
            ui.painter().rect_filled(r, 2.0, color);
            ui.add_space(6.0);

            let status = match &app.state.program {
                crate::state::ProgramState::NoProgramLoaded => "No program loaded",
                crate::state::ProgramState::ProgramLoaded => "Loaded",
                crate::state::ProgramState::Running => "Running",
                crate::state::ProgramState::Paused => "Paused",
                crate::state::ProgramState::Exited { .. } => "Exited",
                // Transient by construction (`ProgramState::Attached`'s doc
                // comment) — `attached_pid` below is what actually keeps
                // this label alive past the immediate `*stopped` -> Paused
                // overwrite, so this arm's text is only ever seen in the
                // brief pre-pause window.
                crate::state::ProgramState::Attached { .. } => "Attached",
                // Transient by construction (`ProgramState::RemoteConnected`'s
                // doc comment), mirroring `Attached { .. }` above —
                // `remote_target` below is what actually keeps the connected
                // label alive past the immediate `*stopped` -> Paused
                // overwrite, so this arm's text is only ever seen in the
                // brief pre-pause window (design D1/D3/D4, task 9.1).
                crate::state::ProgramState::RemoteConnected { .. } => "Connected",
            };

            // `attached_pid`/`remote_target` (not the `program` match above)
            // drive the attached/connected labels: `program` overwrites to
            // `Paused` almost immediately after a successful attach or
            // connect, but these fields survive that transition (design D2,
            // D1/D3/D4) — reading them here is what keeps "Attached (pid N)"/
            // "Remote host:port" visible for the whole session instead of
            // only the initial instant. Mutually exclusive by construction
            // (`attach_enabled`/`remote_connect_enabled` each require the
            // other to be absent), so checking `attached_pid` first is safe.
            let location = if let Some(pid) = app.state.attached_pid {
                attached_status_label(pid, status)
            } else if let Some(target) = app.state.remote_target.as_deref() {
                remote_status_label(target, status)
            } else {
                location_label(app.state.current_file(), app.state.current_function(), status)
            };

            ui.label(m(&location, 11.0, TXT_MUTED));
        });
    });
}
