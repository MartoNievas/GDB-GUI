use std::sync::mpsc;
use std::thread;

mod gdb;
mod state;
mod ui;

use state::DebuggerState;
use ui::{App, command::Command};

fn main() -> eframe::Result<()> {
    // Canales bidireccionales
    let (cmd_tx, cmd_rx) = mpsc::channel::<Command>();
    let (event_tx, event_rx) = mpsc::channel::<state::DebuggerEvent>();

    // Obtener ejecutable del argv (opcional)
    let executable = std::env::args().nth(1);

    // Lanzar hilo GDB
    thread::spawn(move || {
        gdb::run_loop(executable, cmd_rx, event_tx);
    });

    // Lanzar UI
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("GDB GUI")
            .with_inner_size([1400.0, 800.0]),
        ..Default::default()
    };

    eframe::run_native(
        "GDB GUI",
        native_options,
        Box::new(|_cc| {
            let state = DebuggerState::new();
            Ok(Box::new(App::new(state, event_rx, cmd_tx)))
        }),
    )
}
