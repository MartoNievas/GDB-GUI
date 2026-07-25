use eframe::egui::{self, Color32, Stroke};

use super::registers::RegClass;

// ─── Palette ──────────────────────────────────────────────────────────────────

pub(crate) const BG_APP: Color32 = Color32::from_rgb(0x11, 0x11, 0x11);
pub(crate) const BG_TOPBAR: Color32 = Color32::from_rgb(0x1a, 0x1a, 0x1a);
pub(crate) const BG_PANEL: Color32 = Color32::from_rgb(0x14, 0x14, 0x14);
pub(crate) const BG_CONSOLE: Color32 = Color32::from_rgb(0x0f, 0x0f, 0x0f);
pub(crate) const BG_HOVER: Color32 = Color32::from_rgb(0x22, 0x22, 0x22);
pub(crate) const BG_LINE_HL: Color32 = Color32::from_rgb(0x18, 0x2b, 0x18);
pub(crate) const SEP_COLOR: Color32 = Color32::from_rgb(0x28, 0x28, 0x28);

pub(crate) const ACCENT: Color32 = Color32::from_rgb(0x00, 0xcc, 0x44);
pub(crate) const RED: Color32 = Color32::from_rgb(0xcc, 0x44, 0x44);
pub(crate) const BLUE: Color32 = Color32::from_rgb(0x44, 0x88, 0xcc);

pub(crate) const TXT: Color32 = Color32::from_rgb(0xb0, 0xc4, 0xb0);
pub(crate) const TXT_DIM: Color32 = Color32::from_rgb(0x44, 0x44, 0x44);
pub(crate) const TXT_MUTED: Color32 = Color32::from_rgb(0x77, 0x77, 0x77);
pub(crate) const TXT_CYAN: Color32 = Color32::from_rgb(0x7e, 0xc8, 0xe3);
pub(crate) const TXT_YELLOW: Color32 = Color32::from_rgb(0xe8, 0xc9, 0x7d);
pub(crate) const TXT_HL: Color32 = Color32::from_rgb(0xd4, 0xf0, 0xd4);

// ─── Register groups (category → label + color) ───────────────────────────────
// Order in which the groups are displayed in the Registers tab.
pub(crate) const REG_GROUPS: [(RegClass, &str, Color32); 5] = [
    (RegClass::General, "General purpose", TXT_CYAN),
    (RegClass::Control, "Control / flags", ACCENT),
    (RegClass::Segment, "Segment", TXT_MUTED),
    (RegClass::Simd, "SIMD / FP", BLUE),
    (RegClass::Other, "Other", TXT_DIM),
];

pub(crate) fn apply_theme(ctx: &egui::Context) {
    let mut v = egui::Visuals::dark();
    v.panel_fill = BG_APP;
    v.window_fill = BG_APP;
    v.extreme_bg_color = BG_CONSOLE;
    v.faint_bg_color = BG_TOPBAR;
    v.widgets.noninteractive.bg_fill = BG_TOPBAR;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, SEP_COLOR);
    v.widgets.inactive.bg_fill = BG_TOPBAR;
    v.widgets.hovered.bg_fill = BG_HOVER;
    v.widgets.active.bg_fill = BG_HOVER;
    v.override_text_color = Some(TXT);
    ctx.set_visuals(v);
}
