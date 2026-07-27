use eframe::egui::{self, FontId, Margin, ScrollArea, Sense, Stroke, Vec2};
use std::sync::mpsc::{Receiver, Sender};

use super::command::Command;
use super::panels;
use super::panels::watch::WatchTab;
use super::theme::*;
use super::widgets::*;
use crate::state::{DebuggerEvent, DebuggerState, UiEvent};

// ─── Source line for rendering ─────────────────────────────────────────────────

struct SourceLine {
    number: u32,
    text: String,
}

// ─── App ──────────────────────────────────────────────────────────────────────

pub struct App {
    pub state: DebuggerState,
    event_rx: Receiver<DebuggerEvent>,
    cmd_tx: Sender<Command>,

    // UI state
    pub(crate) console_input: String,
    pub(crate) console_log: Vec<String>,
    pub(crate) watch_tab: WatchTab,

    // Collapsible sections
    pub(crate) open_bp: bool,
    pub(crate) open_cmd: bool,
    pub(crate) open_struct: bool,
    pub(crate) open_stack: bool,
    pub(crate) open_files: bool,
    pub(crate) open_thread: bool,

    source_lines: Vec<SourceLine>,
    source_file: Option<String>,

    // Condition column edit buffer, keyed by breakpoint id. Lazily seeded
    // from `bp.condition` the first time each row is rendered.
    pub(crate) bp_cond_buffer: std::collections::HashMap<u32, String>,

    // Struct panel expression edit buffer.
    pub(crate) struct_input: String,
}

impl App {
    pub fn new(
        state: DebuggerState,
        event_rx: Receiver<DebuggerEvent>,
        cmd_tx: Sender<Command>,
    ) -> Self {
        Self {
            state,
            event_rx,
            cmd_tx,
            console_input: String::new(),
            console_log: Vec::new(),
            watch_tab: WatchTab::Watch,
            open_bp: true,
            open_cmd: false,
            open_struct: false,
            open_stack: true,
            open_files: false,
            open_thread: false,
            source_lines: Vec::new(),
            source_file: None,
            bp_cond_buffer: std::collections::HashMap::new(),
            struct_input: String::new(),
        }
    }

    pub(crate) fn send(&self, cmd: Command) {
        let _ = self.cmd_tx.send(cmd);
    }

    /// Refreshes every thread-scoped view: the roster itself plus locals,
    /// stack, registers, disasm, globals, and the struct expression — none
    /// of those commands are thread-qualified, so a thread switch (or a new
    /// pause) invalidates all of them equally. Called identically from both
    /// the pause path and the post-switch path (see design.md Data Flow) so
    /// the two call sites cannot drift apart.
    pub(crate) fn refresh_thread_scoped_views(&self) {
        self.send(Command::RequestThreads);
        self.send(Command::RequestLocals);
        self.send(Command::RequestStack);
        self.send(Command::RequestRegisters);
        self.send(Command::RequestDisasm);
        for name in self.state.global_names.clone() {
            self.send(Command::EvaluateGlobal(name));
        }
        if !self.state.struct_expr.is_empty() {
            self.send(Command::Evaluate(self.state.struct_expr.clone()));
        }
    }

    fn load_source_if_needed(&mut self) {
        let target_file = match self.state.current_file() {
            Some(f) => f.to_owned(),
            None => {
                self.source_lines.clear();
                self.source_file = None;
                return;
            }
        };

        if self.source_file.as_deref() == Some(&target_file) {
            return;
        }

        let content = self.try_load_source(&target_file);

        match content {
            Some(text) => {
                self.source_lines = text
                    .lines()
                    .enumerate()
                    .map(|(i, line)| SourceLine {
                        number: (i + 1) as u32,
                        text: line.to_owned(),
                    })
                    .collect();
                self.source_file = Some(target_file.clone());
                self.console_log.push(format!(
                    "[UI] ✓ Loaded {} ({} lines)",
                    target_file,
                    self.source_lines.len()
                ));
            }
            None => {
                self.console_log
                    .push(format!("[UI] ✗ Could not find source file: {target_file}"));
                self.console_log.push("[UI] Tried:".into());
                self.console_log.push(format!("  1. {target_file}"));
                if let Some(filename) = std::path::Path::new(&target_file).file_name() {
                    self.console_log
                        .push(format!("  2. {}", filename.to_string_lossy()));
                    self.console_log
                        .push(format!("  3. src/{}", filename.to_string_lossy()));
                }
                self.source_lines.clear();
                self.source_file = None;
            }
        }
    }

    fn try_load_source(&self, path: &str) -> Option<String> {
        // 1. Try the path as-is (absolute or relative from CWD)
        if let Ok(content) = std::fs::read_to_string(path) {
            return Some(content);
        }

        if let Some(filename) = std::path::Path::new(path).file_name() {
            if let Ok(content) = std::fs::read_to_string(filename) {
                return Some(content);
            }
        }

        if let Some(filename) = std::path::Path::new(path).file_name() {
            let src_path = format!("src/{}", filename.to_string_lossy());
            if let Ok(content) = std::fs::read_to_string(&src_path) {
                return Some(content);
            }
        }

        None
    }
}

// ─── eframe::App ──────────────────────────────────────────────────────────────

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        apply_theme(ctx);

        while let Ok(event) = self.event_rx.try_recv() {
            match event {
                DebuggerEvent::State(s) => {
                    // GDB can create a second breakpoint on the same resolved line
                    // (e.g. requesting line 11 and line 12, both landing on 12). We
                    // discard the redundant one and remove it from GDB to avoid duplicates.
                    if let crate::state::StateEvent::BreakpointAdded { breakpoint } = &s {
                        if self.state.is_duplicate_breakpoint(breakpoint) {
                            self.send(Command::RemoveBreakpoint(breakpoint.id));
                            continue;
                        }
                    }

                    let was_paused = matches!(s, crate::state::StateEvent::ProgramPaused { .. });
                    let was_loaded = matches!(s, crate::state::StateEvent::ProgramLoaded { .. });
                    let thread_selected =
                        matches!(s, crate::state::StateEvent::ThreadSelected { .. });
                    let new_global_names =
                        if let crate::state::StateEvent::GlobalNamesReceived { names } = &s {
                            Some(names.clone())
                        } else {
                            None
                        };
                    self.state.apply(s);
                    self.load_source_if_needed();
                    if was_loaded {
                        self.send(Command::RequestRegisterNames);
                        self.send(Command::RequestGlobalNames);
                    }
                    if was_paused {
                        self.refresh_thread_scoped_views();
                    }
                    if thread_selected {
                        self.refresh_thread_scoped_views();
                    }
                    if let Some(names) = new_global_names {
                        for name in names {
                            self.send(Command::EvaluateGlobal(name));
                        }
                    }
                }
                DebuggerEvent::Ui(UiEvent::ConsoleOutput(text)) => {
                    self.console_log.push(text);
                }
                DebuggerEvent::Ui(UiEvent::GdbError(err)) => {
                    self.console_log.push(format!("[ERROR] {err}"));
                }
            }
        }

        ctx.request_repaint();

        // ── TOP BAR ───────────────────────────────────────────────────────────
        egui::TopBottomPanel::top("top_bar")
            .frame(flat(BG_TOPBAR).inner_margin(Margin {
                left: 8,
                right: 8,
                top: 4,
                bottom: 4,
            }))
            .show(ctx, |ui| {
                panels::topbar::render(self, ui);
            });

        // ── CONSOLE (bottom) ──────────────────────────────────────────────────
        egui::TopBottomPanel::bottom("console")
            .resizable(true)
            .min_height(50.0)
            .default_height(180.0)
            .frame(flat(BG_CONSOLE))
            .show(ctx, |ui| {
                panels::console::render(self, ui, ctx);
            });

        // ── RIGHT PANEL ───────────────────────────────────────────────────────
        egui::SidePanel::right("right_panel")
            .resizable(true)
            .min_width(180.0)
            .default_width(280.0)
            .frame(flat(BG_PANEL))
            .show(ctx, |ui| {
                // Upper collapsible sections — drag the divider below to resize
                egui::TopBottomPanel::top("right_upper_panel")
                    .resizable(true)
                    .default_height(ui.available_height() * 0.52)
                    .frame(flat(BG_PANEL))
                    .show_inside(ui, |ui| {
                        ScrollArea::vertical()
                            .id_salt("right_upper")
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                ui.set_min_width(ui.available_width());

                                // BREAKPOINTS ──────────────────────────────────────────
                                panels::breakpoints::render(self, ui);

                                // COMMANDS ──────────────────────────────────────────────
                                panels::commands::render(self, ui);

                                // STRUCT ────────────────────────────────────────────────
                                panels::struct_panel::render(self, ui);

                                // STACK ─────────────────────────────────────────────────
                                panels::stack::render(self, ui);

                                // FILES ─────────────────────────────────────────────────
                                panels::files::render(self, ui);

                                // THREAD ────────────────────────────────────────────────
                                panels::thread::render(self, ui);
                            });
                    });

                // Watch / Registers / Data tabs — fill the remaining space
                egui::CentralPanel::default()
                    .frame(flat(BG_PANEL))
                    .show_inside(ui, |ui| {
                        panels::watch::render(self, ui);
                    });
            });

        // ── SOURCE VIEW (central) ─────────────────────────────────────────────
        egui::CentralPanel::default()
            .frame(flat(BG_APP))
            .show(ctx, |ui| {
                ScrollArea::both()
                    .id_salt("source")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                    if self.source_lines.is_empty() {
                        ui.centered_and_justified(|ui| {
                            ui.label(m("No source file loaded", 13.0, TXT_DIM).italics());
                        });
                        return;
                    }

                    let current_line = self.state.current_line();

                    for line in &self.source_lines {
                        let is_current = Some(line.number) == current_line;
                        let file_ref = self.source_file.as_deref().unwrap_or("");
                        // The marker is only drawn on the actual line where GDB stops;
                        // the toggle also accepts the requested line (in case GDB relocated it).
                        let has_bp = self.state.has_breakpoint_marker(file_ref, line.number);
                        let bp_id = self
                            .state
                            .breakpoint_at(file_ref, line.number)
                            .map(|b| b.id);

                        let response = source_row(ui, line.number, &line.text, is_current, has_bp);
                        if response.clicked() {
                            if let Some(id) = bp_id {
                                self.send(Command::RemoveBreakpoint(id));
                            } else if let Some(file) = self.source_file.clone() {
                                self.send(Command::AddBreakpoint {
                                    file,
                                    line: line.number,
                                    condition: None,
                                });
                            }
                        }
                    }
                });
            });
    }
}

// ─── Source row ───────────────────────────────────────────────────────────────

fn source_row(
    ui: &mut egui::Ui,
    line_no: u32,
    code: &str,
    is_current: bool,
    has_bp: bool,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(
        Vec2::new(f32::max(ui.available_width(), 900.0), 18.0),
        Sense::click(),
    );

    if response.hovered() {
        ui.ctx()
            .output_mut(|o| o.cursor_icon = egui::CursorIcon::PointingHand);
    }

    let p = ui.painter();
    let cy = rect.center().y;

    if is_current {
        p.rect_filled(rect, 0.0, BG_LINE_HL);
        p.line_segment(
            [rect.left_top(), rect.left_bottom()],
            Stroke::new(2.0_f32, ACCENT),
        );
    }

    if has_bp {
        p.circle_filled(egui::pos2(rect.left() + 9.0, cy), 5.0, RED);
    }

    // Line number – right-aligned in a 56 px gutter
    p.text(
        egui::pos2(rect.left() + 56.0, cy),
        egui::Align2::RIGHT_CENTER,
        format!("{line_no}"),
        FontId::monospace(12.0),
        if has_bp { RED } else { TXT_DIM },
    );

    // Code
    p.text(
        egui::pos2(rect.left() + 66.0, cy),
        egui::Align2::LEFT_CENTER,
        code,
        FontId::monospace(12.5),
        if is_current { TXT_HL } else { TXT },
    );

    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::DebuggerState;
    use std::sync::mpsc;

    fn test_app() -> (App, Receiver<Command>) {
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (_event_tx, event_rx) = mpsc::channel();
        let app = App::new(DebuggerState::new(), event_rx, cmd_tx);
        (app, cmd_rx)
    }

    // Pins the spec MUST at specs/thread-selection/spec.md:53: both the
    // pause and post-switch paths call this single shared function, so it
    // alone is responsible for the roster re-fetch — no drift possible
    // between two copies.
    #[test]
    fn refresh_thread_scoped_views_sends_request_threads_alongside_existing_refreshes() {
        let (app, cmd_rx) = test_app();
        app.refresh_thread_scoped_views();
        let sent: Vec<Command> = cmd_rx.try_iter().collect();

        assert!(sent.contains(&Command::RequestThreads));
        assert!(sent.contains(&Command::RequestLocals));
        assert!(sent.contains(&Command::RequestStack));
        assert!(sent.contains(&Command::RequestRegisters));
        assert!(sent.contains(&Command::RequestDisasm));
    }

    // Mirrors the existing pause-refresh behavior: no struct expression has
    // ever been committed → no Evaluate command is sent.
    #[test]
    fn refresh_thread_scoped_views_sends_no_evaluate_when_struct_expr_empty() {
        let (app, cmd_rx) = test_app();
        app.refresh_thread_scoped_views();
        let sent: Vec<Command> = cmd_rx.try_iter().collect();

        assert!(!sent.iter().any(|c| matches!(c, Command::Evaluate(_))));
    }
}
