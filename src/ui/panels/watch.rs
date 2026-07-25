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
