//! Watchpoints panel content.
//!
//! Distinct from `ui::app` (the unrelated `WatchTab::Watch`
//! variable-inspection tab, design decision D7): this panel is a new
//! collapsible section, rendered immediately after `panels::breakpoints`.

use eframe::egui::{self, Color32, ComboBox, FontId, Stroke, TextEdit};

use crate::state::{StopReason, Watchpoint, WatchpointKind};
use crate::ui::app::App;
use crate::ui::command::Command;
use crate::ui::theme::*;
use crate::ui::widgets::{hl, m, sec_hdr};

pub(crate) fn render(app: &mut App, ui: &mut egui::Ui) {
    sec_hdr(ui, "Watchpoints", &mut app.open_wp);
    if app.open_wp {
        // CRITICAL-3: watchpoint-trigger old/new values (spec: "Trigger:
        // show old/new values") — a stop reason is transient (overwritten by
        // the next pause), so it is rendered as a banner rather than
        // attached to a row.
        let stop_reason = app.state.pause.as_ref().map(|p| &p.stop_reason);
        if let Some(banner) = trigger_banner_text(stop_reason) {
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                ui.label(m(&format!("⚡ {banner}"), 11.0, TXT_YELLOW));
            });
            ui.add_space(2.0);
        }

        let mut pending_toggles: Vec<(u32, bool)> = Vec::new();
        let mut pending_removes: Vec<u32> = Vec::new();

        egui::Grid::new("wp_grid")
            .num_columns(5)
            .spacing([8.0, 2.0])
            .show(ui, |ui| {
                for h in ["Expr", "Kind", "On", "", ""] {
                    ui.label(m(h, 11.0, TXT_DIM));
                }
                ui.end_row();

                for wp in &app.state.persistent.watchpoints {
                    ui.label(m(&wp.expr, 12.0, TXT_CYAN));
                    ui.label(m(kind_label(wp.kind), 12.0, TXT_YELLOW));

                    let mut enabled = wp.enabled;
                    if ui.checkbox(&mut enabled, "").changed() {
                        pending_toggles.push((wp.id, enabled));
                    }

                    if let Some(err) = app.state.watchpoint_error(&wp.expr) {
                        ui.label(m("⚠", 12.0, RED)).on_hover_text(err.to_string());
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
                        pending_removes.push(wp.id);
                    }
                    ui.end_row();
                }
            });

        for (id, enable) in pending_toggles {
            app.send(Command::ToggleWatchpoint { id, enable });
        }
        for id in pending_removes {
            app.send(Command::RemoveWatchpoint(id));
        }

        ui.add_space(4.0);

        // Input row: kind selector + expression + Add button.
        ui.horizontal(|ui| {
            ComboBox::from_id_salt("wp_kind_select")
                .selected_text(kind_label(app.wp_kind_buffer))
                .show_ui(ui, |ui| {
                    for kind in [
                        WatchpointKind::Write,
                        WatchpointKind::Read,
                        WatchpointKind::Access,
                    ] {
                        ui.selectable_value(&mut app.wp_kind_buffer, kind, kind_label(kind));
                    }
                });

            ui.add(
                TextEdit::singleline(&mut app.wp_expr_buffer)
                    .font(FontId::monospace(11.0))
                    .desired_width(120.0)
                    .hint_text("expression"),
            );

            let can_add =
                should_add_watchpoint(&app.wp_expr_buffer, &app.state.persistent.watchpoints);
            if ui
                .add_enabled(can_add, egui::Button::new(m("Add", 12.0, ACCENT)))
                .clicked()
            {
                app.send(Command::AddWatchpoint {
                    expr: app.wp_expr_buffer.trim().to_string(),
                    kind: app.wp_kind_buffer,
                });
                app.wp_expr_buffer.clear();
            }
        });

        // CRITICAL-2: duplicate-rejection message text (spec:
        // "Watchpoint already exists for expression '<expr>'") — the Add
        // button being disabled is not itself visible feedback; the message
        // makes the rejection reason readable inline, as the user types.
        if let Some(msg) =
            duplicate_expr_message(&app.wp_expr_buffer, &app.state.persistent.watchpoints)
        {
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                ui.label(m(&msg, 11.0, RED));
            });
        }

        // CRITICAL-1: creation-failure errors that never got a row (a
        // rejected `-break-watch` returns no GDB id — design decision D1).
        // Listed here, separately from the table, so they stay visible and
        // are never a transient toast (spec: "Persistent Per-Row Error
        // Surfacing").
        let pending_errors = pending_watchpoint_errors(&app.state.errors.watchpoint_errors);
        if !pending_errors.is_empty() {
            ui.add_space(4.0);
            ui.label(m("Pending Errors", 10.0, TXT_DIM).italics());
            for (expr, message) in &pending_errors {
                ui.horizontal(|ui| {
                    ui.add_space(8.0);
                    ui.label(m(expr, 11.0, TXT_CYAN));
                    ui.label(m(": ", 11.0, TXT_DIM));
                    ui.label(m(message, 11.0, RED));
                });
            }
        }

        ui.add_space(4.0);
    }
    hl(ui);
}

fn kind_label(kind: WatchpointKind) -> &'static str {
    match kind {
        WatchpointKind::Write => "Write",
        WatchpointKind::Read => "Read",
        WatchpointKind::Access => "Access",
    }
}

// ─── Add decision ───────────────────────────────────────────────────────────

/// Pure decision for the input row's Add button: an `AddWatchpoint` may be
/// sent only when the trimmed expression is non-empty AND does not already
/// match an existing watchpoint's expression (spec: "Duplicate Expression
/// Prevention" — rejected client-side, with no MI round trip).
pub(crate) fn should_add_watchpoint(expr: &str, existing: &[Watchpoint]) -> bool {
    let trimmed = expr.trim();
    if trimmed.is_empty() {
        return false;
    }
    !existing.iter().any(|w| w.expr == trimmed)
}

/// CRITICAL-2 fix: the visible message for a duplicate expression, matching
/// the spec's exact scenario wording ("Duplicate rejected client-side"):
/// `Watchpoint already exists for expression '<expr>'`. `None` for an empty
/// (or whitespace-only) or a genuinely non-duplicate expression — mirrors
/// `should_add_watchpoint`'s emptiness/duplicate checks so the two never
/// disagree on what counts as a duplicate.
pub(crate) fn duplicate_expr_message(expr: &str, existing: &[Watchpoint]) -> Option<String> {
    let trimmed = expr.trim();
    if trimmed.is_empty() {
        return None;
    }
    if existing.iter().any(|w| w.expr == trimmed) {
        Some(format!(
            "Watchpoint already exists for expression '{trimmed}'"
        ))
    } else {
        None
    }
}

/// CRITICAL-3 fix: the visible "Old -> New" text for a
/// `StopReason::WatchpointTriggered` stop, `None` for every other stop
/// reason (or no active pause). Kept as a pure function so the formatting
/// is unit-testable without an `egui::Context`.
pub(crate) fn trigger_banner_text(stop_reason: Option<&StopReason>) -> Option<String> {
    match stop_reason {
        Some(StopReason::WatchpointTriggered { id, expr, old, new }) => {
            Some(format!("{expr} (id {id}): Old: {old} -> New: {new}"))
        }
        _ => None,
    }
}

/// CRITICAL-1 fix: `watchpoint_errors` (keyed by expression, design decision
/// D1) has no row to attach to when creation itself failed — this formats
/// the map into a deterministically ordered list for the panel's separate
/// "Pending Errors" section, so those errors are structurally reachable
/// instead of only checked per-existing-row (which a failed creation never
/// reaches).
pub(crate) fn pending_watchpoint_errors(
    errors: &std::collections::HashMap<String, String>,
) -> Vec<(String, String)> {
    let mut list: Vec<(String, String)> =
        errors.iter().map(|(e, m)| (e.clone(), m.clone())).collect();
    list.sort_by(|a, b| a.0.cmp(&b.0));
    list
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wp(expr: &str) -> Watchpoint {
        Watchpoint {
            id: 1,
            expr: expr.into(),
            kind: WatchpointKind::Write,
            enabled: true,
        }
    }

    #[test]
    fn rejects_empty_expression() {
        assert!(!should_add_watchpoint("", &[]));
    }

    #[test]
    fn rejects_whitespace_only_expression() {
        assert!(!should_add_watchpoint("   ", &[]));
    }

    #[test]
    fn rejects_duplicate_expression() {
        let existing = vec![wp("x")];
        assert!(!should_add_watchpoint("x", &existing));
    }

    #[test]
    fn accepts_non_empty_non_duplicate_expression() {
        let existing = vec![wp("x")];
        assert!(should_add_watchpoint("y", &existing));
    }

    #[test]
    fn accepts_trimmed_non_duplicate_expression_against_empty_existing() {
        assert!(should_add_watchpoint("count", &[]));
    }

    // ── CRITICAL-2: duplicate-rejection message text ────────────────────────

    #[test]
    fn duplicate_expr_message_none_for_empty_input() {
        assert_eq!(duplicate_expr_message("", &[]), None);
        assert_eq!(duplicate_expr_message("   ", &[]), None);
    }

    #[test]
    fn duplicate_expr_message_none_for_non_duplicate() {
        let existing = vec![wp("x")];
        assert_eq!(duplicate_expr_message("y", &existing), None);
    }

    #[test]
    fn duplicate_expr_message_matches_spec_wording_for_duplicate() {
        let existing = vec![wp("x")];
        assert_eq!(
            duplicate_expr_message("x", &existing),
            Some("Watchpoint already exists for expression 'x'".to_string())
        );
    }

    // ── CRITICAL-3: trigger old/new banner text ──────────────────────────────

    #[test]
    fn trigger_banner_text_none_when_no_trigger_stop_reason() {
        assert_eq!(trigger_banner_text(None), None);
        assert_eq!(trigger_banner_text(Some(&StopReason::Unknown)), None);
        assert_eq!(
            trigger_banner_text(Some(&StopReason::BreakpointHit(1))),
            None
        );
    }

    #[test]
    fn trigger_banner_text_formats_old_and_new_on_trigger() {
        let reason = StopReason::WatchpointTriggered {
            id: 2,
            expr: "x".into(),
            old: "0".into(),
            new: "3".into(),
        };
        assert_eq!(
            trigger_banner_text(Some(&reason)),
            Some("x (id 2): Old: 0 -> New: 3".to_string())
        );
    }

    // ── CRITICAL-1: pending (row-less) creation-error listing ───────────────

    #[test]
    fn pending_watchpoint_errors_empty_map_yields_empty_list() {
        let errors: std::collections::HashMap<String, String> = std::collections::HashMap::new();
        assert!(pending_watchpoint_errors(&errors).is_empty());
    }

    #[test]
    fn pending_watchpoint_errors_lists_entries_sorted_by_expr() {
        let mut errors = std::collections::HashMap::new();
        errors.insert("z".to_string(), "No symbol z".to_string());
        errors.insert("a".to_string(), "No symbol a".to_string());

        let list = pending_watchpoint_errors(&errors);
        assert_eq!(
            list,
            vec![
                ("a".to_string(), "No symbol a".to_string()),
                ("z".to_string(), "No symbol z".to_string()),
            ]
        );
    }
}
