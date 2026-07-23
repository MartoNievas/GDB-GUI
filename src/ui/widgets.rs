use eframe::egui::{self, Color32, FontId, Frame, RichText, Stroke, Vec2};

use super::theme::{ACCENT, BG_TOPBAR, SEP_COLOR};

#[inline]
pub(crate) fn m(text: &str, size: f32, color: Color32) -> RichText {
    RichText::new(text)
        .font(FontId::monospace(size))
        .color(color)
}

#[inline]
pub(crate) fn flat(bg: Color32) -> Frame {
    Frame::new().fill(bg)
}

pub(crate) fn tbtn(ui: &mut egui::Ui, label: &str, accent: bool) -> egui::Response {
    ui.add(
        egui::Button::new(m(
            label,
            12.0,
            if accent {
                ACCENT
            } else {
                Color32::from_rgb(0xaa, 0xaa, 0xaa)
            },
        ))
        .fill(if accent {
            Color32::from_rgb(0x1a, 0x3a, 0x1a)
        } else {
            BG_TOPBAR
        })
        .stroke(Stroke::new(1.0_f32, SEP_COLOR))
        .min_size(Vec2::new(0.0, 22.0)),
    )
}

pub(crate) fn sec_hdr(ui: &mut egui::Ui, label: &str, open: &mut bool) {
    let icon = if *open { "▾" } else { "▸" };
    if ui
        .add(
            egui::Button::new(m(
                &format!("{icon}  {label}"),
                12.0,
                Color32::from_rgb(0xcc, 0xcc, 0xcc),
            ))
            .fill(BG_TOPBAR)
            .stroke(Stroke::NONE)
            .min_size(Vec2::new(ui.available_width(), 22.0)),
        )
        .clicked()
    {
        *open = !*open;
    }
}

pub(crate) fn hl(ui: &mut egui::Ui) {
    let y = ui.cursor().top();
    ui.painter()
        .hline(ui.max_rect().x_range(), y, Stroke::new(1.0_f32, SEP_COLOR));
    ui.add_space(1.0);
}
