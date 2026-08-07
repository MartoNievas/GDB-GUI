//! Remote-connect panel content — host:port input + Connect button (design
//! D1-D5). Mirrors `panels::attach`'s buffer-field + button pattern: a
//! `String` buffer bound to a `TextEdit::singleline`, parsed on submit via
//! `parse_remote_target`, plus a persistent (never a transient toast) line
//! for `remote_connect_error`.

use eframe::egui::{self, TextEdit};

use crate::state::{ProgramState, parse_remote_target};
use crate::ui::app::App;
use crate::ui::command::Command;
use crate::ui::theme::*;
use crate::ui::widgets::{hl, m, sec_hdr};

pub(crate) fn render(app: &mut App, ui: &mut egui::Ui) {
    let mut open = app.panels.contains(crate::ui::app::PanelState::REMOTE);
    sec_hdr(ui, "Remote", &mut open);
    app.panels.set(crate::ui::app::PanelState::REMOTE, open);
    if open {
        ui.horizontal(|ui| {
            ui.add(
                TextEdit::singleline(&mut app.remote_target_buffer)
                    .font(egui::FontId::monospace(11.0))
                    .desired_width(140.0)
                    .hint_text("host:port"),
            );

            let can_connect = remote_connect_enabled(
                &app.state.program,
                app.state.attached_pid,
                app.state.remote_target.as_deref(),
                &app.remote_target_buffer,
            );
            if ui
                .add_enabled(can_connect, egui::Button::new(m("Connect", 12.0, ACCENT)))
                .clicked()
            {
                if let Some(target) = parse_remote_target(&app.remote_target_buffer) {
                    app.send(Command::ConnectRemote { target });
                    app.remote_target_buffer.clear();
                }
            }
        });

        // Persistent error line (mirrors attach.rs's `attach_error` slot —
        // a rejected connect never got a row of its own to attach the
        // error to).
        if let Some(err) = &app.state.errors.remote_connect_error {
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

/// D1: pure gating predicate for the Connect button. Enabled iff `program`
/// is `NoProgramLoaded` or `ProgramLoaded` (symbols loaded, nothing running
/// yet — the proposal's `NoProgramLoaded`-only gate would make
/// `gdb-gui ./firmware.elf` unable to reach a remote target at all) AND no
/// local attach is active AND no remote target is already connected AND
/// `buffer` parses via `parse_remote_target`.
pub(crate) fn remote_connect_enabled(
    program: &ProgramState,
    attached_pid: Option<u32>,
    remote_target: Option<&str>,
    buffer: &str,
) -> bool {
    matches!(program, ProgramState::NoProgramLoaded | ProgramState::ProgramLoaded)
        && attached_pid.is_none()
        && remote_target.is_none()
        && parse_remote_target(buffer).is_some()
}

#[cfg(test)]
mod tests {
    use crate::state::ProgramState;

    #[test]
    fn remote_connect_enabled_true_when_no_program_loaded_and_valid_target() {
        assert!(super::remote_connect_enabled(
            &ProgramState::NoProgramLoaded,
            None,
            None,
            "localhost:1234",
        ));
    }

    // D1 corrects the proposal: `ProgramLoaded` (symbols loaded, nothing
    // running yet) must also allow connect — otherwise `gdb-gui
    // ./firmware.elf` could never reach a remote target.
    #[test]
    fn remote_connect_enabled_true_when_program_loaded_and_valid_target() {
        assert!(super::remote_connect_enabled(
            &ProgramState::ProgramLoaded,
            None,
            None,
            "localhost:1234",
        ));
    }

    #[test]
    fn remote_connect_enabled_false_when_running() {
        assert!(!super::remote_connect_enabled(
            &ProgramState::Running,
            None,
            None,
            "localhost:1234",
        ));
    }

    #[test]
    fn remote_connect_enabled_false_when_paused() {
        assert!(!super::remote_connect_enabled(
            &ProgramState::Paused,
            None,
            None,
            "localhost:1234",
        ));
    }

    #[test]
    fn remote_connect_enabled_false_when_exited() {
        assert!(!super::remote_connect_enabled(
            &ProgramState::Exited { code: Some(0) },
            None,
            None,
            "localhost:1234",
        ));
    }

    #[test]
    fn remote_connect_enabled_false_when_attached_pid_set() {
        assert!(!super::remote_connect_enabled(
            &ProgramState::NoProgramLoaded,
            Some(4242),
            None,
            "localhost:1234",
        ));
    }

    #[test]
    fn remote_connect_enabled_false_when_program_state_is_attached_variant() {
        assert!(!super::remote_connect_enabled(
            &ProgramState::Attached { pid: 1 },
            None,
            None,
            "localhost:1234",
        ));
    }

    #[test]
    fn remote_connect_enabled_false_when_already_connected() {
        assert!(!super::remote_connect_enabled(
            &ProgramState::NoProgramLoaded,
            None,
            Some("localhost:1234"),
            "otherhost:1234",
        ));
    }

    #[test]
    fn remote_connect_enabled_false_when_program_state_is_remote_connected_variant() {
        assert!(!super::remote_connect_enabled(
            &ProgramState::RemoteConnected {
                target: "localhost:1234".into(),
            },
            None,
            Some("localhost:1234"),
            "otherhost:1234",
        ));
    }

    #[test]
    fn remote_connect_enabled_false_when_buffer_does_not_parse() {
        assert!(!super::remote_connect_enabled(
            &ProgramState::NoProgramLoaded,
            None,
            None,
            "not-a-target",
        ));
        assert!(!super::remote_connect_enabled(
            &ProgramState::NoProgramLoaded,
            None,
            None,
            "",
        ));
        assert!(!super::remote_connect_enabled(
            &ProgramState::NoProgramLoaded,
            None,
            None,
            "localhost:0",
        ));
        assert!(!super::remote_connect_enabled(
            &ProgramState::NoProgramLoaded,
            None,
            None,
            "localhost:1234\n-exec-run",
        ));
    }
}
