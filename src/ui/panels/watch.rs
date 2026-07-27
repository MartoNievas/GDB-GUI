//! Watch/Registers/Data tabs content — extracted from app.rs in slice S3.

use eframe::egui::{self, Color32, ScrollArea, Stroke, Vec2};

use super::util::visible_register_rows;
use crate::ui::app::App;
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
            match app.watch_tab {
                WatchTab::Watch => {
                    for var in &app.state.locals {
                        ui.horizontal(|ui| {
                            ui.add_space(8.0);
                            ui.label(m(&var.name, 11.0, TXT_CYAN));
                            ui.label(m(" = ", 11.0, TXT_DIM));
                            ui.label(m(&var.value, 11.0, TXT_YELLOW));
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
                            ui.horizontal(|ui| {
                                ui.add_space(8.0);
                                ui.label(m(&var.name, 11.0, TXT_CYAN));
                                ui.label(m(" = ", 11.0, TXT_DIM));
                                ui.label(m(&var.value, 11.0, TXT_YELLOW));
                            });
                        }
                    }
                }
                WatchTab::Registers => {
                    if app.state.registers.is_empty() {
                        ui.label(m("Not paused — no register data", 11.0, TXT_DIM).italics());
                    } else {
                        let rows = visible_register_rows(&app.state.register_names, &app.state.registers);

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
                                .num_columns(2)
                                .spacing([12.0, 1.0])
                                .striped(true)
                                .show(ui, |ui| {
                                    for (name, value, _) in &group {
                                        ui.horizontal(|ui| {
                                            ui.add_space(8.0);
                                            ui.label(m(name, 11.0, color));
                                        });
                                        ui.label(m(value, 11.0, TXT_YELLOW));
                                        ui.end_row();
                                    }
                                });
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
