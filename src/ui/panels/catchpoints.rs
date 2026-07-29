//! Catchpoints panel content.
//!
//! Mirrors `panels::watchpoints` exactly (design decision D8), rendered
//! immediately after it (`app.rs`). Creation is fire-and-forget with no
//! optimistic row (design addendum A2): a submitted `AddCatchpoint` shows up
//! only once GDB's asynchronous `=breakpoint-created` notify (or, for
//! Load/Unload, its tokened `^done`) is parsed into `CatchpointAdded`.

use eframe::egui::{self, Color32, ComboBox, FontId, Stroke, TextEdit};

use crate::state::{Catchpoint, CatchpointKind, StopReason};
use crate::ui::app::App;
use crate::ui::command::Command;
use crate::ui::theme::*;
use crate::ui::widgets::{hl, m, sec_hdr};

pub(crate) fn render(app: &mut App, ui: &mut egui::Ui) {
    sec_hdr(ui, "Catchpoints", &mut app.open_cp);
    if app.open_cp {
        // D6: a signal-received stop is correlated to an armed Signal
        // catchpoint by name (or "all") only — no id-based disambiguation
        // (that is the breakpoint-hit/C++-exception path, deferred to Phase
        // 2b). Rendered as a banner, like the watchpoint trigger banner.
        let stop_reason = app.state.pause.as_ref().map(|p| &p.stop_reason);
        if let Some(name) = signal_stop_name(stop_reason) {
            if let Some(hint) = signal_catch_hint(name, &app.state.persistent.catchpoints) {
                ui.horizontal(|ui| {
                    ui.add_space(8.0);
                    ui.label(m(&format!("⚡ {hint}"), 11.0, TXT_YELLOW));
                });
                ui.add_space(2.0);
            }
        }

        let mut pending_toggles: Vec<(u32, bool)> = Vec::new();
        let mut pending_removes: Vec<u32> = Vec::new();

        egui::Grid::new("cp_grid")
            .num_columns(5)
            .spacing([8.0, 2.0])
            .show(ui, |ui| {
                for h in ["Kind", "Args", "On", "", ""] {
                    ui.label(m(h, 11.0, TXT_DIM));
                }
                ui.end_row();

                for cp in &app.state.persistent.catchpoints {
                    ui.label(m(kind_label(cp.kind), 12.0, TXT_YELLOW));
                    ui.label(m(&cp.args.join(" "), 12.0, TXT_CYAN));

                    let mut enabled = cp.enabled;
                    if ui.checkbox(&mut enabled, "").changed() {
                        pending_toggles.push((cp.id, enabled));
                    }

                    // Per-row error display: a rejected toggle/delete can
                    // still target an id that DOES have a row — see the
                    // Pending Errors section below for creation failures,
                    // which have no id and therefore no row to attach to
                    // (design decision D1).
                    let key = catchpoint_error_key(cp.kind, &cp.args);
                    if let Some(err) = app.state.catchpoint_error(&key) {
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
                        pending_removes.push(cp.id);
                    }
                    ui.end_row();
                }
            });

        for (id, enable) in pending_toggles {
            app.send(Command::ToggleCatchpoint { id, enable });
        }
        for id in pending_removes {
            app.send(Command::RemoveCatchpoint(id));
        }

        ui.add_space(4.0);

        // Input row: kind dropdown + args field (disabled for Fork/Vfork/
        // Exec, which take none) + Add button.
        ui.horizontal(|ui| {
            ComboBox::from_id_salt("cp_kind_select")
                .selected_text(kind_label(app.cp_kind_buffer))
                .show_ui(ui, |ui| {
                    for kind in [
                        CatchpointKind::Fork,
                        CatchpointKind::Vfork,
                        CatchpointKind::Exec,
                        CatchpointKind::Signal,
                        CatchpointKind::Load,
                        CatchpointKind::Unload,
                    ] {
                        ui.selectable_value(&mut app.cp_kind_buffer, kind, kind_label(kind));
                    }
                });

            let args_enabled = takes_args(app.cp_kind_buffer);
            ui.add_enabled(
                args_enabled,
                TextEdit::singleline(&mut app.cp_args_buffer)
                    .font(FontId::monospace(11.0))
                    .desired_width(120.0)
                    .hint_text(args_hint(app.cp_kind_buffer)),
            );

            let args = args_from_buffer(app.cp_kind_buffer, &app.cp_args_buffer);
            let can_add =
                should_add_catchpoint(app.cp_kind_buffer, &args, &app.state.persistent.catchpoints);
            if ui
                .add_enabled(can_add, egui::Button::new(m("Add", 12.0, ACCENT)))
                .clicked()
            {
                app.send(Command::AddCatchpoint {
                    kind: app.cp_kind_buffer,
                    args,
                });
                app.cp_args_buffer.clear();
            }
        });

        // CRITICAL-1-equivalent (mirrors watchpoints): a rejected creation
        // never got a GDB id, so there is no row to attach its error to
        // (design decision D1). Listed separately so it stays visible and
        // is never a transient toast (spec: "Persistent Per-Row Error
        // Surfacing").
        let pending_errors = pending_catchpoint_errors(&app.state.catchpoint_errors);
        if !pending_errors.is_empty() {
            ui.add_space(4.0);
            ui.label(m("Pending Errors", 10.0, TXT_DIM).italics());
            for (key, message) in &pending_errors {
                ui.horizontal(|ui| {
                    ui.add_space(8.0);
                    ui.label(m(key, 11.0, TXT_CYAN));
                    ui.label(m(": ", 11.0, TXT_DIM));
                    ui.label(m(message, 11.0, RED));
                });
            }
        }

        ui.add_space(4.0);
    }
    hl(ui);
}

fn kind_label(kind: CatchpointKind) -> &'static str {
    match kind {
        CatchpointKind::Fork => "Fork",
        CatchpointKind::Vfork => "Vfork",
        CatchpointKind::Exec => "Exec",
        CatchpointKind::Signal => "Signal",
        CatchpointKind::Load => "Load",
        CatchpointKind::Unload => "Unload",
    }
}

/// Whether `kind` accepts a user-entered args string (spec: "Argument Arity
/// Validation Only" — Fork/Vfork/Exec: none; Signal: variadic; Load/Unload:
/// zero-or-one).
fn takes_args(kind: CatchpointKind) -> bool {
    !matches!(
        kind,
        CatchpointKind::Fork | CatchpointKind::Vfork | CatchpointKind::Exec
    )
}

fn args_hint(kind: CatchpointKind) -> &'static str {
    match kind {
        CatchpointKind::Signal => "signal name(s), space-separated",
        CatchpointKind::Load | CatchpointKind::Unload => "library regex (optional)",
        _ => "",
    }
}

/// Splits the single-line `cp_args_buffer` into the `Vec<String>` shape
/// `Command::AddCatchpoint` expects. Signal is variadic (space-separated);
/// Load/Unload take at most one token (the whole trimmed buffer, so a
/// regex containing spaces survives intact); Fork/Vfork/Exec always yield
/// an empty vec regardless of buffer content (their UI field is disabled,
/// but this stays total even if called directly, e.g. from a test).
pub(crate) fn args_from_buffer(kind: CatchpointKind, buffer: &str) -> Vec<String> {
    match kind {
        CatchpointKind::Fork | CatchpointKind::Vfork | CatchpointKind::Exec => vec![],
        CatchpointKind::Signal => buffer.split_whitespace().map(|s| s.to_string()).collect(),
        CatchpointKind::Load | CatchpointKind::Unload => {
            let trimmed = buffer.trim();
            if trimmed.is_empty() {
                vec![]
            } else {
                vec![trimmed.to_string()]
            }
        }
    }
}

/// D1 key shape (`"{kind}:{args joined by ','}"`), matching
/// `catchpoint_errors`'/`pending_catch`'s key exactly so a per-row error
/// lookup for an EXISTING row (a toggle/delete failure, which does have an
/// id) still finds it.
fn catchpoint_error_key(kind: CatchpointKind, args: &[String]) -> String {
    format!("{kind}:{}", args.join(","))
}

// ─── Add decision ───────────────────────────────────────────────────────────

/// Pure decision for the input row's Add button (spec: "Argument Arity
/// Validation Only" + "Catchpoint Creation" duplicate rejection, mirroring
/// `should_add_watchpoint`). Validates only arity/emptiness per kind — never
/// the content of `args` (GDB is the sole authority on legality) — and
/// rejects an exact (kind, args) duplicate client-side, with no MI round
/// trip.
pub(crate) fn should_add_catchpoint(
    kind: CatchpointKind,
    args: &[String],
    existing: &[Catchpoint],
) -> bool {
    if !arity_ok(kind, args) {
        return false;
    }
    !existing.iter().any(|c| c.kind == kind && c.args == args)
}

fn arity_ok(kind: CatchpointKind, args: &[String]) -> bool {
    match kind {
        CatchpointKind::Fork | CatchpointKind::Vfork | CatchpointKind::Exec => args.is_empty(),
        CatchpointKind::Signal => true,
        CatchpointKind::Load | CatchpointKind::Unload => args.len() <= 1,
    }
}

/// D6: the visible hint text for a `StopReason::Signal` stop that matches an
/// armed, enabled Signal catchpoint's name (or its literal `"all"` arg).
/// `None` for every other stop reason, or when no catchpoint matches — no
/// false precision, renders exactly as today when no catchpoint is armed.
pub(crate) fn signal_catch_hint(name: &str, catchpoints: &[Catchpoint]) -> Option<String> {
    catchpoints
        .iter()
        .find(|c| {
            c.kind == CatchpointKind::Signal
                && c.enabled
                && c.args.iter().any(|a| a == name || a == "all")
        })
        .map(|c| format!("Signal catchpoint (id {}) armed for {name}", c.id))
}

fn signal_stop_name(stop_reason: Option<&StopReason>) -> Option<&str> {
    match stop_reason {
        Some(StopReason::Signal(name)) => Some(name.as_str()),
        _ => None,
    }
}

/// Formats `catchpoint_errors` into a deterministically ordered list for the
/// panel's "Pending Errors" section — mirrors
/// `watchpoints::pending_watchpoint_errors` exactly.
pub(crate) fn pending_catchpoint_errors(
    errors: &std::collections::HashMap<String, String>,
) -> Vec<(String, String)> {
    let mut list: Vec<(String, String)> =
        errors.iter().map(|(k, m)| (k.clone(), m.clone())).collect();
    list.sort_by(|a, b| a.0.cmp(&b.0));
    list
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cp(kind: CatchpointKind, args: &[&str]) -> Catchpoint {
        Catchpoint {
            id: 1,
            kind,
            args: args.iter().map(|s| s.to_string()).collect(),
            enabled: true,
        }
    }

    // ── should_add_catchpoint: arity per kind ────────────────────────────────

    #[test]
    fn should_add_catchpoint_fork_vfork_exec_reject_nonempty_args() {
        assert!(should_add_catchpoint(CatchpointKind::Fork, &[], &[]));
        assert!(!should_add_catchpoint(
            CatchpointKind::Fork,
            &["stray".to_string()],
            &[]
        ));
        assert!(should_add_catchpoint(CatchpointKind::Vfork, &[], &[]));
        assert!(should_add_catchpoint(CatchpointKind::Exec, &[], &[]));
    }

    #[test]
    fn should_add_catchpoint_signal_accepts_any_arity_including_zero() {
        assert!(should_add_catchpoint(CatchpointKind::Signal, &[], &[]));
        assert!(should_add_catchpoint(
            CatchpointKind::Signal,
            &["SIGINT".to_string()],
            &[]
        ));
        assert!(should_add_catchpoint(
            CatchpointKind::Signal,
            &["SIGINT".to_string(), "SIGTERM".to_string()],
            &[]
        ));
    }

    #[test]
    fn should_add_catchpoint_load_unload_accept_zero_or_one_reject_two() {
        assert!(should_add_catchpoint(CatchpointKind::Load, &[], &[]));
        assert!(should_add_catchpoint(
            CatchpointKind::Load,
            &["libc".to_string()],
            &[]
        ));
        assert!(!should_add_catchpoint(
            CatchpointKind::Load,
            &["libc".to_string(), "libm".to_string()],
            &[]
        ));
        assert!(should_add_catchpoint(CatchpointKind::Unload, &[], &[]));
    }

    #[test]
    fn should_add_catchpoint_rejects_exact_kind_and_args_duplicate() {
        let existing = vec![cp(CatchpointKind::Signal, &["SIGINT"])];
        assert!(!should_add_catchpoint(
            CatchpointKind::Signal,
            &["SIGINT".to_string()],
            &existing
        ));
        // Different args → not a duplicate.
        assert!(should_add_catchpoint(
            CatchpointKind::Signal,
            &["SIGSEGV".to_string()],
            &existing
        ));
        // Same args, different kind → not a duplicate.
        assert!(should_add_catchpoint(
            CatchpointKind::Load,
            &["SIGINT".to_string()],
            &existing
        ));
    }

    #[test]
    fn should_add_catchpoint_multiple_forks_are_not_duplicates_of_each_other_by_kind_alone() {
        // Two Fork catchpoints both have empty args by construction — a
        // second Fork add IS a duplicate (same kind, same empty args).
        let existing = vec![cp(CatchpointKind::Fork, &[])];
        assert!(!should_add_catchpoint(CatchpointKind::Fork, &[], &existing));
    }

    // ── args_from_buffer ──────────────────────────────────────────────────

    #[test]
    fn args_from_buffer_fork_vfork_exec_always_empty() {
        assert!(args_from_buffer(CatchpointKind::Fork, "ignored text").is_empty());
        assert!(args_from_buffer(CatchpointKind::Vfork, "ignored").is_empty());
        assert!(args_from_buffer(CatchpointKind::Exec, "ignored").is_empty());
    }

    #[test]
    fn args_from_buffer_signal_splits_on_whitespace() {
        assert_eq!(
            args_from_buffer(CatchpointKind::Signal, "SIGINT SIGTERM"),
            vec!["SIGINT".to_string(), "SIGTERM".to_string()]
        );
        assert!(args_from_buffer(CatchpointKind::Signal, "").is_empty());
        assert!(args_from_buffer(CatchpointKind::Signal, "   ").is_empty());
    }

    #[test]
    fn args_from_buffer_load_unload_keeps_whole_trimmed_buffer_as_one_token() {
        assert_eq!(
            args_from_buffer(CatchpointKind::Load, "  libfoo\\.so  "),
            vec!["libfoo\\.so".to_string()]
        );
        assert!(args_from_buffer(CatchpointKind::Load, "   ").is_empty());
    }

    // ── D6: signal_catch_hint ────────────────────────────────────────────────

    #[test]
    fn signal_catch_hint_none_when_no_catchpoints() {
        assert_eq!(signal_catch_hint("SIGINT", &[]), None);
    }

    #[test]
    fn signal_catch_hint_matches_exact_signal_name() {
        let catchpoints = vec![cp(CatchpointKind::Signal, &["SIGINT"])];
        assert!(signal_catch_hint("SIGINT", &catchpoints).is_some());
        assert_eq!(signal_catch_hint("SIGSEGV", &catchpoints), None);
    }

    #[test]
    fn signal_catch_hint_matches_all_wildcard_arg() {
        let catchpoints = vec![cp(CatchpointKind::Signal, &["all"])];
        assert!(signal_catch_hint("SIGSEGV", &catchpoints).is_some());
        assert!(signal_catch_hint("SIGINT", &catchpoints).is_some());
    }

    #[test]
    fn signal_catch_hint_ignores_disabled_catchpoint() {
        let mut disabled = cp(CatchpointKind::Signal, &["SIGINT"]);
        disabled.enabled = false;
        assert_eq!(signal_catch_hint("SIGINT", &[disabled]), None);
    }

    #[test]
    fn signal_catch_hint_ignores_non_signal_kinds() {
        let catchpoints = vec![cp(CatchpointKind::Fork, &[])];
        assert_eq!(signal_catch_hint("SIGINT", &catchpoints), None);
    }

    // ── pending_catchpoint_errors ─────────────────────────────────────────

    #[test]
    fn pending_catchpoint_errors_empty_map_yields_empty_list() {
        let errors: std::collections::HashMap<String, String> = std::collections::HashMap::new();
        assert!(pending_catchpoint_errors(&errors).is_empty());
    }

    #[test]
    fn pending_catchpoint_errors_lists_entries_sorted_by_key() {
        let mut errors = std::collections::HashMap::new();
        errors.insert("signal:BOGUS".to_string(), "Undefined signal".to_string());
        errors.insert("fork:".to_string(), "some error".to_string());

        let list = pending_catchpoint_errors(&errors);
        assert_eq!(
            list,
            vec![
                ("fork:".to_string(), "some error".to_string()),
                ("signal:BOGUS".to_string(), "Undefined signal".to_string()),
            ]
        );
    }

    // ── catchpoint_error_key ──────────────────────────────────────────────

    #[test]
    fn catchpoint_error_key_matches_d1_shape() {
        assert_eq!(
            catchpoint_error_key(CatchpointKind::Signal, &["SIGINT".to_string()]),
            "signal:SIGINT"
        );
        assert_eq!(catchpoint_error_key(CatchpointKind::Fork, &[]), "fork:");
    }
}
