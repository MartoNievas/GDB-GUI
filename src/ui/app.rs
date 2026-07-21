use eframe::egui::{
    self, Align, Color32, FontId, Frame, Key, Layout, Margin, RichText, ScrollArea, Sense, Stroke,
    TextEdit, Vec2,
};
use std::sync::mpsc::{Receiver, Sender};

use super::command::Command;
use super::registers::{RegClass, classify, display_order, is_x86_32_gp};
use crate::state::{DebuggerEvent, DebuggerState, UiEvent};

// ─── Palette ──────────────────────────────────────────────────────────────────

const BG_APP: Color32 = Color32::from_rgb(0x11, 0x11, 0x11);
const BG_TOPBAR: Color32 = Color32::from_rgb(0x1a, 0x1a, 0x1a);
const BG_PANEL: Color32 = Color32::from_rgb(0x14, 0x14, 0x14);
const BG_CONSOLE: Color32 = Color32::from_rgb(0x0f, 0x0f, 0x0f);
const BG_HOVER: Color32 = Color32::from_rgb(0x22, 0x22, 0x22);
const BG_LINE_HL: Color32 = Color32::from_rgb(0x18, 0x2b, 0x18);
const SEP_COLOR: Color32 = Color32::from_rgb(0x28, 0x28, 0x28);

const ACCENT: Color32 = Color32::from_rgb(0x00, 0xcc, 0x44);
const RED: Color32 = Color32::from_rgb(0xcc, 0x44, 0x44);
const BLUE: Color32 = Color32::from_rgb(0x44, 0x88, 0xcc);

const TXT: Color32 = Color32::from_rgb(0xb0, 0xc4, 0xb0);
const TXT_DIM: Color32 = Color32::from_rgb(0x44, 0x44, 0x44);
const TXT_MUTED: Color32 = Color32::from_rgb(0x77, 0x77, 0x77);
const TXT_CYAN: Color32 = Color32::from_rgb(0x7e, 0xc8, 0xe3);
const TXT_YELLOW: Color32 = Color32::from_rgb(0xe8, 0xc9, 0x7d);
const TXT_HL: Color32 = Color32::from_rgb(0xd4, 0xf0, 0xd4);

// ─── Register groups (categoría → etiqueta + color) ───────────────────────────
// Orden en que se muestran los grupos en la pestaña Registers.
const REG_GROUPS: [(RegClass, &str, Color32); 5] = [
    (RegClass::General, "General purpose", TXT_CYAN),
    (RegClass::Control, "Control / flags", ACCENT),
    (RegClass::Segment, "Segment", TXT_MUTED),
    (RegClass::Simd, "SIMD / FP", BLUE),
    (RegClass::Other, "Other", TXT_DIM),
];

// ─── UI-only tab state ────────────────────────────────────────────────────────

#[derive(Default, PartialEq, Clone, Copy)]
enum WatchTab {
    #[default]
    Watch,
    Registers,
    Data,
}

// ─── Source line para renderizado ─────────────────────────────────────────────

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
    console_input: String,
    console_log: Vec<String>,
    watch_tab: WatchTab,

    // Collapsible sections
    open_bp: bool,
    open_cmd: bool,
    open_struct: bool,
    open_stack: bool,
    open_files: bool,
    open_thread: bool,

    source_lines: Vec<SourceLine>,
    source_file: Option<String>,
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
        }
    }

    fn send(&self, cmd: Command) {
        let _ = self.cmd_tx.send(cmd);
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
        // 1. Intentar path tal cual (absoluto o relativo desde CWD)
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
                    // GDB puede crear un segundo breakpoint en la misma línea resuelta
                    // (p.ej. al pedir la línea 11 y la 12, ambas caen en la 12). Descartamos
                    // el redundante y lo borramos de GDB para no duplicar.
                    if let crate::state::StateEvent::BreakpointAdded { breakpoint } = &s {
                        if self.state.is_duplicate_breakpoint(breakpoint) {
                            self.send(Command::RemoveBreakpoint(breakpoint.id));
                            continue;
                        }
                    }

                    let was_paused = matches!(s, crate::state::StateEvent::ProgramPaused { .. });
                    let was_loaded = matches!(s, crate::state::StateEvent::ProgramLoaded { .. });
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
                        self.send(Command::RequestLocals);
                        self.send(Command::RequestStack);
                        self.send(Command::RequestRegisters);
                        self.send(Command::RequestDisasm);
                        for name in self.state.global_names.clone() {
                            self.send(Command::EvaluateGlobal(name));
                        }
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
                ui.horizontal(|ui| {
                    ui.label(m("GDB GUI", 13.0, ACCENT).strong());
                    ui.add(egui::Separator::default().vertical());

                    if tbtn(ui, "Run", true).clicked() {
                        self.send(Command::Run);
                    }
                    if tbtn(ui, "Continue", false).clicked() {
                        self.send(Command::Continue);
                    }
                    if tbtn(ui, "Step", false).clicked() {
                        self.send(Command::Step);
                    }
                    if tbtn(ui, "Next", false).clicked() {
                        self.send(Command::Next);
                    }
                    if tbtn(ui, "Finish", false).clicked() {
                        self.send(Command::Finish);
                    }
                    if tbtn(ui, "Restart", false).clicked() {
                        self.send(Command::Restart);
                    }

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let (r, _) = ui.allocate_exact_size(Vec2::splat(12.0), Sense::hover());
                        let color = if self.state.is_running() {
                            ACCENT
                        } else if self.state.is_paused() {
                            TXT_YELLOW
                        } else {
                            TXT_DIM
                        };
                        ui.painter().rect_filled(r, 2.0, color);
                        ui.add_space(6.0);

                        let status = match &self.state.program {
                            crate::state::ProgramState::NoProgramLoaded => "No program loaded",
                            crate::state::ProgramState::ProgramLoaded => "Loaded",
                            crate::state::ProgramState::Running => "Running",
                            crate::state::ProgramState::Paused => "Paused",
                            crate::state::ProgramState::Exited { .. } => "Exited",
                        };

                        let location = if let (Some(file), Some(func)) =
                            (self.state.current_file(), self.state.current_function())
                        {
                            format!("{file} — {func}")
                        } else {
                            status.to_owned()
                        };

                        ui.label(m(&location, 11.0, TXT_MUTED));
                    });
                });
            });

        // ── CONSOLE (bottom) ──────────────────────────────────────────────────
        egui::TopBottomPanel::bottom("console")
            .resizable(true)
            .min_height(50.0)
            .default_height(180.0)
            .frame(flat(BG_CONSOLE))
            .show(ctx, |ui| {
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
                                    TextEdit::singleline(&mut self.console_input)
                                        .font(FontId::monospace(12.0))
                                        .desired_width(ui.available_width())
                                        .frame(false)
                                        .text_color(Color32::from_rgb(0xe0, 0xe0, 0xe0)),
                                );
                                if resp.lost_focus() && ctx.input(|i| i.key_pressed(Key::Enter)) {
                                    let raw = self.console_input.trim().to_owned();
                                    if !raw.is_empty() {
                                        self.send(Command::Raw(raw));
                                        self.console_input.clear();
                                    }
                                    resp.request_focus();
                                }
                            });
                        });

                    hl(ui);

                    ScrollArea::vertical()
                        .id_salt("con_log")
                        .stick_to_bottom(true)
                        .show(ui, |ui| {
                            ui.add_space(2.0);
                            for line in &self.console_log {
                                ui.horizontal(|ui| {
                                    ui.add_space(6.0);
                                    ui.label(m(line, 11.0, TXT));
                                });
                            }
                            ui.add_space(2.0);
                        });
                });
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
                                sec_hdr(ui, "Breakpoints", &mut self.open_bp);
                                if self.open_bp {
                                    egui::Grid::new("bp_grid")
                                        .num_columns(3)
                                        .spacing([8.0, 2.0])
                                        .show(ui, |ui| {
                                            for h in ["File", "Line", ""] {
                                                ui.label(m(h, 11.0, TXT_DIM));
                                            }
                                            ui.end_row();

                                            for bp in &self.state.persistent.breakpoints {
                                                // Nombre corto del archivo
                                                let short_file = bp
                                                    .file
                                                    .split('/')
                                                    .last()
                                                    .or_else(|| bp.file.split('\\').last())
                                                    .unwrap_or(&bp.file);

                                                ui.label(m(short_file, 12.0, TXT_CYAN));
                                                ui.label(m(&bp.line.to_string(), 12.0, TXT_YELLOW));
                                                if ui
                                                    .add(
                                                        egui::Button::new(m("×", 12.0, RED))
                                                            .fill(Color32::TRANSPARENT)
                                                            .stroke(Stroke::NONE),
                                                    )
                                                    .clicked()
                                                {
                                                    self.send(Command::RemoveBreakpoint(bp.id));
                                                }
                                                ui.end_row();
                                            }
                                        });
                                    ui.add_space(4.0);
                                }
                                hl(ui);

                                // COMMANDS ──────────────────────────────────────────────
                                sec_hdr(ui, "Commands", &mut self.open_cmd);
                                if self.open_cmd {
                                    for cmd_str in &[
                                        "info locals",
                                        "bt full",
                                        "info registers",
                                        "info threads",
                                    ] {
                                        if ui
                                            .add(
                                                egui::Button::new(m(cmd_str, 11.0, TXT_CYAN))
                                                    .fill(Color32::TRANSPARENT)
                                                    .stroke(Stroke::NONE)
                                                    .min_size(Vec2::new(
                                                        ui.available_width(),
                                                        18.0,
                                                    )),
                                            )
                                            .clicked()
                                        {
                                            self.send(Command::Raw(cmd_str.to_string()));
                                        }
                                    }
                                    ui.add_space(4.0);
                                }
                                hl(ui);

                                // STRUCT ────────────────────────────────────────────────
                                sec_hdr(ui, "Struct", &mut self.open_struct);
                                if self.open_struct {
                                    ui.horizontal(|ui| {
                                        ui.add_space(8.0);
                                        ui.label(
                                            RichText::new("No struct selected")
                                                .color(TXT_DIM)
                                                .font(FontId::monospace(11.0))
                                                .italics(),
                                        );
                                    });
                                    ui.add_space(4.0);
                                }
                                hl(ui);

                                // STACK ─────────────────────────────────────────────────
                                sec_hdr(ui, "Stack", &mut self.open_stack);
                                if self.open_stack {
                                    if let Some(pause) = &self.state.pause {
                                        egui::Grid::new("stack_grid")
                                            .num_columns(3)
                                            .spacing([6.0, 2.0])
                                            .show(ui, |ui| {
                                                for h in ["#", "Function", "Location"] {
                                                    ui.label(m(h, 11.0, TXT_DIM));
                                                }
                                                ui.end_row();

                                                for (idx, frame) in pause.stack.iter().enumerate() {
                                                    let active = idx == 0;

                                                    let (stripe, _) = ui.allocate_exact_size(
                                                        Vec2::new(2.0, 14.0),
                                                        Sense::hover(),
                                                    );
                                                    if active {
                                                        ui.painter().rect_filled(stripe, 0.0, BLUE);
                                                    }

                                                    let fn_col =
                                                        if active { BLUE } else { TXT_CYAN };
                                                    ui.label(m(&idx.to_string(), 11.0, TXT_DIM));
                                                    ui.label(m(&frame.function, 11.0, fn_col));

                                                    let loc = if let (Some(file), Some(line)) =
                                                        (&frame.file, frame.line)
                                                    {
                                                        let short = file
                                                            .split('/')
                                                            .last()
                                                            .or_else(|| file.split('\\').last())
                                                            .unwrap_or(file);
                                                        format!("{short}:{line}")
                                                    } else {
                                                        format!("0x{:x}", frame.addr)
                                                    };
                                                    ui.label(m(&loc, 11.0, TXT_MUTED));
                                                    ui.end_row();
                                                }
                                            });
                                    } else {
                                        ui.label(m("Not paused", 11.0, TXT_DIM).italics());
                                    }
                                    ui.add_space(4.0);
                                }
                                hl(ui);

                                // FILES ─────────────────────────────────────────────────
                                sec_hdr(ui, "Files", &mut self.open_files);
                                if self.open_files {
                                    if let Some(exe) = &self.state.persistent.executable {
                                        ui.horizontal(|ui| {
                                            ui.add_space(8.0);
                                            ui.label(m(&format!("📄 {exe}"), 11.0, TXT_CYAN));
                                        });
                                    }
                                    ui.add_space(4.0);
                                }
                                hl(ui);

                                // THREAD ────────────────────────────────────────────────
                                sec_hdr(ui, "Thread", &mut self.open_thread);
                                if self.open_thread {
                                    if let Some(pause) = &self.state.pause {
                                        ui.horizontal(|ui| {
                                            ui.add_space(8.0);
                                            let (r, _) = ui.allocate_exact_size(
                                                Vec2::splat(8.0),
                                                Sense::hover(),
                                            );
                                            ui.painter().circle_filled(r.center(), 4.0, ACCENT);
                                            ui.add_space(4.0);
                                            ui.label(m(
                                                &format!("Thread {}", pause.thread_id),
                                                11.0,
                                                TXT_MUTED,
                                            ));
                                        });
                                    }
                                    ui.add_space(4.0);
                                }
                                hl(ui);
                            });
                    });

                // Watch / Registers / Data tabs — fill the remaining space
                egui::CentralPanel::default()
                    .frame(flat(BG_PANEL))
                    .show_inside(ui, |ui| {
                        ui.add_space(2.0);
                        ui.horizontal(|ui| {
                            for (label, tab) in [
                                ("Watch", WatchTab::Watch),
                                ("Registers", WatchTab::Registers),
                                ("Data", WatchTab::Data),
                            ] {
                                let active = self.watch_tab == tab;
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
                                    self.watch_tab = tab;
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
                                match self.watch_tab {
                                    WatchTab::Watch => {
                                        for var in &self.state.locals {
                                            ui.horizontal(|ui| {
                                                ui.add_space(8.0);
                                                ui.label(m(&var.name, 11.0, TXT_CYAN));
                                                ui.label(m(" = ", 11.0, TXT_DIM));
                                                ui.label(m(&var.value, 11.0, TXT_YELLOW));
                                            });
                                        }
                                        if self.state.locals.is_empty() {
                                            ui.label(m("No locals", 11.0, TXT_DIM).italics());
                                        }

                                        if !self.state.globals.is_empty() {
                                            ui.add_space(6.0);
                                            ui.horizontal(|ui| {
                                                ui.add_space(8.0);
                                                ui.label(m("Globals", 10.0, TXT_DIM).italics());
                                            });
                                            for var in &self.state.globals {
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
                                        if self.state.registers.is_empty() {
                                            ui.label(
                                                m("Not paused — no register data", 11.0, TXT_DIM)
                                                    .italics(),
                                            );
                                        } else {
                                            let names = &self.state.register_names;
                                            // En x86-64, eax/ebx/... son la parte baja
                                            // de rax/rbx/...; solo las ocultamos si el
                                            // registro de 64 bits está presente.
                                            let is_x86_64 = names.iter().any(|n| n == "rax");

                                            // (nombre, valor, categoría)
                                            let rows: Vec<(String, &str, RegClass)> = self
                                                .state
                                                .registers
                                                .iter()
                                                .filter_map(|r| {
                                                    // GDB deja algunos slots con nombre
                                                    // vacío; mostramos solo los que tienen
                                                    // nombre real.
                                                    let name = names.get(r.number as usize)?;
                                                    if name.is_empty() {
                                                        return None;
                                                    }
                                                    if is_x86_64 && is_x86_32_gp(name) {
                                                        return None;
                                                    }
                                                    let class = classify(name);
                                                    Some((name.clone(), r.value.as_str(), class))
                                                })
                                                .collect();

                                            for (class, label, color) in REG_GROUPS {
                                                let mut group: Vec<_> = rows
                                                    .iter()
                                                    .filter(|(_, _, c)| *c == class)
                                                    .collect();
                                                if group.is_empty() {
                                                    continue;
                                                }
                                                // El grupo general va en orden convencional;
                                                // el resto conserva el orden de GDB.
                                                if class == RegClass::General {
                                                    group.sort_by_key(|(name, _, _)| {
                                                        display_order(name)
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
                                        if self.state.disasm.is_empty() {
                                            ui.label(m("Not paused", 11.0, TXT_DIM).italics());
                                        } else {
                                            let cur_addr = self.state.current_addr();
                                            for asm in &self.state.disasm {
                                                let is_current = Some(asm.addr) == cur_addr;
                                                let col = if is_current { TXT_HL } else { TXT };
                                                ui.horizontal(|ui| {
                                                    if is_current {
                                                        ui.label(m("▶", 11.0, ACCENT));
                                                    } else {
                                                        ui.add_space(14.0);
                                                    }
                                                    ui.label(m(
                                                        &format!("0x{:x}", asm.addr),
                                                        11.0,
                                                        TXT_DIM,
                                                    ));
                                                    ui.add_space(6.0);
                                                    ui.label(m(&asm.inst, 11.0, col));
                                                });
                                            }
                                        }
                                    }
                                }
                            });
                    });
            });

        // ── SOURCE VIEW (central) ─────────────────────────────────────────────
        egui::CentralPanel::default()
            .frame(flat(BG_APP))
            .show(ctx, |ui| {
                ScrollArea::both().id_salt("source").show(ui, |ui| {
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
                        // El marcador solo se dibuja en la línea real donde GDB para;
                        // el toggle admite además la línea solicitada (por si GDB reubicó).
                        let has_bp = self.state.has_breakpoint_marker(file_ref, line.number);
                        let bp_id = self.state.breakpoint_at(file_ref, line.number).map(|b| b.id);

                        let response = source_row(ui, line.number, &line.text, is_current, has_bp);
                        if response.clicked() {
                            if let Some(id) = bp_id {
                                self.send(Command::RemoveBreakpoint(id));
                            } else if let Some(file) = self.source_file.clone() {
                                self.send(Command::AddBreakpoint {
                                    file,
                                    line: line.number,
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

// ─── Micro-helpers ────────────────────────────────────────────────────────────

#[inline]
fn m(text: &str, size: f32, color: Color32) -> RichText {
    RichText::new(text)
        .font(FontId::monospace(size))
        .color(color)
}

#[inline]
fn flat(bg: Color32) -> Frame {
    Frame::new().fill(bg)
}

fn tbtn(ui: &mut egui::Ui, label: &str, accent: bool) -> egui::Response {
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

fn sec_hdr(ui: &mut egui::Ui, label: &str, open: &mut bool) {
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

fn hl(ui: &mut egui::Ui) {
    let y = ui.cursor().top();
    ui.painter()
        .hline(ui.max_rect().x_range(), y, Stroke::new(1.0_f32, SEP_COLOR));
    ui.add_space(1.0);
}

fn apply_theme(ctx: &egui::Context) {
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
