//! Watch/Registers/Data tabs content — extracted from app.rs in slice S3.

use eframe::egui::{self, Color32, FontId, ScrollArea, Stroke, TextEdit, Vec2};

use super::util::visible_register_rows;
use crate::state::EditTarget;
use crate::ui::app::App;
use crate::ui::command::Command;
use crate::ui::registers::RegClass;
use crate::ui::theme::*;
use crate::ui::widgets::{hl, m};

// ─── UI-only tab state ────────────────────────────────────────────────────────

#[derive(Default, PartialEq, Clone, Copy)]
pub(crate) enum WatchTab {
    #[default]
    Watch,
    Registers,
    Data,
}

pub(crate) fn render(app: &mut App, ui: &mut egui::Ui) {
    ui.add_space(2.0);
    ui.horizontal(|ui| {
        for (label, tab) in [
            ("Watch", WatchTab::Watch),
            ("Registers", WatchTab::Registers),
            ("Data", WatchTab::Data),
        ] {
            let active = app.watch_tab == tab;
            let col = if active {
                Color32::from_rgb(0xe0, 0xe0, 0xe0)
            } else {
                TXT_DIM
            };
            let fill = if active {
                BG_HOVER
            } else {
                Color32::TRANSPARENT
            };
            let resp = ui.add(
                egui::Button::new(m(label, 12.0, col))
                    .fill(fill)
                    .stroke(Stroke::NONE)
                    .min_size(Vec2::new(0.0, 24.0)),
            );
            if active {
                let r = resp.rect;
                ui.painter().line_segment(
                    [r.left_bottom(), r.right_bottom()],
                    Stroke::new(2.0_f32, ACCENT),
                );
            }
            if resp.clicked() {
                app.watch_tab = tab;
            }
        }
    });
    hl(ui);

    // Tab body ──────────────────────────────────────────────────────
    ScrollArea::vertical()
        .id_salt("watch_body")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.add_space(2.0);
            let paused = app.state.is_paused();
            match app.watch_tab {
                WatchTab::Watch => {
                    let mut pending_commits: Vec<Command> = Vec::new();

                    for var in &app.state.locals {
                        let target = EditTarget::Local(var.name.clone());
                        let error = app.state.edit_error(&target).map(|s| s.to_string());
                        ui.horizontal(|ui| {
                            ui.add_space(8.0);
                            ui.label(m(&var.name, 11.0, TXT_CYAN));
                            ui.label(m(" = ", 11.0, TXT_DIM));
                            if let Some(cmd) =
                                value_cell(ui, &mut app.value_edit_buffer, &target, &var.value, paused)
                            {
                                pending_commits.push(cmd);
                            }
                            if let Some(msg) = &error {
                                ui.label(m("⚠", 12.0, RED)).on_hover_text(msg.clone());
                            }
                        });
                    }
                    if app.state.locals.is_empty() {
                        ui.label(m("No locals", 11.0, TXT_DIM).italics());
                    }

                    if !app.state.globals.is_empty() {
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            ui.add_space(8.0);
                            ui.label(m("Globals", 10.0, TXT_DIM).italics());
                        });
                        for var in &app.state.globals {
                            let target = EditTarget::Global(var.name.clone());
                            let error = app.state.edit_error(&target).map(|s| s.to_string());
                            ui.horizontal(|ui| {
                                ui.add_space(8.0);
                                ui.label(m(&var.name, 11.0, TXT_CYAN));
                                ui.label(m(" = ", 11.0, TXT_DIM));
                                if let Some(cmd) = value_cell(
                                    ui,
                                    &mut app.value_edit_buffer,
                                    &target,
                                    &var.value,
                                    paused,
                                ) {
                                    pending_commits.push(cmd);
                                }
                                if let Some(msg) = &error {
                                    ui.label(m("⚠", 12.0, RED)).on_hover_text(msg.clone());
                                }
                            });
                        }
                    }

                    for cmd in pending_commits {
                        app.send(cmd);
                    }
                }
                WatchTab::Registers => {
                    if app.state.registers.is_empty() {
                        ui.label(m("Not paused — no register data", 11.0, TXT_DIM).italics());
                    } else {
                        let rows = visible_register_rows(&app.state.register_names, &app.state.registers);
                        let mut pending_commits: Vec<Command> = Vec::new();

                        for (class, label, color) in REG_GROUPS {
                            let mut group: Vec<_> =
                                rows.iter().filter(|(_, _, c)| *c == class).collect();
                            if group.is_empty() {
                                continue;
                            }
                            // The general group goes in conventional order;
                            // the rest preserve GDB's order.
                            if class == RegClass::General {
                                group.sort_by_key(|(name, _, _)| {
                                    crate::ui::registers::display_order(name)
                                });
                            }

                            ui.add_space(4.0);
                            ui.horizontal(|ui| {
                                ui.add_space(8.0);
                                ui.label(m(label, 10.0, TXT_MUTED).italics());
                            });

                            egui::Grid::new(label)
                                .num_columns(3)
                                .spacing([12.0, 1.0])
                                .striped(true)
                                .show(ui, |ui| {
                                    for (name, value, _) in &group {
                                        ui.horizontal(|ui| {
                                            ui.add_space(8.0);
                                            ui.label(m(name, 11.0, color));
                                        });
                                        let target = EditTarget::Register(name.clone());
                                        let error =
                                            app.state.edit_error(&target).map(|s| s.to_string());
                                        if let Some(cmd) = value_cell(
                                            ui,
                                            &mut app.value_edit_buffer,
                                            &target,
                                            value,
                                            paused,
                                        ) {
                                            pending_commits.push(cmd);
                                        }
                                        if let Some(msg) = &error {
                                            ui.label(m("⚠", 12.0, RED)).on_hover_text(msg.clone());
                                        } else {
                                            ui.label("");
                                        }
                                        ui.end_row();
                                    }
                                });
                        }

                        for cmd in pending_commits {
                            app.send(cmd);
                        }
                    }
                }
                WatchTab::Data => {
                    if app.state.disasm.is_empty() {
                        ui.label(m("Not paused", 11.0, TXT_DIM).italics());
                    } else {
                        let cur_addr = app.state.current_addr();
                        for asm in &app.state.disasm {
                            let is_current = Some(asm.addr) == cur_addr;
                            let col = if is_current { TXT_HL } else { TXT };
                            ui.horizontal(|ui| {
                                if is_current {
                                    ui.label(m("▶", 11.0, ACCENT));
                                } else {
                                    ui.add_space(14.0);
                                }
                                ui.label(m(&format!("0x{:x}", asm.addr), 11.0, TXT_DIM));
                                ui.add_space(6.0);
                                ui.label(m(&asm.inst, 11.0, col));
                            });
                        }
                    }
                }
            }
        });
}

// ─── Editable value cell ────────────────────────────────────────────────────────

/// Renders one editable value cell (Locals/Globals/Registers): a `TextEdit`
/// seeded from `authoritative` while paused, a plain label while running
/// (inert per the "Paused-Only Edit Precondition" requirement). Esc reverts
/// the buffer immediately without committing (spec: "User cancels an
/// edit"); a commit decision (4.1) queues — never sends directly, avoiding a
/// mutable/immutable borrow conflict on `App` while iterating `app.state` —
/// a `Command::SetValue` for the caller to flush after the render pass. A
/// reseed decision (4.2) then hard-reverts an unfocused, diverged buffer
/// back to `authoritative`, which covers both the post-refresh case and the
/// error hard-revert case (ValueEditFailed never updates the authoritative
/// value, so the very next unfocused frame snaps back to the last
/// known-good value).
fn value_cell(
    ui: &mut egui::Ui,
    buffer_map: &mut std::collections::HashMap<EditTarget, String>,
    target: &EditTarget,
    authoritative: &str,
    paused: bool,
) -> Option<Command> {
    let buffer = buffer_map
        .entry(target.clone())
        .or_insert_with(|| authoritative.to_string());

    if !paused {
        // Inert while running: keep the buffer in lockstep with the
        // authoritative value so it is ready to display verbatim the moment
        // the program pauses again.
        *buffer = authoritative.to_string();
        ui.label(m(buffer.as_str(), 11.0, TXT_YELLOW));
        return None;
    }

    let resp = ui.add(
        TextEdit::singleline(buffer)
            .font(FontId::monospace(11.0))
            .desired_width(100.0),
    );

    let escaped = resp.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape));
    if escaped {
        *buffer = authoritative.to_string();
    }

    let commit = if !escaped && should_commit_value_edit(paused, resp.lost_focus(), buffer, authoritative)
    {
        Some(Command::SetValue {
            target: target.clone(),
            value: buffer.clone(),
        })
    } else {
        None
    };

    if should_reseed_value_buffer(resp.has_focus(), buffer, authoritative) {
        *buffer = authoritative.to_string();
    }

    commit
}

// ─── Value-cell commit/reseed decisions ────────────────────────────────────────

/// Pure decision for a Watch/Registers value cell: a `Command::SetValue` must
/// be sent only when the program is paused, the field just lost focus
/// (covers both Enter and blur), AND the edited text actually differs from
/// the last known-good value. Mirrors
/// `breakpoints::should_commit_breakpoint_condition`, plus the paused-only
/// precondition from the spec.
pub(crate) fn should_commit_value_edit(
    paused: bool,
    lost_focus: bool,
    buffer: &str,
    current: &str,
) -> bool {
    if !paused || !lost_focus {
        return false;
    }
    buffer != current
}

/// Pure decision for whether a value-cell buffer should be overwritten with
/// the authoritative value from `DebuggerState` (re-fetch landed, or the cell
/// was never touched). Never reseeds a focused field — that would clobber
/// the user's in-progress keystrokes.
pub(crate) fn should_reseed_value_buffer(has_focus: bool, buffer: &str, authoritative: &str) -> bool {
    if has_focus {
        return false;
    }
    buffer != authoritative
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_commit_value_edit_false_when_not_paused() {
        // Even with lost_focus and a real change, a running program must
        // never accept a value-edit commit (spec: "Paused-Only Edit
        // Precondition").
        assert!(!should_commit_value_edit(false, true, "42", "7"));
    }

    #[test]
    fn should_commit_value_edit_false_mid_keystroke() {
        // Simulates typing: field retains focus so lost_focus() is false —
        // no command must be sent per keystroke, paused or not.
        assert!(!should_commit_value_edit(true, false, "4", "7"));
        assert!(!should_commit_value_edit(true, false, "42", "42"));
    }

    #[test]
    fn should_commit_value_edit_false_when_unchanged() {
        // Clicking into the cell and back out without editing (Esc, or
        // blur-without-change) must not send a no-op SetValue.
        assert!(!should_commit_value_edit(true, true, "42", "42"));
    }

    #[test]
    fn should_commit_value_edit_true_when_paused_lost_focus_and_changed() {
        assert!(should_commit_value_edit(true, true, "99", "42"));
        assert!(should_commit_value_edit(true, true, "0x2a", "0x1"));
    }

    #[test]
    fn should_reseed_value_buffer_false_when_focused() {
        // A field the user is actively editing must never be clobbered by an
        // incoming refresh, even if the buffer has diverged from state.
        assert!(!should_reseed_value_buffer(true, "in-progress", "42"));
    }

    #[test]
    fn should_reseed_value_buffer_false_when_already_matching() {
        assert!(!should_reseed_value_buffer(false, "42", "42"));
    }

    #[test]
    fn should_reseed_value_buffer_true_when_unfocused_and_diverged() {
        // Covers both the post-refresh case (new authoritative value landed)
        // and the hard-revert-on-error case (buffer still shows rejected
        // text, authoritative is the last known-good value).
        assert!(should_reseed_value_buffer(false, "stale", "42"));
        assert!(should_reseed_value_buffer(false, "rejected-text", "7"));
    }
}
