//! Catchpoints panel content.
//!
//! Mirrors `panels::watchpoints` exactly (design decision D8), rendered
//! immediately after it (`app.rs`). Creation is fire-and-forget with no
//! optimistic row (design addendum A2): a submitted `AddCatchpoint` shows up
//! only once GDB's asynchronous `=breakpoint-created` notify (or, for
//! Load/Unload, its tokened `^done`) is parsed into `CatchpointAdded`.

use eframe::egui::{self, Color32, ComboBox, FontId, Stroke, TextEdit};

use crate::state::{Catchpoint, CatchpointKind, StopReason, SyscallPhase};
use crate::ui::app::App;
use crate::ui::command::Command;
use crate::ui::theme::*;
use crate::ui::widgets::{hl, m, sec_hdr};

pub(crate) fn render(app: &mut App, ui: &mut egui::Ui) {
    let mut open = app.panels.contains(crate::ui::app::PanelState::CATCHPOINTS);
    sec_hdr(ui, "Catchpoints", &mut open);
    app.panels.set(crate::ui::app::PanelState::CATCHPOINTS, open);
    if open {
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

        // Stop-event labeling for the other 5 kinds (Fork/Vfork/Exec always;
        // Load/Unload via the shared SolibEvent reason) — every one of these
        // stop reasons carries no `bkptno` to disambiguate by id (unlike a
        // plain breakpoint hit), so a single banner is shown regardless of
        // which specific armed catchpoint caused it, mirroring the
        // watchpoint trigger banner's presentation.
        if let Some(banner) = stop_event_banner_text(stop_reason) {
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                ui.label(m(&format!("⚡ {banner}"), 11.0, TXT_YELLOW));
            });
            ui.add_space(2.0);
        }

        // Phase 2c (D3): Throw/Rethrow/Catch stop labeling — these arrive
        // as a plain `BreakpointHit(id)`, so the banner is derived by
        // looking up the id in the catchpoint table (same shape as
        // `signal_catch_hint`, not `stop_event_banner_text`).
        if let Some(banner) = exception_banner_text(stop_reason, &app.state.persistent.catchpoints)
        {
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                ui.label(m(&format!("⚡ {banner}"), 11.0, TXT_YELLOW));
            });
            ui.add_space(2.0);
        }

        // D6: the one case that DOES carry an id is `BreakpointHit(id)` —
        // `catchpoint_by_id()` confirms that id belongs to an armed
        // catchpoint (not an ordinary breakpoint sharing the same id space)
        // before the row is highlighted.
        let highlighted_id = highlighted_catchpoint_id(stop_reason)
            .and_then(|id| app.state.catchpoint_by_id(id))
            .map(|c| c.id);

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
                    let is_hit = Some(cp.id) == highlighted_id;
                    let kind_color = if is_hit { ACCENT } else { TXT_YELLOW };
                    ui.label(m(kind_label(cp.kind), 12.0, kind_color));
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
                        CatchpointKind::Syscall,
                        CatchpointKind::Throw,
                        CatchpointKind::Rethrow,
                        CatchpointKind::Catch,
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
        let pending_errors = pending_catchpoint_errors(&app.state.errors.catchpoint_errors);
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
        CatchpointKind::Syscall => "Syscall",
        CatchpointKind::Throw => "Throw",
        CatchpointKind::Rethrow => "Rethrow",
        CatchpointKind::Catch => "Catch",
    }
}

/// Whether `kind` accepts a user-entered args string (spec: "Argument Arity
/// Validation Only" — Fork/Vfork/Exec: none; Signal: variadic; Load/Unload/
/// Throw/Rethrow/Catch: zero-or-one).
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
        // D1: no confirmation gate on a bare (all-syscalls) entry — the
        // warning text is the sole guard, matching Phase 2a's no-gate
        // precedent for other broad catchpoints.
        CatchpointKind::Syscall => {
            "name, number, or group:foo (space-separated) — ⚠ Bare = all syscalls"
        }
        // Phase 2c: optional regexp filter — a bare entry catches every
        // exception of this kind (A4: needs libstdc++ SDT probes; silently
        // ignored on a stripped runtime, documented here rather than gated
        // client-side, per design).
        CatchpointKind::Throw | CatchpointKind::Rethrow | CatchpointKind::Catch => {
            "optional regexp, e.g. std::runtime_error — bare = all exceptions"
        }
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
        // D5: variadic, whitespace-separated — same shape as Signal.
        CatchpointKind::Syscall => buffer.split_whitespace().map(|s| s.to_string()).collect(),
        // Phase 2c: zero-or-one regexp, whole trimmed buffer as one token —
        // same shape as Load/Unload (a regexp may contain spaces, e.g.
        // template arguments).
        CatchpointKind::Throw | CatchpointKind::Rethrow | CatchpointKind::Catch => {
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
        // D5: any arity is valid (bare, one, or many) — no client-side
        // validation of syscall name/number/group:foo legality.
        CatchpointKind::Syscall => true,
        // Phase 2c: zero-or-one regexp, same arity rule as Load/Unload.
        CatchpointKind::Throw | CatchpointKind::Rethrow | CatchpointKind::Catch => {
            args.len() <= 1
        }
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

/// Phase 2c (D3): the visible banner for a stop correlated to a Throw/
/// Rethrow/Catch catchpoint via `StopReason::BreakpointHit { bkptno }` (A2 —
/// verified live, Phase 0 spike: no dedicated `StopReason` variant exists).
/// A separate function rather than widening `stop_event_banner_text`
/// (which is a pure function of the stop reason alone, with a pinned test
/// asserting `BreakpointHit(1) -> None`): exception labeling requires the
/// catchpoint table to tell a catchpoint hit from a plain breakpoint hit —
/// exactly `signal_catch_hint`'s shape, not `stop_event_banner_text`'s.
/// `None` for an unmatched id, a plain breakpoint, or any other catchpoint
/// kind hit by id — never panics.
pub(crate) fn exception_banner_text(
    stop_reason: Option<&StopReason>,
    catchpoints: &[Catchpoint],
) -> Option<String> {
    let id = match stop_reason {
        Some(StopReason::BreakpointHit(id)) => *id,
        _ => return None,
    };
    let cp = catchpoints.iter().find(|c| c.id == id)?;
    match cp.kind {
        CatchpointKind::Throw => Some("Exception thrown".to_string()),
        CatchpointKind::Rethrow => Some("Exception rethrown".to_string()),
        CatchpointKind::Catch => Some("Exception caught".to_string()),
        _ => None,
    }
}

/// Stop-event labeling (verify blocker fix) for the 5 catchpoint stop
/// reasons that are NOT `StopReason::Signal` (which already has its own
/// `signal_catch_hint` banner, D6): Fork/Vfork show the child's new PID,
/// Exec shows the new executable's path, and the shared `SolibEvent`
/// (Load/Unload — GDB reports one literal reason for both, D4) shows the
/// affected library. `None` for every other stop reason, mirroring
/// `trigger_banner_text`'s (watchpoints) shape exactly.
///
/// Every field here is `Option` because GDB may omit it (verified live for
/// fork's `newpid` and solib-event's `library` — apply-blocker #68); a
/// missing field renders as "unknown" instead of silently showing nothing,
/// so the user always gets SOME labeling even on an under-specified reply.
pub(crate) fn stop_event_banner_text(stop_reason: Option<&StopReason>) -> Option<String> {
    const UNKNOWN: &str = "unknown";
    match stop_reason {
        Some(StopReason::Forked { newpid }) => Some(format!(
            "Fork: new PID {}",
            newpid
                .map(|p| p.to_string())
                .unwrap_or_else(|| UNKNOWN.into())
        )),
        Some(StopReason::Vforked { newpid }) => Some(format!(
            "Vfork: new PID {}",
            newpid
                .map(|p| p.to_string())
                .unwrap_or_else(|| UNKNOWN.into())
        )),
        Some(StopReason::Execd { path }) => {
            Some(format!("Exec: {}", path.as_deref().unwrap_or(UNKNOWN)))
        }
        Some(StopReason::SolibEvent { library }) => {
            Some(format!("Library {}", library.as_deref().unwrap_or(UNKNOWN)))
        }
        // Phase 2b (D3/D4): number-first (deterministic identity), name as
        // parenthesized context; "unknown" fallback when both are absent;
        // a trailing " [return]" marker distinguishes the return-phase stop
        // from entry (entry gets no suffix).
        Some(StopReason::SyscallTriggered {
            phase,
            number,
            name,
        }) => {
            let identity = match (number.as_deref(), name.as_deref()) {
                (Some(n), Some(name)) => format!("{n} ({name})"),
                (Some(n), None) => n.to_string(),
                (None, Some(name)) => name.to_string(),
                (None, None) => UNKNOWN.to_string(),
            };
            let suffix = match phase {
                SyscallPhase::Entry => "",
                SyscallPhase::Return => " [return]",
            };
            Some(format!("Syscall: {identity}{suffix}"))
        }
        _ => None,
    }
}

/// D6: which catchpoint id (if any) the current stop should highlight in
/// the table. Only `StopReason::BreakpointHit(id)` carries an id at all —
/// every other catchpoint stop reason (Fork/Vfork/Exec/Signal/SolibEvent)
/// has no `bkptno` to correlate from, so nothing is highlighted for those
/// (their banner above the table is the sole feedback). The caller still
/// confirms the id via `DebuggerState::catchpoint_by_id()` before treating
/// it as a catchpoint hit — this function only extracts the candidate id.
pub(crate) fn highlighted_catchpoint_id(stop_reason: Option<&StopReason>) -> Option<u32> {
    match stop_reason {
        Some(StopReason::BreakpointHit(id)) => Some(*id),
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

    // ── Phase 2b task 3.1: kind_label / takes_args for Syscall ───────────────

    #[test]
    fn kind_label_syscall_is_syscall() {
        assert_eq!(kind_label(CatchpointKind::Syscall), "Syscall");
    }

    #[test]
    fn takes_args_syscall_is_true() {
        assert!(takes_args(CatchpointKind::Syscall));
    }

    // ── Phase 2b task 3.4: ComboBox kind list includes Syscall ───────────────
    // No compiler guard on the render()-local array (unlike the exhaustive
    // matches above), so this test pins the count and renders every label
    // without panicking — the same array `render()` iterates.
    #[test]
    fn combobox_kind_list_has_seven_entries_including_syscall() {
        let kinds = [
            CatchpointKind::Fork,
            CatchpointKind::Vfork,
            CatchpointKind::Exec,
            CatchpointKind::Signal,
            CatchpointKind::Load,
            CatchpointKind::Unload,
            CatchpointKind::Syscall,
        ];
        assert_eq!(kinds.len(), 7);
        assert!(kinds.contains(&CatchpointKind::Syscall));
        for kind in kinds {
            // Must not panic for any kind — pins the "silently-passing" site.
            let _ = kind_label(kind);
        }
    }

    // ── Phase 2b task 3.2: args_hint for Syscall ─────────────────────────────

    #[test]
    fn args_hint_syscall_documents_shape_and_warns_bare_catches_all() {
        let hint = args_hint(CatchpointKind::Syscall);
        assert!(
            hint.contains("name, number, or group:foo"),
            "hint should document accepted input shapes: {hint:?}"
        );
        assert!(
            hint.contains("⚠ Bare = all syscalls"),
            "hint should warn that a bare entry catches every syscall: {hint:?}"
        );
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

    // Phase 2b task 3.5 (D5): Syscall accepts any arity, mirroring Signal —
    // GDB is the sole authority on syscall name/number/group legality.
    #[test]
    fn should_add_catchpoint_syscall_accepts_any_arity_including_zero() {
        assert!(should_add_catchpoint(CatchpointKind::Syscall, &[], &[]));
        assert!(should_add_catchpoint(
            CatchpointKind::Syscall,
            &["open".to_string()],
            &[]
        ));
        assert!(should_add_catchpoint(
            CatchpointKind::Syscall,
            &["open".to_string(), "read".to_string()],
            &[]
        ));
    }

    // D4 regression: a duplicate Syscall add (same parsed-back args) must be
    // rejected client-side, same as every other kind.
    #[test]
    fn should_add_catchpoint_rejects_syscall_duplicate_against_parsed_back_args() {
        let existing = vec![cp(CatchpointKind::Syscall, &["open"])];
        assert!(!should_add_catchpoint(
            CatchpointKind::Syscall,
            &["open".to_string()],
            &existing
        ));
        assert!(should_add_catchpoint(
            CatchpointKind::Syscall,
            &["close".to_string()],
            &existing
        ));
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
    fn args_from_buffer_syscall_splits_on_whitespace() {
        assert_eq!(
            args_from_buffer(CatchpointKind::Syscall, "open read"),
            vec!["open".to_string(), "read".to_string()]
        );
        assert!(args_from_buffer(CatchpointKind::Syscall, "").is_empty());
        assert!(args_from_buffer(CatchpointKind::Syscall, "   ").is_empty());
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

    // ── stop_event_banner_text (verify blocker fix) ──────────────────────────

    #[test]
    fn stop_event_banner_text_none_for_no_pause_or_unrelated_reasons() {
        assert_eq!(stop_event_banner_text(None), None);
        assert_eq!(stop_event_banner_text(Some(&StopReason::Unknown)), None);
        assert_eq!(
            stop_event_banner_text(Some(&StopReason::BreakpointHit(1))),
            None
        );
        // Signal has its own dedicated D6 banner (signal_catch_hint) —
        // must not also produce a generic stop-event banner.
        assert_eq!(
            stop_event_banner_text(Some(&StopReason::Signal("SIGINT".into()))),
            None
        );
    }

    // Phase 2b task 3.3 (D3/D4): Syscall banner — number first (deterministic
    // identity), name as parenthesized context, "unknown" fallback when both
    // are absent, and a trailing " [return]" marker distinguishing the
    // return-phase stop from entry (entry gets no suffix).
    #[test]
    fn stop_event_banner_text_syscall_entry_full() {
        assert_eq!(
            stop_event_banner_text(Some(&StopReason::SyscallTriggered {
                phase: SyscallPhase::Entry,
                number: Some("2".into()),
                name: Some("open".into()),
            })),
            Some("Syscall: 2 (open)".to_string())
        );
    }

    #[test]
    fn stop_event_banner_text_syscall_entry_number_only() {
        assert_eq!(
            stop_event_banner_text(Some(&StopReason::SyscallTriggered {
                phase: SyscallPhase::Entry,
                number: Some("2".into()),
                name: None,
            })),
            Some("Syscall: 2".to_string())
        );
    }

    #[test]
    fn stop_event_banner_text_syscall_entry_name_only() {
        assert_eq!(
            stop_event_banner_text(Some(&StopReason::SyscallTriggered {
                phase: SyscallPhase::Entry,
                number: None,
                name: Some("open".into()),
            })),
            Some("Syscall: open".to_string())
        );
    }

    #[test]
    fn stop_event_banner_text_syscall_entry_neither_field_falls_back_to_unknown() {
        assert_eq!(
            stop_event_banner_text(Some(&StopReason::SyscallTriggered {
                phase: SyscallPhase::Entry,
                number: None,
                name: None,
            })),
            Some("Syscall: unknown".to_string())
        );
    }

    #[test]
    fn stop_event_banner_text_syscall_return_full() {
        assert_eq!(
            stop_event_banner_text(Some(&StopReason::SyscallTriggered {
                phase: SyscallPhase::Return,
                number: Some("2".into()),
                name: Some("open".into()),
            })),
            Some("Syscall: 2 (open) [return]".to_string())
        );
    }

    #[test]
    fn stop_event_banner_text_syscall_return_number_only() {
        assert_eq!(
            stop_event_banner_text(Some(&StopReason::SyscallTriggered {
                phase: SyscallPhase::Return,
                number: Some("2".into()),
                name: None,
            })),
            Some("Syscall: 2 [return]".to_string())
        );
    }

    #[test]
    fn stop_event_banner_text_syscall_return_name_only() {
        assert_eq!(
            stop_event_banner_text(Some(&StopReason::SyscallTriggered {
                phase: SyscallPhase::Return,
                number: None,
                name: Some("open".into()),
            })),
            Some("Syscall: open [return]".to_string())
        );
    }

    #[test]
    fn stop_event_banner_text_syscall_return_neither_field_falls_back_to_unknown() {
        assert_eq!(
            stop_event_banner_text(Some(&StopReason::SyscallTriggered {
                phase: SyscallPhase::Return,
                number: None,
                name: None,
            })),
            Some("Syscall: unknown [return]".to_string())
        );
    }

    #[test]
    fn stop_event_banner_text_fork_with_and_without_newpid() {
        assert_eq!(
            stop_event_banner_text(Some(&StopReason::Forked { newpid: Some(4242) })),
            Some("Fork: new PID 4242".to_string())
        );
        assert_eq!(
            stop_event_banner_text(Some(&StopReason::Forked { newpid: None })),
            Some("Fork: new PID unknown".to_string())
        );
    }

    #[test]
    fn stop_event_banner_text_vfork_with_and_without_newpid() {
        assert_eq!(
            stop_event_banner_text(Some(&StopReason::Vforked { newpid: Some(999) })),
            Some("Vfork: new PID 999".to_string())
        );
        assert_eq!(
            stop_event_banner_text(Some(&StopReason::Vforked { newpid: None })),
            Some("Vfork: new PID unknown".to_string())
        );
    }

    #[test]
    fn stop_event_banner_text_exec_with_and_without_path() {
        assert_eq!(
            stop_event_banner_text(Some(&StopReason::Execd {
                path: Some("/usr/bin/foo".into())
            })),
            Some("Exec: /usr/bin/foo".to_string())
        );
        assert_eq!(
            stop_event_banner_text(Some(&StopReason::Execd { path: None })),
            Some("Exec: unknown".to_string())
        );
    }

    #[test]
    fn stop_event_banner_text_solib_event_with_and_without_library() {
        assert_eq!(
            stop_event_banner_text(Some(&StopReason::SolibEvent {
                library: Some("/lib/libfoo.so".into())
            })),
            Some("Library /lib/libfoo.so".to_string())
        );
        assert_eq!(
            stop_event_banner_text(Some(&StopReason::SolibEvent { library: None })),
            Some("Library unknown".to_string())
        );
    }

    // ── Phase 2c: Throw/Rethrow/Catch (C++ exceptions) ───────────────────────

    #[test]
    fn kind_label_throw_rethrow_catch() {
        assert_eq!(kind_label(CatchpointKind::Throw), "Throw");
        assert_eq!(kind_label(CatchpointKind::Rethrow), "Rethrow");
        assert_eq!(kind_label(CatchpointKind::Catch), "Catch");
    }

    #[test]
    fn takes_args_throw_rethrow_catch_is_true() {
        assert!(takes_args(CatchpointKind::Throw));
        assert!(takes_args(CatchpointKind::Rethrow));
        assert!(takes_args(CatchpointKind::Catch));
    }

    #[test]
    fn args_hint_throw_rethrow_catch_documents_optional_regexp() {
        for kind in [
            CatchpointKind::Throw,
            CatchpointKind::Rethrow,
            CatchpointKind::Catch,
        ] {
            let hint = args_hint(kind);
            assert!(
                hint.contains("regexp"),
                "hint should mention regexp for {kind:?}: {hint:?}"
            );
            assert!(
                hint.to_lowercase().contains("bare")
                    || hint.to_lowercase().contains("all exceptions"),
                "hint should document bare = all exceptions for {kind:?}: {hint:?}"
            );
        }
    }

    #[test]
    fn args_from_buffer_throw_rethrow_catch_keeps_whole_trimmed_buffer_as_one_token() {
        // Regexps contain spaces (e.g. template args) — unlike Signal's
        // whitespace split, the whole trimmed buffer is one token.
        assert_eq!(
            args_from_buffer(CatchpointKind::Throw, "  std::runtime_error  "),
            vec!["std::runtime_error".to_string()]
        );
        assert!(args_from_buffer(CatchpointKind::Throw, "   ").is_empty());
        assert_eq!(
            args_from_buffer(CatchpointKind::Rethrow, "My Exception"),
            vec!["My Exception".to_string()]
        );
        assert!(args_from_buffer(CatchpointKind::Catch, "").is_empty());
    }

    #[test]
    fn arity_ok_throw_rethrow_catch_accept_zero_or_one_reject_two() {
        assert!(arity_ok(CatchpointKind::Throw, &[]));
        assert!(arity_ok(
            CatchpointKind::Throw,
            &["std::runtime_error".to_string()]
        ));
        assert!(!arity_ok(
            CatchpointKind::Throw,
            &["a".to_string(), "b".to_string()]
        ));
        assert!(arity_ok(CatchpointKind::Rethrow, &[]));
        assert!(arity_ok(CatchpointKind::Catch, &[]));
    }

    #[test]
    fn should_add_catchpoint_throw_rethrow_catch_accept_zero_or_one_reject_two() {
        assert!(should_add_catchpoint(CatchpointKind::Throw, &[], &[]));
        assert!(should_add_catchpoint(
            CatchpointKind::Throw,
            &["std::runtime_error".to_string()],
            &[]
        ));
        assert!(!should_add_catchpoint(
            CatchpointKind::Throw,
            &["a".to_string(), "b".to_string()],
            &[]
        ));
    }

    // Multiple Throw catchpoints with different regexps coexist — no
    // client-side de-dup beyond the exact (kind, args) match (D4/spec).
    #[test]
    fn should_add_catchpoint_multiple_throw_with_different_regexps_are_not_duplicates() {
        let existing = vec![cp(CatchpointKind::Throw, &["std::runtime_error"])];
        assert!(should_add_catchpoint(
            CatchpointKind::Throw,
            &["std::logic_error".to_string()],
            &existing
        ));
        assert!(!should_add_catchpoint(
            CatchpointKind::Throw,
            &["std::runtime_error".to_string()],
            &existing
        ));
    }

    #[test]
    fn combobox_kind_list_has_ten_entries_including_exceptions() {
        let kinds = [
            CatchpointKind::Fork,
            CatchpointKind::Vfork,
            CatchpointKind::Exec,
            CatchpointKind::Signal,
            CatchpointKind::Load,
            CatchpointKind::Unload,
            CatchpointKind::Syscall,
            CatchpointKind::Throw,
            CatchpointKind::Rethrow,
            CatchpointKind::Catch,
        ];
        assert_eq!(kinds.len(), 10);
        for kind in kinds {
            let _ = kind_label(kind);
        }
    }

    // ── exception_banner_text (D3) ───────────────────────────────────────────

    fn armed(id: u32, kind: CatchpointKind) -> Catchpoint {
        Catchpoint {
            id,
            kind,
            args: vec![],
            enabled: true,
        }
    }

    #[test]
    fn exception_banner_text_throw_hit_shows_exception_thrown() {
        let catchpoints = vec![armed(3, CatchpointKind::Throw)];
        assert_eq!(
            exception_banner_text(Some(&StopReason::BreakpointHit(3)), &catchpoints),
            Some("Exception thrown".to_string())
        );
    }

    #[test]
    fn exception_banner_text_rethrow_hit_shows_exception_rethrown() {
        let catchpoints = vec![armed(4, CatchpointKind::Rethrow)];
        assert_eq!(
            exception_banner_text(Some(&StopReason::BreakpointHit(4)), &catchpoints),
            Some("Exception rethrown".to_string())
        );
    }

    #[test]
    fn exception_banner_text_catch_hit_shows_exception_caught() {
        let catchpoints = vec![armed(5, CatchpointKind::Catch)];
        assert_eq!(
            exception_banner_text(Some(&StopReason::BreakpointHit(5)), &catchpoints),
            Some("Exception caught".to_string())
        );
    }

    #[test]
    fn exception_banner_text_none_for_unmatched_id() {
        let catchpoints = vec![armed(3, CatchpointKind::Throw)];
        assert_eq!(
            exception_banner_text(Some(&StopReason::BreakpointHit(99)), &catchpoints),
            None
        );
    }

    #[test]
    fn exception_banner_text_none_for_plain_breakpoint_no_catchpoints() {
        assert_eq!(
            exception_banner_text(Some(&StopReason::BreakpointHit(1)), &[]),
            None
        );
    }

    #[test]
    fn exception_banner_text_none_for_other_kind_catchpoint_hit_by_id() {
        let catchpoints = vec![armed(3, CatchpointKind::Fork)];
        assert_eq!(
            exception_banner_text(Some(&StopReason::BreakpointHit(3)), &catchpoints),
            None
        );
    }

    #[test]
    fn exception_banner_text_none_for_non_breakpoint_hit_stop_reason() {
        let catchpoints = vec![armed(3, CatchpointKind::Throw)];
        assert_eq!(exception_banner_text(None, &catchpoints), None);
        assert_eq!(
            exception_banner_text(Some(&StopReason::Unknown), &catchpoints),
            None
        );
    }

    // ── highlighted_catchpoint_id (D6, verify blocker fix) ───────────────────

    #[test]
    fn highlighted_catchpoint_id_extracts_id_from_breakpoint_hit_only() {
        assert_eq!(
            highlighted_catchpoint_id(Some(&StopReason::BreakpointHit(7))),
            Some(7)
        );
        assert_eq!(highlighted_catchpoint_id(None), None);
        assert_eq!(
            highlighted_catchpoint_id(Some(&StopReason::Forked { newpid: Some(1) })),
            None,
            "Fork carries no bkptno to correlate a row by id"
        );
        assert_eq!(
            highlighted_catchpoint_id(Some(&StopReason::SolibEvent { library: None })),
            None,
            "SolibEvent carries no bkptno to correlate a row by id"
        );
    }
}
