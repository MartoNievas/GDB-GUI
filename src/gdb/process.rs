use std::{
    collections::VecDeque,
    io::{BufRead, BufReader, Write},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    sync::mpsc::{Receiver, Sender},
    thread,
};

use super::parser::{extract_str, parse_line};
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
        .arg("-nx")
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

    // Cola FIFO de nombres de variables globales pendientes de evaluar. GDB responde a
    // comandos síncronos (-data-evaluate-expression, etc.) en el mismo orden en que se
    // mandan, así que podemos correlacionar cada "^done,value=..." sin nombre propio con
    // el nombre que le corresponde simplemente desencolando en orden de llegada.
    let mut pending_globals: VecDeque<String> = VecDeque::new();

    loop {
        while let Ok(cmd) = cmd_rx.try_recv() {
            let mi = command_to_mi(&cmd);

            if let DebuggerCommand::EvaluateGlobal(name) = &cmd {
                pending_globals.push_back(name.clone());
            }

            let _ = event_tx.send(DebuggerEvent::Ui(UiEvent::ConsoleOutput(format!("> {mi}"))));

            if let Err(e) = writer.send(&mi) {
                let _ = event_tx.send(DebuggerEvent::Ui(UiEvent::GdbError(format!(
                    "Error escribiendo a GDB: {e}"
                ))));
                let _ = child.kill();
                return;
            }

            // GDB responde a `-break-delete` con un simple `^done` sin `=breakpoint-deleted`
            // ni el id borrado, así que la respuesta no se puede correlacionar. Emitimos el
            // evento de eliminación nosotros mismos para que la UI lo refleje.
            if let DebuggerCommand::RemoveBreakpoint(id) = &cmd {
                let _ = event_tx.send(DebuggerEvent::State(StateEvent::BreakpointRemoved {
                    id: *id,
                }));
            }
        }

        while let Ok(line) = line_rx.try_recv() {
            // Los stream records (~ @ &) los convierte parse_line en texto limpio;
            // echoar además la línea cruda duplicaría la salida en la consola.
            let first = line
                .trim_start_matches(|c: char| c.is_ascii_digit())
                .chars()
                .next();
            let is_stream = matches!(first, Some('~') | Some('@') | Some('&'));
            if !is_stream {
                let _ = event_tx.send(DebuggerEvent::Ui(UiEvent::ConsoleOutput(line.clone())));
            }

            if !pending_globals.is_empty() && is_bare_value_done(&line) {
                if let Some(name) = pending_globals.pop_front() {
                    if let Some(value) = extract_str(&line, "value") {
                        let event =
                            DebuggerEvent::State(StateEvent::GlobalValueUpdated { name, value });
                        if event_tx.send(event).is_err() {
                            let _ = child.kill();
                            return;
                        }
                    }
                }
                continue;
            }

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

// ─── Helpers ───────────────────────────────────────────────────────────────

/// true si la línea es exactamente `^done,value="..."`, la respuesta de
/// -data-evaluate-expression sin ningún otro campo.
fn is_bare_value_done(line: &str) -> bool {
    line.trim_start_matches(|c: char| c.is_ascii_digit())
        .starts_with("^done,value=\"")
}
