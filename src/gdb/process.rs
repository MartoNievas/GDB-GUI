use std::{
    collections::{HashMap, VecDeque},
    io::{BufRead, BufReader, Write},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    sync::mpsc::{Receiver, Sender},
    thread,
};

use super::parser::{extract_str, parse_line, parse_token};
use super::writer::{GdbAction, dispatch};
use crate::state::{DebuggerEvent, StateEvent, UiEvent};
use crate::ui::command::Command as DebuggerCommand;

/// Generic over `W: Write` so unit tests can substitute an in-memory buffer
/// instead of a real `ChildStdin` (which requires a live subprocess).
struct GdbWriter<W: Write> {
    stdin: W,
    seq: u32,
}

impl<W: Write> GdbWriter<W> {
    /// Writes `"{seq}{raw_mi}\n"` and returns the token (`seq` before
    /// increment) it used — callers correlate GDB's reply to this token.
    fn send(&mut self, raw_mi: &str) -> std::io::Result<u32> {
        let token = self.seq;
        writeln!(self.stdin, "{}{}", token, raw_mi)?;
        self.stdin.flush()?;
        self.seq += 1;
        Ok(token)
    }
}

/// Inspects an incoming raw MI line for a token that correlates to a pending
/// `-break-condition` command. If the token matches an entry in
/// `pending_cond`, removes it (cleanup happens on both success and failure)
/// and — only for `^error` — returns the `BreakpointConditionError` event to
/// emit for the affected row. Success (`^done`) needs no event here: the
/// separate `=breakpoint-modified` notify-async record (parsed elsewhere via
/// `parse_line`) already carries the state update.
fn correlate_pending_cond(line: &str, pending_cond: &mut HashMap<u32, u32>) -> Option<StateEvent> {
    let token = parse_token(line)?;
    let id = *pending_cond.get(&token)?;
    let rest = line.trim_start_matches(|c: char| c.is_ascii_digit());

    if rest.starts_with("^error") {
        pending_cond.remove(&token);
        let msg = extract_str(rest, "msg").unwrap_or_else(|| "GDB error".into());
        Some(StateEvent::BreakpointConditionError { id, message: msg })
    } else if rest.starts_with("^done") {
        pending_cond.remove(&token);
        None
    } else {
        None
    }
}

/// Inspects an incoming raw MI line for a token that correlates to a pending
/// struct-panel `Command::Evaluate`. If the token matches an entry in
/// `pending_struct`, removes it (cleanup happens on both success and failure).
/// `^done,value=...` returns `StructValueUpdated{expr,value}` for the caller
/// to emit (and skip further line processing for this line, since the bare
/// value carries no other information). `^error` returns `None` — the token
/// is still removed but no event is emitted here, so the line falls through
/// to `parse_line`, which turns the generic `^error` into a console
/// `UiEvent::GdbError`.
fn correlate_pending_struct(
    line: &str,
    pending_struct: &mut HashMap<u32, String>,
) -> Option<StateEvent> {
    let token = parse_token(line)?;
    let expr = pending_struct.get(&token)?.clone();
    let rest = line.trim_start_matches(|c: char| c.is_ascii_digit());

    if rest.starts_with("^done") {
        pending_struct.remove(&token);
        let value = extract_str(rest, "value")?;
        Some(StateEvent::StructValueUpdated { expr, value })
    } else if rest.starts_with("^error") {
        pending_struct.remove(&token);
        None
    } else {
        None
    }
}

// ─── Spawn ────────────────────────────────────────────────────────────────────

fn spawn_gdb(
    executable: Option<&str>,
) -> std::io::Result<(Child, GdbWriter<ChildStdin>, BufReader<ChildStdout>)> {
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

    // PID de GDB, necesario para mandarle SIGINT en un Interrupt (ver dispatch).
    let gdb_pid = child.id();

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

    // Token (asignado por GdbWriter::send) -> id del breakpoint cuyo
    // `-break-condition` está pendiente de respuesta. GDB ecoa el token en su
    // result record (`{token}^done`/`{token}^error`), lo que permite
    // correlacionar un `^error` con la fila exacta que lo originó.
    let mut pending_cond: HashMap<u32, u32> = HashMap::new();

    // Token (asignado por GdbWriter::send) -> expresión del panel de struct
    // pendiente de respuesta. Correlación por token, no por FIFO: distinta de
    // `pending_globals` para que una respuesta de struct nunca sea consumida
    // por el camino de globals (y viceversa) aunque ambas estén en vuelo a la
    // vez tras el mismo pause.
    let mut pending_struct: HashMap<u32, String> = HashMap::new();

    loop {
        while let Ok(cmd) = cmd_rx.try_recv() {
            let mi = match dispatch(&cmd) {
                GdbAction::Interrupt => {
                    // El inferior está corriendo: en modo síncrono GDB no lee su
                    // stdin, así que `-exec-interrupt` por el pipe no haría nada.
                    // Le mandamos SIGINT al proceso de GDB, que frena el inferior
                    // y emite `*stopped,reason="signal-received"` (lo parsea
                    // parse_line más abajo).
                    let _ = event_tx.send(DebuggerEvent::Ui(UiEvent::ConsoleOutput(
                        "> [SIGINT] interrupt".into(),
                    )));
                    send_interrupt(gdb_pid);
                    continue;
                }
                GdbAction::Mi(mi) => mi,
            };

            if let DebuggerCommand::EvaluateGlobal(name) = &cmd {
                pending_globals.push_back(name.clone());
            }

            let _ = event_tx.send(DebuggerEvent::Ui(UiEvent::ConsoleOutput(format!("> {mi}"))));

            let token = match writer.send(&mi) {
                Ok(token) => token,
                Err(e) => {
                    let _ = event_tx.send(DebuggerEvent::Ui(UiEvent::GdbError(format!(
                        "Error escribiendo a GDB: {e}"
                    ))));
                    let _ = child.kill();
                    return;
                }
            };

            if let DebuggerCommand::SetBreakpointCondition { id, .. } = &cmd {
                pending_cond.insert(token, *id);
            }

            if let DebuggerCommand::Evaluate(expr) = &cmd {
                pending_struct.insert(token, expr.clone());
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
            // Los records crudos de protocolo MI (^done, *stopped, =notify-async, …)
            // no se echoan a la consola: parse_line ya los traduce en eventos de
            // estado, y los errores reales llegan aparte como GdbError. Solo los
            // stream records (~ @) producen texto legible para el usuario.
            // Correlación del panel de struct: se comprueba PRIMERO, antes que
            // pending_globals, para que una respuesta de struct nunca sea
            // consumida por el FIFO de globals.
            if let Some(event) = correlate_pending_struct(&line, &mut pending_struct) {
                if event_tx.send(DebuggerEvent::State(event)).is_err() {
                    let _ = child.kill();
                    return;
                }
                continue;
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

            // Correlación de -break-condition: un `^error` cuyo token está en
            // pending_cond se traduce en BreakpointConditionError para la fila
            // exacta. El GdbError de consola de parse_line abajo se sigue
            // emitiendo igual (no se reemplaza), así el log no pierde nada.
            if let Some(event) = correlate_pending_cond(&line, &mut pending_cond) {
                if event_tx.send(DebuggerEvent::State(event)).is_err() {
                    let _ = child.kill();
                    return;
                }
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

/// Frena el inferior mandándole SIGINT al proceso de GDB.
///
/// GDB atrapa la señal e interrumpe el programa en ejecución (equivalente al
/// `Ctrl+C` de una sesión interactiva), emitiendo `*stopped`. Se le manda solo
/// al PID de GDB —no al grupo de procesos— para que sea GDB quien decida cómo
/// frenar el inferior, en vez de matarlo directamente.
#[cfg(unix)]
fn send_interrupt(pid: u32) {
    // SAFETY: `kill` con un pid válido y SIGINT no tiene precondiciones de
    // memoria. Ignoramos el resultado: un interrupt fallido (p.ej. el proceso ya
    // terminó) no es fatal.
    unsafe {
        libc::kill(pid as libc::pid_t, libc::SIGINT);
    }
}

#[cfg(not(unix))]
fn send_interrupt(_pid: u32) {
    // El interrupt por señal solo está soportado en Unix por ahora.
}

/// true si la línea es exactamente `^done,value="..."`, la respuesta de
/// -data-evaluate-expression sin ningún otro campo.
fn is_bare_value_done(line: &str) -> bool {
    line.trim_start_matches(|c: char| c.is_ascii_digit())
        .starts_with("^done,value=\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn send_returns_the_token_it_used() {
        let mut writer = GdbWriter {
            stdin: Vec::<u8>::new(),
            seq: 5,
        };
        let token = writer.send("-break-condition 3 \"x > 5\"").unwrap();
        assert_eq!(token, 5);
        assert_eq!(writer.seq, 6);

        let token2 = writer.send("-exec-continue").unwrap();
        assert_eq!(token2, 6);

        assert_eq!(
            String::from_utf8(writer.stdin).unwrap(),
            "5-break-condition 3 \"x > 5\"\n6-exec-continue\n"
        );
    }

    #[test]
    fn pending_cond_insert_and_removal_on_matching_reply() {
        let mut pending_cond: HashMap<u32, u32> = HashMap::new();
        pending_cond.insert(7, 3);

        // A `^done` (success) for the matching token must remove the entry
        // and emit no new event — the =breakpoint-modified notify already
        // carries the state update through the normal parse_line path.
        let result = correlate_pending_cond("7^done", &mut pending_cond);
        assert!(result.is_none());
        assert!(
            !pending_cond.contains_key(&7),
            "token must be removed after a matching ^done"
        );
    }

    #[test]
    fn correlate_pending_cond_emits_error_for_correct_row() {
        let mut pending_cond: HashMap<u32, u32> = HashMap::new();
        pending_cond.insert(9, 42);

        let event = correlate_pending_cond(
            "9^error,msg=\"No symbol \\\"unknown_symbol_xyz\\\" in current context.\"",
            &mut pending_cond,
        );

        match event {
            Some(StateEvent::BreakpointConditionError { id, message }) => {
                assert_eq!(id, 42);
                assert_eq!(
                    message,
                    "No symbol \"unknown_symbol_xyz\" in current context."
                );
            }
            other => panic!("expected BreakpointConditionError, got {other:?}"),
        }
        assert!(
            !pending_cond.contains_key(&9),
            "token must be removed after a matching ^error"
        );
    }

    #[test]
    fn correlate_pending_struct_emits_event_and_removes_token_on_done() {
        let mut pending_struct: HashMap<u32, String> = HashMap::new();
        pending_struct.insert(3, "my_struct.field".into());

        let event = correlate_pending_struct(
            "3^done,value=\"{a = 1, b = 2}\"",
            &mut pending_struct,
        );

        match event {
            Some(StateEvent::StructValueUpdated { expr, value }) => {
                assert_eq!(expr, "my_struct.field");
                assert_eq!(value, "{a = 1, b = 2}");
            }
            other => panic!("expected StructValueUpdated, got {other:?}"),
        }
        assert!(
            !pending_struct.contains_key(&3),
            "token must be removed after a matching ^done"
        );
    }

    #[test]
    fn correlate_pending_struct_removes_token_and_emits_no_event_on_error() {
        let mut pending_struct: HashMap<u32, String> = HashMap::new();
        pending_struct.insert(4, "bad_expr".into());

        let event = correlate_pending_struct(
            "4^error,msg=\"No symbol \\\"bad_expr\\\" in current context.\"",
            &mut pending_struct,
        );

        assert!(event.is_none());
        assert!(
            !pending_struct.contains_key(&4),
            "token must be removed after a matching ^error"
        );
    }

    #[test]
    fn correlate_pending_struct_ignores_unrelated_tokens() {
        let mut pending_struct: HashMap<u32, String> = HashMap::new();
        pending_struct.insert(1, "my_struct".into());

        let event = correlate_pending_struct("2^done,value=\"5\"", &mut pending_struct);
        assert!(event.is_none());
        assert!(pending_struct.contains_key(&1));
    }

    #[test]
    fn correlate_pending_cond_ignores_unrelated_tokens() {
        let mut pending_cond: HashMap<u32, u32> = HashMap::new();
        pending_cond.insert(1, 10);

        // Different token (2), not in the map -> no correlation, map untouched.
        let event = correlate_pending_cond("2^done", &mut pending_cond);
        assert!(event.is_none());
        assert!(pending_cond.contains_key(&1));
    }
}
