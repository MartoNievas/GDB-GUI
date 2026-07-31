//! Struct panel content — extracted from app.rs in slice S1.

use eframe::egui::{self, FontId, RichText, TextEdit};

use crate::ui::app::App;
use crate::ui::command::Command;
use crate::ui::theme::{TXT, TXT_DIM};
use crate::ui::widgets::{hl, sec_hdr};

pub(crate) fn render(app: &mut App, ui: &mut egui::Ui) {
    let mut open = app.panels.contains(crate::ui::app::PanelState::STRUCT);
    sec_hdr(ui, "Struct", &mut open);
    app.panels.set(crate::ui::app::PanelState::STRUCT, open);
    if open {
        let resp = ui
            .horizontal(|ui| {
                ui.add_space(8.0);
                ui.add(
                    TextEdit::singleline(&mut app.struct_input)
                        .font(FontId::monospace(11.0))
                        .desired_width(160.0)
                        .hint_text("expression"),
                )
            })
            .inner;

        if should_commit_struct_expr(resp.lost_focus(), &app.struct_input, &app.state.struct_expr)
        {
            let expr = app.struct_input.clone();
            app.state.struct_expr = expr.clone();
            app.state.struct_value = None;
            if !expr.is_empty() {
                app.send(Command::Evaluate(expr));
            }
        }

        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            match &app.state.struct_value {
                Some(value) => {
                    ui.label(RichText::new(value).color(TXT).font(FontId::monospace(11.0)));
                }
                None => {
                    ui.label(
                        RichText::new("No struct selected")
                            .color(TXT_DIM)
                            .font(FontId::monospace(11.0))
                            .italics(),
                    );
                }
            }
        });
        ui.add_space(4.0);
    }
    hl(ui);
}

// ─── Struct expression commit decision ────────────────────────────────────────

/// Pure decision for the struct panel input: `Command::Evaluate` must be sent
/// only when the field just lost focus (Enter or blur) AND the edited text
/// actually differs from the last committed expression. Never fires per
/// keystroke, and never sends a redundant no-op when the user clicks away
/// unchanged. Unlike `should_commit_breakpoint_condition`, `committed` is a
/// bare `&str` (not `Option<String>`) — the struct panel has no keyed rows,
/// just one always-present committed expression (`""` = none yet).
pub(crate) fn should_commit_struct_expr(lost_focus: bool, buffer: &str, committed: &str) -> bool {
    if !lost_focus {
        return false;
    }
    buffer != committed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn never_commits_while_still_focused_mid_keystroke() {
        assert!(!should_commit_struct_expr(false, "my_struct", ""));
        assert!(!should_commit_struct_expr(
            false,
            "my_struct.field",
            "my_struct"
        ));
    }

    #[test]
    fn commits_on_lost_focus_when_text_changed() {
        assert!(should_commit_struct_expr(true, "my_struct", ""));
        assert!(should_commit_struct_expr(
            true,
            "my_struct.field",
            "my_struct"
        ));
    }

    #[test]
    fn skips_redundant_commit_when_lost_focus_but_text_unchanged() {
        assert!(!should_commit_struct_expr(true, "my_struct", "my_struct"));
        assert!(!should_commit_struct_expr(true, "", ""));
    }

    #[test]
    fn commits_on_lost_focus_when_buffer_emptied() {
        assert!(should_commit_struct_expr(true, "", "my_struct"));
    }
}
