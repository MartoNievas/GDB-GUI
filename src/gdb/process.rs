use std::{
    collections::HashMap,
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

/// Inspects an incoming raw MI line for a token that correlates to a pending
/// global-variable `Command::EvaluateGlobal`. If the token matches an entry
/// in `pending_globals`, removes it (cleanup happens on both success and
/// failure). `^done,value=...` returns `GlobalValueUpdated{name,value}` for
/// the caller to emit. `^error` returns `None` — the token is still removed
/// (so a failed evaluation, e.g. a global out of scope after a pause, cannot
/// leak an entry forever) but no event is emitted here.
fn correlate_pending_global(
    line: &str,
    pending_globals: &mut HashMap<u32, String>,
) -> Option<StateEvent> {
    let token = parse_token(line)?;
    let name = pending_globals.get(&token)?.clone();
    let rest = line.trim_start_matches(|c: char| c.is_ascii_digit());

    if rest.starts_with("^done") {
        pending_globals.remove(&token);
        let value = extract_str(rest, "value")?;
        Some(StateEvent::GlobalValueUpdated { name, value })
    } else if rest.starts_with("^error") {
        pending_globals.remove(&token);
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

    // Token (assigned by GdbWriter::send) -> global-variable name pending a
    // response. Correlated by token, not FIFO: kept separate from
    // `pending_struct` (and vice versa) so a globals response is never
    // consumed by the struct path, even when both are in flight at the same
    // time after the same pause.
    let mut pending_globals: HashMap<u32, String> = HashMap::new();

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

            if let DebuggerCommand::EvaluateGlobal(name) = &cmd {
                pending_globals.insert(token, name.clone());
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
            // Struct-panel correlation: checked FIRST, before pending_globals. Both
            // sides are token-keyed maps, so isolation is mutual — neither path can
            // consume the other's reply, regardless of check order.
            if let Some(event) = correlate_pending_struct(&line, &mut pending_struct) {
                if event_tx.send(DebuggerEvent::State(event)).is_err() {
                    let _ = child.kill();
                    return;
                }
                continue;
            }

            if let Some(event) = correlate_pending_global(&line, &mut pending_globals) {
                if event_tx.send(DebuggerEvent::State(event)).is_err() {
                    let _ = child.kill();
                    return;
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

    #[test]
    fn correlate_pending_global_emits_event_and_removes_token_on_done() {
        let mut pending_globals: HashMap<u32, String> = HashMap::new();
        pending_globals.insert(11, "g_counter".into());

        let event = correlate_pending_global("11^done,value=\"42\"", &mut pending_globals);

        match event {
            Some(StateEvent::GlobalValueUpdated { name, value }) => {
                assert_eq!(name, "g_counter");
                assert_eq!(value, "42");
            }
            other => panic!("expected GlobalValueUpdated, got {other:?}"),
        }
        assert!(
            !pending_globals.contains_key(&11),
            "token must be removed after a matching ^done"
        );
    }

    #[test]
    fn correlate_pending_global_removes_token_and_emits_no_event_on_error() {
        let mut pending_globals: HashMap<u32, String> = HashMap::new();
        pending_globals.insert(12, "g_out_of_scope".into());

        let event = correlate_pending_global(
            "12^error,msg=\"No symbol \\\"g_out_of_scope\\\" in current context.\"",
            &mut pending_globals,
        );

        assert!(event.is_none());
        assert!(
            !pending_globals.contains_key(&12),
            "token must be removed after a matching ^error"
        );
    }

    #[test]
    fn correlate_pending_global_ignores_unrelated_tokens() {
        let mut pending_globals: HashMap<u32, String> = HashMap::new();
        pending_globals.insert(1, "g_flag".into());

        // Simulates a raced Command::Raw reply sharing the same
        // `^done,value="..."` shape but a different, untracked token.
        let event = correlate_pending_global("2^done,value=\"5\"", &mut pending_globals);
        assert!(event.is_none());
        assert!(pending_globals.contains_key(&1));
    }

    #[test]
    fn correlate_pending_global_out_of_order_replies_resolve_correct_names() {
        let mut pending_globals: HashMap<u32, String> = HashMap::new();
        pending_globals.insert(20, "g_first".into());
        pending_globals.insert(21, "g_second".into());

        // Replies arrive reversed: token 21 first, then token 20.
        let event_second =
            correlate_pending_global("21^done,value=\"200\"", &mut pending_globals);
        match event_second {
            Some(StateEvent::GlobalValueUpdated { name, value }) => {
                assert_eq!(name, "g_second");
                assert_eq!(value, "200");
            }
            other => panic!("expected GlobalValueUpdated, got {other:?}"),
        }
        assert!(!pending_globals.contains_key(&21));
        assert!(pending_globals.contains_key(&20));

        let event_first = correlate_pending_global("20^done,value=\"100\"", &mut pending_globals);
        match event_first {
            Some(StateEvent::GlobalValueUpdated { name, value }) => {
                assert_eq!(name, "g_first");
                assert_eq!(value, "100");
            }
            other => panic!("expected GlobalValueUpdated, got {other:?}"),
        }
        assert!(!pending_globals.contains_key(&20));
    }

    #[test]
    fn struct_and_global_evaluations_in_flight_simultaneously_resolve_independently() {
        // Mirrors the struct-inspection spec scenario: a globals refresh and a
        // struct expression evaluation are both pending after the same pause,
        // and their replies (checked struct-first, as in the reply loop) each
        // update only their own panel, regardless of arrival order.
        let mut pending_struct: HashMap<u32, String> = HashMap::new();
        pending_struct.insert(30, "my_struct.field".into());
        let mut pending_globals: HashMap<u32, String> = HashMap::new();
        pending_globals.insert(31, "g_counter".into());

        // The globals reply arrives first. The struct path (checked first in
        // the real loop) must not consume it, since its token isn't its own.
        let struct_attempt =
            correlate_pending_struct("31^done,value=\"7\"", &mut pending_struct);
        assert!(struct_attempt.is_none());
        assert!(
            pending_struct.contains_key(&30),
            "struct path must not touch its own pending entry when the reply belongs to globals"
        );
        assert!(
            pending_globals.contains_key(&31),
            "struct path must never remove a globals entry"
        );

        // The globals path resolves its own token correctly.
        let global_event = correlate_pending_global("31^done,value=\"7\"", &mut pending_globals);
        match global_event {
            Some(StateEvent::GlobalValueUpdated { name, value }) => {
                assert_eq!(name, "g_counter");
                assert_eq!(value, "7");
            }
            other => panic!("expected GlobalValueUpdated, got {other:?}"),
        }
        assert!(!pending_globals.contains_key(&31));

        // The struct reply arrives after and still resolves to its own entry,
        // unaffected by the globals reply that was processed in between.
        let struct_event =
            correlate_pending_struct("30^done,value=\"{a = 1}\"", &mut pending_struct);
        match struct_event {
            Some(StateEvent::StructValueUpdated { expr, value }) => {
                assert_eq!(expr, "my_struct.field");
                assert_eq!(value, "{a = 1}");
            }
            other => panic!("expected StructValueUpdated, got {other:?}"),
        }
        assert!(!pending_struct.contains_key(&30));
    }
}
