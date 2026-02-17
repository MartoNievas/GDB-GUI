use std::{
    io::{BufRead, BufReader, Write},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    sync::mpsc::{Receiver, Sender},
    thread,
};

use super::parser::parse_line;
use super::writer::command_to_mi;
use crate::state::{DebuggerEvent, StateEvent, UiEvent};
use crate::ui::command::Command as DebuggerCommand;

struct GdbWriter {
    stdin: ChildStdin,
    seq: u32,
}

impl GdbWriter {
    fn send(&mut self, raw_mi: &str) -> std::io::Result<()> {
        writeln!(self.stdin, "{}{}", self.seq, raw_mi)?;
        self.stdin.flush()?;
        self.seq += 1;
        Ok(())
    }
}

// ─── Spawn ────────────────────────────────────────────────────────────────────

fn spawn_gdb(
    executable: Option<&str>,
) -> std::io::Result<(Child, GdbWriter, BufReader<ChildStdout>)> {
    let mut cmd = Command::new("gdb");
    cmd.arg("--interpreter=mi")
        .arg("--quiet")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());

    if let Some(exe) = executable {
        cmd.arg(exe);
    }

    let mut child = cmd.spawn()?;
    let stdin = child.stdin.take().expect("stdin piped");
    let stdout_raw = child.stdout.take().expect("stdout piped");

    let writer = GdbWriter { stdin, seq: 1 };
    let reader = BufReader::new(stdout_raw);

    Ok((child, writer, reader))
}

// ─── run_loop ─────────────────────────────────────────────────────────────────

pub fn run_loop(
    executable: Option<String>,
    cmd_rx: Receiver<DebuggerCommand>,
    event_tx: Sender<DebuggerEvent>,
) {
    let (mut child, mut writer, reader) = match spawn_gdb(executable.as_deref()) {
        Ok(parts) => parts,
        Err(e) => {
            let _ = event_tx.send(DebuggerEvent::Ui(UiEvent::GdbError(format!(
                "No se pudo lanzar GDB: {e}"
            ))));
            return;
        }
    };

    if let Some(exe) = &executable {
        let _ = event_tx.send(DebuggerEvent::State(StateEvent::ProgramLoaded {
            executable: exe.clone(),
        }));
    }

    let (line_tx, line_rx) = std::sync::mpsc::channel::<String>();
    let event_tx_reader = event_tx.clone();

    thread::spawn(move || {
        let mut reader = reader;
        let mut buf = String::new();
        loop {
            buf.clear();
            match reader.read_line(&mut buf) {
                Ok(0) => break, // EOF
                Ok(_) => {
                    let line = buf.trim_end_matches('\n').trim_end_matches('\r').to_owned();
                    if !line.is_empty() && line_tx.send(line).is_err() {
                        break;
                    }
                }
                Err(e) => {
                    let _ = event_tx_reader.send(DebuggerEvent::Ui(UiEvent::GdbError(format!(
                        "Error leyendo GDB: {e}"
                    ))));
                    break;
                }
            }
        }
    });

    loop {
        while let Ok(cmd) = cmd_rx.try_recv() {
            let mi = command_to_mi(&cmd);

            let _ = event_tx.send(DebuggerEvent::Ui(UiEvent::ConsoleOutput(format!("> {mi}"))));

            if let Err(e) = writer.send(&mi) {
                let _ = event_tx.send(DebuggerEvent::Ui(UiEvent::GdbError(format!(
                    "Error escribiendo a GDB: {e}"
                ))));
                let _ = child.kill();
                return;
            }
        }

        while let Ok(line) = line_rx.try_recv() {
            let _ = event_tx.send(DebuggerEvent::Ui(UiEvent::ConsoleOutput(line.clone())));

            if let Some(event) = parse_line(&line) {
                // None = línea ignorable, no es error
                if event_tx.send(event).is_err() {
                    let _ = child.kill();
                    return; // UI cerrada
                }
            }
        }

        thread::sleep(std::time::Duration::from_millis(10));
    }
}
