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

    // GDB's PID, needed to send it SIGINT on an Interrupt (see dispatch).
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

    // FIFO queue of global variable names pending evaluation. GDB responds to
    // synchronous commands (-data-evaluate-expression, etc.) in the same order they
    // are sent, so we can correlate each nameless "^done,value=..." with its
    // corresponding name simply by dequeuing in arrival order.
    let mut pending_globals: VecDeque<String> = VecDeque::new();

    // Token (assigned by GdbWriter::send) -> id of the breakpoint whose
    // `-break-condition` is pending a response. GDB echoes the token in its
    // result record (`{token}^done`/`{token}^error`), which lets us
    // correlate an `^error` with the exact row that originated it.
    let mut pending_cond: HashMap<u32, u32> = HashMap::new();

    // Token (assigned by GdbWriter::send) -> struct-panel expression pending
    // a response. Correlated by token, not FIFO: kept separate from
    // `pending_globals` so a struct response is never consumed by the
    // globals path (and vice versa), even when both are in flight at the
    // same time after the same pause.
    let mut pending_struct: HashMap<u32, String> = HashMap::new();

    loop {
        while let Ok(cmd) = cmd_rx.try_recv() {
            let mi = match dispatch(&cmd) {
                GdbAction::Interrupt => {
                    // The inferior is running: in synchronous mode GDB does not
                    // read its stdin, so `-exec-interrupt` sent through the pipe
                    // would do nothing. We send SIGINT to the GDB process instead,
                    // which stops the inferior and emits
                    // `*stopped,reason="signal-received"` (parsed by parse_line
                    // below).
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

            // GDB responds to `-break-delete` with a plain `^done` without
            // `=breakpoint-deleted` or the deleted id, so the response cannot be
            // correlated. We emit the removal event ourselves so the UI reflects it.
            if let DebuggerCommand::RemoveBreakpoint(id) = &cmd {
                let _ = event_tx.send(DebuggerEvent::State(StateEvent::BreakpointRemoved {
                    id: *id,
                }));
            }
        }

        while let Ok(line) = line_rx.try_recv() {
            // Raw MI protocol records (^done, *stopped, =notify-async, …) are not
            // echoed to the console: parse_line already translates them into state
            // events, and real errors arrive separately as GdbError. Only stream
            // records (~ @) produce readable text for the user.
            // Struct-panel correlation: checked FIRST, before pending_globals, so
            // that a struct response is never consumed by the globals FIFO.
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

            // -break-condition correlation: an `^error` whose token is in
            // pending_cond is translated into a BreakpointConditionError for the
            // exact row. The console GdbError from parse_line below is still
            // emitted regardless (not replaced), so the log loses nothing.
            if let Some(event) = correlate_pending_cond(&line, &mut pending_cond) {
                if event_tx.send(DebuggerEvent::State(event)).is_err() {
                    let _ = child.kill();
                    return;
                }
            }

            if let Some(event) = parse_line(&line) {
                // None = ignorable line, not an error
                if event_tx.send(event).is_err() {
                    let _ = child.kill();
                    return; // UI closed
                }
            }
        }

        thread::sleep(std::time::Duration::from_millis(10));
    }
}

// ─── Helpers ───────────────────────────────────────────────────────────────

/// Stops the inferior by sending SIGINT to the GDB process.
///
/// GDB traps the signal and interrupts the running program (equivalent to
/// `Ctrl+C` in an interactive session), emitting `*stopped`. It is sent only
/// to GDB's PID —not to the process group— so that GDB decides how to stop
/// the inferior, instead of killing it directly.
#[cfg(unix)]
fn send_interrupt(pid: u32) {
    // SAFETY: `kill` with a valid pid and SIGINT has no memory preconditions.
    // We ignore the result: a failed interrupt (e.g. the process already
    // terminated) is not fatal.
    unsafe {
        libc::kill(pid as libc::pid_t, libc::SIGINT);
    }
}

#[cfg(not(unix))]
fn send_interrupt(_pid: u32) {
    // Signal-based interrupt is only supported on Unix for now.
}

/// true if the line is exactly `^done,value="..."`, the response to
/// -data-evaluate-expression with no other field.
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
