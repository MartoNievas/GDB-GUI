//! Breakpoints panel content — extracted from app.rs in slice S2.

use eframe::egui::{self, Color32, FontId, Stroke, TextEdit};

use super::util::short_file_name;
use crate::ui::app::App;
use crate::ui::command::Command;
use crate::ui::theme::*;
use crate::ui::widgets::{hl, m, sec_hdr};

pub(crate) fn render(app: &mut App, ui: &mut egui::Ui) {
    sec_hdr(ui, "Breakpoints", &mut app.open_bp);
    if app.open_bp {
        let mut pending_commits: Vec<(u32, String)> = Vec::new();

        egui::Grid::new("bp_grid")
            .num_columns(5)
            .spacing([8.0, 2.0])
            .show(ui, |ui| {
                for h in ["File", "Line", "Condition", "", ""] {
                    ui.label(m(h, 11.0, TXT_DIM));
                }
                ui.end_row();

                for bp in &app.state.persistent.breakpoints {
                    // Nombre corto del archivo
                    let short_file = short_file_name(&bp.file);

                    ui.label(m(short_file, 12.0, TXT_CYAN));
                    ui.label(m(&bp.line.to_string(), 12.0, TXT_YELLOW));

                    let buffer = app
                        .bp_cond_buffer
                        .entry(bp.id)
                        .or_insert_with(|| bp.condition.clone().unwrap_or_default());

                    let resp = ui.add(
                        TextEdit::singleline(buffer)
                            .font(FontId::monospace(11.0))
                            .desired_width(120.0)
                            .hint_text("condition"),
                    );
                    if should_commit_breakpoint_condition(resp.lost_focus(), buffer, &bp.condition)
                    {
                        pending_commits.push((bp.id, buffer.clone()));
                    }

                    if bp.condition_error.is_some() {
                        ui.label(m("⚠", 12.0, RED))
                            .on_hover_text(bp.condition_error.as_deref().unwrap_or(""));
                    } else {
                        ui.label("");
                    }

                    if ui
                        .add(
                            egui::Button::new(m("×", 12.0, RED))
                                .fill(Color32::TRANSPARENT)
                                .stroke(Stroke::NONE),
                        )
                        .clicked()
                    {
                        app.send(Command::RemoveBreakpoint(bp.id));
                    }
                    ui.end_row();
                }
            });

        for (id, condition) in pending_commits {
            app.send(Command::SetBreakpointCondition { id, condition });
        }

        ui.add_space(4.0);
    }
    hl(ui);
}

// ─── Breakpoint condition commit decision ─────────────────────────────────────

/// Pure decision for the Condition cell: a `SetBreakpointCondition` command
/// must be sent only when the field just lost focus (covers both Enter and
/// blur — egui's `lost_focus()` fires once for either) AND the edited text
/// actually differs from the breakpoint's last known committed condition.
/// Never fires per keystroke (`lost_focus == false` while typing), and never
/// sends a redundant no-op command when the user clicks away unchanged.
pub(crate) fn should_commit_breakpoint_condition(
    lost_focus: bool,
    buffer_text: &str,
    committed_condition: &Option<String>,
) -> bool {
    if !lost_focus {
        return false;
    }
    let committed = committed_condition.as_deref().unwrap_or("");
    buffer_text != committed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn never_commits_while_still_focused_mid_keystroke() {
        // Simulates typing: field retains focus for every intermediate
        // keystroke, so lost_focus() is false and no command must be sent —
        // regardless of how much the buffer has diverged from committed.
        assert!(!should_commit_breakpoint_condition(false, "i == 1", &None));
        assert!(!should_commit_breakpoint_condition(
            false,
            "i == 10",
            &Some("i == 1".to_string())
        ));
    }

    #[test]
    fn commits_exactly_once_on_lost_focus_when_text_changed() {
        // Enter or blur -> lost_focus() true, and the text actually changed.
        assert!(should_commit_breakpoint_condition(true, "count > 3", &None));
        assert!(should_commit_breakpoint_condition(
            true,
            "x == 2",
            &Some("x == 1".to_string())
        ));
    }

    #[test]
    fn skips_redundant_commit_when_lost_focus_but_text_unchanged() {
        // Clicking into the cell and back out without editing must not send
        // a no-op SetBreakpointCondition.
        assert!(!should_commit_breakpoint_condition(
            true,
            "x == 1",
            &Some("x == 1".to_string())
        ));
        assert!(!should_commit_breakpoint_condition(true, "", &None));
    }
}
