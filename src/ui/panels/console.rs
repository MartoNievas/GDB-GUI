//! Console content — extracted from app.rs in slice S4.

use eframe::egui::{self, Align, Color32, FontId, Frame, Key, Layout, Margin, ScrollArea, TextEdit};

use crate::ui::app::App;
use crate::ui::command::Command;
use crate::ui::theme::*;
use crate::ui::widgets::{hl, m};

pub(crate) fn render(app: &mut App, ui: &mut egui::Ui, ctx: &egui::Context) {
    // Header fijo arriba
    Frame::new()
        .fill(BG_TOPBAR)
        .inner_margin(Margin {
            left: 8,
            right: 8,
            top: 3,
            bottom: 3,
        })
        .show(ui, |ui| {
            ui.label(m("Console", 11.0, TXT_MUTED));
        });
    hl(ui);

    ui.with_layout(Layout::bottom_up(Align::LEFT), |ui| {
        // Input line
        Frame::new()
            .fill(BG_CONSOLE)
            .inner_margin(Margin {
                left: 8,
                right: 8,
                top: 3,
                bottom: 3,
            })
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(m("(gdb)", 12.0, ACCENT));
                    ui.add_space(4.0);
                    let resp = ui.add(
                        TextEdit::singleline(&mut app.console_input)
                            .font(FontId::monospace(12.0))
                            .desired_width(ui.available_width())
                            .frame(false)
                            .text_color(Color32::from_rgb(0xe0, 0xe0, 0xe0)),
                    );
                    if resp.lost_focus() && ctx.input(|i| i.key_pressed(Key::Enter)) {
                        let raw = app.console_input.trim().to_owned();
                        if raw.eq_ignore_ascii_case("flush") {
                            app.console_log.clear();
                        } else if !raw.is_empty() {
                            app.send(Command::Raw(raw));
                        }
                        app.console_input.clear();
                        resp.request_focus();
                    }
                });
            });

        hl(ui);

        ScrollArea::vertical()
            .id_salt("con_log")
            .stick_to_bottom(true)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.with_layout(Layout::top_down(Align::LEFT), |ui| {
                    ui.add_space(2.0);
                    for line in &app.console_log {
                        ui.horizontal(|ui| {
                            ui.add_space(6.0);
                            ui.label(m(line, 11.0, TXT));
                        });
                    }
                    ui.add_space(2.0);
                });
            });
    });
}
