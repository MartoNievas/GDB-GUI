//! Per-panel content renderers extracted from `src/ui/app.rs`.
//!
//! Each submodule exposes `pub(crate) fn render(app: &mut App, ui: &mut egui::Ui)`
//! (console additionally takes `&egui::Context`). The egui panel/ScrollArea
//! skeleton stays in `app.rs`; only panel *content* lives here.

pub(crate) mod breakpoints;
pub(crate) mod commands;
pub(crate) mod console;
pub(crate) mod files;
pub(crate) mod stack;
pub(crate) mod struct_panel;
pub(crate) mod thread;
pub(crate) mod topbar;
pub(crate) mod util;
pub(crate) mod watch;
