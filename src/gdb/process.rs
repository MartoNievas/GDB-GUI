use std::{
    collections::{HashMap, HashSet},
    io::{BufRead, BufReader, Write},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    sync::mpsc::{Receiver, Sender},
    thread,
};

use super::parser::{extract_str, parse_breakpoint_field, parse_line, parse_token};
use super::writer::{GdbAction, dispatch};
use crate::state::{DebuggerEvent, EditTarget, StateEvent, UiEvent};
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

/// Inspects an incoming raw MI line for a token that correlates to a pending
/// `Command::AddWatchpoint`. If the token matches an entry in
/// `pending_watch`, removes it (cleanup happens on both success and
/// failure). `^error` returns `WatchpointError{expr,message}` for the caller
/// to emit — success needs no event here: GDB's `-break-watch` reply is
/// self-describing (`wpt=`/`hw-rwpt=`/`hw-awpt=`), parsed by the normal
/// `parse_line` path into `WatchpointAdded` (design decision: "Creation
/// replies are self-describing... so success needs no token map").
fn correlate_pending_watch(
    line: &str,
    pending_watch: &mut HashMap<u32, String>,
) -> Option<StateEvent> {
    let token = parse_token(line)?;
    let expr = pending_watch.get(&token)?.clone();
    let rest = line.trim_start_matches(|c: char| c.is_ascii_digit());

    if rest.starts_with("^error") {
        pending_watch.remove(&token);
        let msg = extract_str(rest, "msg").unwrap_or_else(|| "GDB error".into());
        Some(StateEvent::WatchpointError { expr, message: msg })
    } else if rest.starts_with("^done") {
        pending_watch.remove(&token);
        None
    } else {
        None
    }
}

/// Inspects an incoming raw MI line for a token that correlates to a pending
/// `Command::SetValue` write. If the token matches an entry in
/// `pending_edit`, removes it (cleanup happens on both success and failure,
/// mirroring `correlate_pending_global`) and returns the value-edit event
/// for the caller to emit: `^done` -> `ValueEditSucceeded`, `^error` ->
/// `ValueEditFailed` with GDB's message. Unlike the struct/global paths,
/// `^error` here DOES emit an event — the row must show the failure inline.
/// `-gdb-set` has no `=notify-async` counterpart, so `^done` is the only
/// signal that triggers the caller's re-fetch.
fn correlate_pending_edit(
    line: &str,
    pending_edit: &mut HashMap<u32, EditTarget>,
) -> Option<StateEvent> {
    let token = parse_token(line)?;
    let target = pending_edit.get(&token)?.clone();
    let rest = line.trim_start_matches(|c: char| c.is_ascii_digit());

    if rest.starts_with("^done") {
        pending_edit.remove(&token);
        Some(StateEvent::ValueEditSucceeded { target })
    } else if rest.starts_with("^error") {
        pending_edit.remove(&token);
        let msg = extract_str(rest, "msg").unwrap_or_else(|| "GDB error".into());
        Some(StateEvent::ValueEditFailed {
            target,
            message: msg,
        })
    } else {
        None
    }
}

/// Outcome of a resolved `Command::ProbeMainSource` reply, correlated by MI
/// token in `correlate_pending_probe`. Never crosses into a `StateEvent`
/// that reaches the UI as its own row — `run_loop` translates `Resolved`
/// into a direct `-break-delete` write plus `StateEvent::SourcePreviewResolved`,
/// and `Failed` into a silent no-op.
#[derive(Debug, PartialEq)]
pub(crate) enum ProbeOutcome {
    Resolved { number: u32, file: String },
    Failed,
}

/// Inspects an incoming raw MI line for a token that correlates to the
/// pending `Command::ProbeMainSource` probe. If the token matches an entry
/// in `pending_probe`, removes it (cleanup happens on both success and
/// failure, mirroring the other `pending_*` correlators) and returns the
/// outcome for the caller to act on. `^done,bkpt={...}` resolves via
/// `parse_breakpoint_field` (the same tested code path `parse_result` uses
/// for a real `AddBreakpoint` reply) into `Resolved{number,file}`.
/// `^error` (no `main` symbol) yields `Failed`. Checked and `continue`d on
/// in `run_loop` before `parse_line`, so a probe reply never reaches
/// `parse_result` and never becomes `BreakpointAdded`.
fn correlate_pending_probe(line: &str, pending_probe: &mut HashSet<u32>) -> Option<ProbeOutcome> {
    let token = parse_token(line)?;
    if !pending_probe.contains(&token) {
        return None;
    }
    let rest = line.trim_start_matches(|c: char| c.is_ascii_digit());

    if rest.starts_with("^done") {
        pending_probe.remove(&token);
        let fields = rest.strip_prefix("^done,").unwrap_or("");
        let bp = parse_breakpoint_field(fields, "bkpt")?;
        Some(ProbeOutcome::Resolved {
            number: bp.id,
            file: bp.file,
        })
    } else if rest.starts_with("^error") {
        pending_probe.remove(&token);
        Some(ProbeOutcome::Failed)
    } else {
        None
    }
}

/// Inspects an incoming raw MI line for a token that correlates to a pending
/// `Command::AddCatchpoint`. If the token matches an entry in
/// `pending_catch`, removes it (cleanup on both success and failure).
/// `^error` returns `CatchpointError{key,message}` for the caller to emit,
/// keyed the same way as `catchpoint_errors` (D1: `"{kind}:{args joined}"`).
/// `^done` is cleanup-only and emits no event: catchpoint creation has no
/// send-time optimistic add (design addendum A2/A3) — the GDB id is unknown
/// until the asynchronous, untokened `=breakpoint-created` notify arrives
/// (parsed separately by `parse_notify_async` into `CatchpointAdded`), so a
/// tokened `^done` here (Load/Unload's native reply) carries nothing new.
fn correlate_pending_catch(
    line: &str,
    pending_catch: &mut HashMap<u32, String>,
) -> Option<StateEvent> {
    let token = parse_token(line)?;
    let key = pending_catch.get(&token)?.clone();
    let rest = line.trim_start_matches(|c: char| c.is_ascii_digit());

    if rest.starts_with("^error") {
        pending_catch.remove(&token);
        let msg = extract_str(rest, "msg").unwrap_or_else(|| "GDB error".into());
        Some(StateEvent::CatchpointError { key, message: msg })
    } else if rest.starts_with("^done") {
        pending_catch.remove(&token);
        None
    } else {
        None
    }
}

/// Pure decision for the two catchpoint commands (`RemoveCatchpoint`,
/// `ToggleCatchpoint`) that have no correlatable reply — mirrors
/// `optimistic_watchpoint_event` exactly (D2/D4 from design #65, unaffected
/// by the transport addendum). `Command::AddCatchpoint` is deliberately
/// absent here (addendum A2): its creation event is never optimistic.
fn optimistic_catchpoint_event(cmd: &DebuggerCommand) -> Option<StateEvent> {
    match cmd {
        DebuggerCommand::RemoveCatchpoint(id) => Some(StateEvent::CatchpointRemoved { id: *id }),
        DebuggerCommand::ToggleCatchpoint { id, enable } => Some(StateEvent::CatchpointToggled {
            id: *id,
            enabled: *enable,
        }),
        _ => None,
    }
}

/// Pure decision for the two watchpoint commands (`RemoveWatchpoint`,
/// `ToggleWatchpoint`) that have no correlatable reply: `-break-delete`
/// gives a bare `^done` with no id (mirroring `RemoveBreakpoint`), and
/// `-break-enable`/`-break-disable` also reply with a bare `^done` while the
/// `=breakpoint-modified` notify a watchpoint would otherwise get is dropped
/// by `parse_breakpoint_field`'s file/fullname bail (design decision D3).
/// Both are therefore emitted optimistically at send time (D2, D4) instead
/// of being correlated from a reply. `None` for every other command.
fn optimistic_watchpoint_event(cmd: &DebuggerCommand) -> Option<StateEvent> {
    match cmd {
        DebuggerCommand::RemoveWatchpoint(id) => Some(StateEvent::WatchpointRemoved { id: *id }),
        DebuggerCommand::ToggleWatchpoint { id, enable } => Some(StateEvent::WatchpointToggled {
            id: *id,
            enabled: *enable,
        }),
        _ => None,
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

    // Token (assigned by GdbWriter::send) -> EditTarget of the
    // `Command::SetValue` write pending a response. Correlated by token, not
    // FIFO, like the other pending maps: kept separate so a value-edit reply
    // is never consumed by the struct/globals/cond paths, even when several
    // are in flight after the same pause.
    let mut pending_edit: HashMap<u32, EditTarget> = HashMap::new();

    // Token (assigned by GdbWriter::send) of the in-flight
    // `Command::ProbeMainSource` probe, if any. Correlated by token like the
    // other pending sets: its reply is intercepted and `continue`d on before
    // `parse_line`, so it never reaches `parse_result` and never becomes a
    // `BreakpointAdded` row (see `correlate_pending_probe`).
    let mut pending_probe: HashSet<u32> = HashSet::new();

    // Token (assigned by GdbWriter::send) -> expression of the
    // `Command::AddWatchpoint` pending a response. Correlated by token, not
    // FIFO, like the other pending maps: only `^error` needs correlation
    // here (success is self-describing and flows through the normal
    // `parse_line` path into `WatchpointAdded`).
    let mut pending_watch: HashMap<u32, String> = HashMap::new();

    // Token (assigned by GdbWriter::send) -> D1 key (`"{kind}:{args}"`) of
    // the `Command::AddCatchpoint` pending a response. Correlated by token,
    // not FIFO, like the other pending maps: only `^error` needs correlation
    // here (a successful creation is never send-time optimistic — see
    // `correlate_pending_catch` doc comment / design addendum A2/A3).
    let mut pending_catch: HashMap<u32, String> = HashMap::new();

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

            if let DebuggerCommand::SetValue { target, .. } = &cmd {
                pending_edit.insert(token, target.clone());
            }

            if matches!(cmd, DebuggerCommand::ProbeMainSource) {
                pending_probe.insert(token);
            }

            if let DebuggerCommand::AddWatchpoint { expr, .. } = &cmd {
                pending_watch.insert(token, expr.clone());
            }

            // D1 key: "{kind}:{args joined by ','}" — matches
            // `catchpoint_errors`' key shape so a later `^error` attributes
            // straight to the row the panel is tracking by (kind, args).
            if let DebuggerCommand::AddCatchpoint { kind, args } = &cmd {
                pending_catch.insert(token, format!("{kind}:{}", args.join(",")));
            }

            // GDB responds to `-break-delete` with a plain `^done` without
            // `=breakpoint-deleted` or the deleted id, so the response cannot be
            // correlated. We emit the removal event ourselves so the UI reflects it.
            if let DebuggerCommand::RemoveBreakpoint(id) = &cmd {
                let _ = event_tx.send(DebuggerEvent::State(StateEvent::BreakpointRemoved {
                    id: *id,
                }));
            }

            // Watchpoint remove/toggle (D2, D4): neither `-break-delete` nor
            // `-break-enable`/`-break-disable` gives a reply this can
            // correlate a row update from (see `optimistic_watchpoint_event`
            // doc comment), so both are emitted optimistically at send time.
            if let Some(event) = optimistic_watchpoint_event(&cmd) {
                let _ = event_tx.send(DebuggerEvent::State(event));
            }

            // Catchpoint remove/toggle (D2, D4, unaffected by the transport
            // addendum): same fire-and-forget reasoning as watchpoints.
            if let Some(event) = optimistic_catchpoint_event(&cmd) {
                let _ = event_tx.send(DebuggerEvent::State(event));
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

            // Preload-source probe correlation: intercepted and `continue`d
            // on for BOTH outcomes, before `parse_line`, so the probe's
            // `^done,bkpt={...}` never reaches `parse_result` and never
            // becomes a `BreakpointAdded` row (design decision #2). On
            // `Resolved`, the probe's own `-break-delete <number>` is
            // written directly through `writer` here — not via
            // `Command::RemoveBreakpoint` — so no `BreakpointRemoved` event
            // is emitted for a row the UI never had (design decision #3).
            // On `Failed` (no `main` symbol), nothing is emitted: a silent
            // no-op matching today's empty-source-view behavior.
            if let Some(outcome) = correlate_pending_probe(&line, &mut pending_probe) {
                if let ProbeOutcome::Resolved { number, file } = outcome {
                    if let Err(e) = writer.send(&format!("-break-delete {number}")) {
                        let _ = event_tx.send(DebuggerEvent::Ui(UiEvent::GdbError(format!(
                            "Error escribiendo a GDB: {e}"
                        ))));
                        let _ = child.kill();
                        return;
                    }
                    if event_tx
                        .send(DebuggerEvent::State(StateEvent::SourcePreviewResolved {
                            file,
                        }))
                        .is_err()
                    {
                        let _ = child.kill();
                        return;
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

            // Value-edit correlation: like pending_cond, does not `continue` —
            // an `^error` still falls through to parse_line below so the
            // console log also shows it (not replaced, just supplemented).
            if let Some(event) = correlate_pending_edit(&line, &mut pending_edit) {
                if event_tx.send(DebuggerEvent::State(event)).is_err() {
                    let _ = child.kill();
                    return;
                }
            }

            // Watchpoint-creation correlation: like pending_cond/pending_edit,
            // does not `continue` on `^error` — the console log still shows
            // it via parse_line below. On `^done`, the token is cleaned up
            // and the line falls through to parse_line, which turns the
            // self-describing `wpt=`/`hw-rwpt=`/`hw-awpt=` reply into
            // `WatchpointAdded`.
            if let Some(event) = correlate_pending_watch(&line, &mut pending_watch) {
                if event_tx.send(DebuggerEvent::State(event)).is_err() {
                    let _ = child.kill();
                    return;
                }
            }

            // Catchpoint-creation correlation: like pending_watch, does not
            // `continue` on `^error` — the console log still shows it via
            // parse_line below. On `^done` (Load/Unload's native, tokened
            // reply), the token is cleaned up and the line falls through to
            // parse_line, which turns its self-describing `bkpt={catch-type=
            // ...}` payload into `CatchpointAdded` (same fn the untokened
            // notify-async path uses — A6).
            if let Some(event) = correlate_pending_catch(&line, &mut pending_catch) {
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

        let event =
            correlate_pending_struct("3^done,value=\"{a = 1, b = 2}\"", &mut pending_struct);

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
        let event_second = correlate_pending_global("21^done,value=\"200\"", &mut pending_globals);
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
    fn correlate_pending_edit_emits_succeeded_and_removes_token_on_done() {
        let mut pending_edit: HashMap<u32, EditTarget> = HashMap::new();
        pending_edit.insert(15, EditTarget::Local("x".into()));

        let event = correlate_pending_edit("15^done", &mut pending_edit);

        match event {
            Some(StateEvent::ValueEditSucceeded { target }) => {
                assert_eq!(target, EditTarget::Local("x".into()));
            }
            other => panic!("expected ValueEditSucceeded, got {other:?}"),
        }
        assert!(
            !pending_edit.contains_key(&15),
            "token must be removed after a matching ^done"
        );
    }

    #[test]
    fn correlate_pending_edit_emits_failed_with_message_and_removes_token_on_error() {
        let mut pending_edit: HashMap<u32, EditTarget> = HashMap::new();
        pending_edit.insert(16, EditTarget::Register("pc".into()));

        let event = correlate_pending_edit(
            "16^error,msg=\"Invalid number \\\"abc\\\".\"",
            &mut pending_edit,
        );

        match event {
            Some(StateEvent::ValueEditFailed { target, message }) => {
                assert_eq!(target, EditTarget::Register("pc".into()));
                assert_eq!(message, "Invalid number \"abc\".");
            }
            other => panic!("expected ValueEditFailed, got {other:?}"),
        }
        assert!(
            !pending_edit.contains_key(&16),
            "token must be removed after a matching ^error"
        );
    }

    #[test]
    fn correlate_pending_edit_ignores_unrelated_tokens() {
        let mut pending_edit: HashMap<u32, EditTarget> = HashMap::new();
        pending_edit.insert(1, EditTarget::Local("kept".into()));

        let event = correlate_pending_edit("2^done", &mut pending_edit);
        assert!(event.is_none());
        assert!(pending_edit.contains_key(&1));
    }

    #[test]
    fn correlate_pending_edit_and_global_in_flight_resolve_independently() {
        // Mirrors struct_and_global_evaluations_in_flight_simultaneously_resolve_independently:
        // an edit write and a globals refresh in flight at the same time must
        // each resolve only their own token, regardless of arrival order.
        let mut pending_edit: HashMap<u32, EditTarget> = HashMap::new();
        pending_edit.insert(40, EditTarget::Global("g_counter".into()));
        let mut pending_globals: HashMap<u32, String> = HashMap::new();
        pending_globals.insert(41, "g_other".into());

        // The globals reply arrives first; the edit path must not touch it
        // (different token, never in pending_edit).
        let edit_attempt = correlate_pending_edit("41^done", &mut pending_edit);
        assert!(edit_attempt.is_none());
        assert!(pending_edit.contains_key(&40));
        assert!(pending_globals.contains_key(&41));

        let global_event = correlate_pending_global("41^done,value=\"3\"", &mut pending_globals);
        match global_event {
            Some(StateEvent::GlobalValueUpdated { name, value }) => {
                assert_eq!(name, "g_other");
                assert_eq!(value, "3");
            }
            other => panic!("expected GlobalValueUpdated, got {other:?}"),
        }
        assert!(!pending_globals.contains_key(&41));

        // The edit reply resolves its own token afterward, unaffected.
        let edit_event = correlate_pending_edit("40^done", &mut pending_edit);
        match edit_event {
            Some(StateEvent::ValueEditSucceeded { target }) => {
                assert_eq!(target, EditTarget::Global("g_counter".into()));
            }
            other => panic!("expected ValueEditSucceeded, got {other:?}"),
        }
        assert!(!pending_edit.contains_key(&40));
    }

    // ── Preload-source probe correlation ────────────────────────────────────
    //
    // The probe's `^done,bkpt={...}` must be intercepted and resolved by
    // token before the line ever reaches `parse_line`/`parse_result` — it
    // must never become a `StateEvent::BreakpointAdded` (design decision #2).

    #[test]
    fn correlate_pending_probe_resolves_done_and_removes_token() {
        let mut pending_probe: std::collections::HashSet<u32> = std::collections::HashSet::new();
        pending_probe.insert(3);

        let outcome = correlate_pending_probe(
            "3^done,bkpt={number=\"1\",fullname=\"/tmp/main.c\",line=\"5\"}",
            &mut pending_probe,
        );

        match outcome {
            Some(ProbeOutcome::Resolved { number, file }) => {
                assert_eq!(number, 1);
                assert_eq!(file, "/tmp/main.c");
            }
            other => panic!("expected ProbeOutcome::Resolved, got {other:?}"),
        }
        assert!(
            !pending_probe.contains(&3),
            "token must be removed after a matching ^done"
        );
    }

    #[test]
    fn correlate_pending_probe_resolves_error_and_removes_token_no_event() {
        let mut pending_probe: std::collections::HashSet<u32> = std::collections::HashSet::new();
        pending_probe.insert(4);

        let outcome = correlate_pending_probe(
            "4^error,msg=\"Function \\\"main\\\" not defined.\"",
            &mut pending_probe,
        );

        match outcome {
            Some(ProbeOutcome::Failed) => {}
            other => panic!("expected ProbeOutcome::Failed, got {other:?}"),
        }
        assert!(
            !pending_probe.contains(&4),
            "token must be removed after a matching ^error"
        );
    }

    #[test]
    fn correlate_pending_probe_ignores_unrelated_tokens() {
        let mut pending_probe: std::collections::HashSet<u32> = std::collections::HashSet::new();
        pending_probe.insert(1);

        // A real AddBreakpoint reply on a different, untracked token must
        // not be consumed by the probe path.
        let outcome = correlate_pending_probe(
            "2^done,bkpt={number=\"5\",fullname=\"/tmp/other.c\",line=\"9\"}",
            &mut pending_probe,
        );

        assert!(outcome.is_none());
        assert!(
            pending_probe.contains(&1),
            "unrelated entry must survive untouched"
        );
    }

    // ── Watchpoint remove/toggle fire-and-forget (D2, D4) ────────────────────

    #[test]
    fn optimistic_watchpoint_event_remove_emits_watchpoint_removed() {
        let event = optimistic_watchpoint_event(&DebuggerCommand::RemoveWatchpoint(3));
        match event {
            Some(StateEvent::WatchpointRemoved { id }) => assert_eq!(id, 3),
            other => panic!("expected WatchpointRemoved, got {other:?}"),
        }
    }

    #[test]
    fn optimistic_watchpoint_event_toggle_emits_watchpoint_toggled() {
        let enable_event = optimistic_watchpoint_event(&DebuggerCommand::ToggleWatchpoint {
            id: 5,
            enable: true,
        });
        match enable_event {
            Some(StateEvent::WatchpointToggled { id, enabled }) => {
                assert_eq!(id, 5);
                assert!(enabled);
            }
            other => panic!("expected WatchpointToggled, got {other:?}"),
        }

        let disable_event = optimistic_watchpoint_event(&DebuggerCommand::ToggleWatchpoint {
            id: 5,
            enable: false,
        });
        match disable_event {
            Some(StateEvent::WatchpointToggled { id, enabled }) => {
                assert_eq!(id, 5);
                assert!(!enabled);
            }
            other => panic!("expected WatchpointToggled, got {other:?}"),
        }
    }

    #[test]
    fn optimistic_watchpoint_event_ignores_unrelated_commands() {
        assert!(optimistic_watchpoint_event(&DebuggerCommand::Continue).is_none());
        assert!(optimistic_watchpoint_event(&DebuggerCommand::RemoveBreakpoint(1)).is_none());
    }

    // ── Watchpoint creation correlation ──────────────────────────────────────

    #[test]
    fn correlate_pending_watch_emits_error_for_correct_expr() {
        let mut pending_watch: HashMap<u32, String> = HashMap::new();
        pending_watch.insert(9, "nosuchvar".into());

        let event = correlate_pending_watch(
            "9^error,msg=\"No symbol \\\"nosuchvar\\\" in current context.\"",
            &mut pending_watch,
        );

        match event {
            Some(StateEvent::WatchpointError { expr, message }) => {
                assert_eq!(expr, "nosuchvar");
                assert_eq!(message, "No symbol \"nosuchvar\" in current context.");
            }
            other => panic!("expected WatchpointError, got {other:?}"),
        }
        assert!(
            !pending_watch.contains_key(&9),
            "token must be removed after a matching ^error"
        );
    }

    #[test]
    fn correlate_pending_watch_done_is_cleanup_only_no_event() {
        let mut pending_watch: HashMap<u32, String> = HashMap::new();
        pending_watch.insert(7, "x".into());

        // Success carries no event here: the self-describing wpt= reply is
        // parsed separately by parse_line into WatchpointAdded.
        let result =
            correlate_pending_watch("7^done,wpt={number=\"2\",exp=\"x\"}", &mut pending_watch);
        assert!(result.is_none());
        assert!(
            !pending_watch.contains_key(&7),
            "token must be removed after a matching ^done"
        );
    }

    #[test]
    fn correlate_pending_watch_ignores_unrelated_tokens() {
        let mut pending_watch: HashMap<u32, String> = HashMap::new();
        pending_watch.insert(1, "kept".into());

        let event = correlate_pending_watch("2^done", &mut pending_watch);
        assert!(event.is_none());
        assert!(pending_watch.contains_key(&1));
    }

    // ── Catchpoint creation correlation (^error only, A2/A3) ─────────────────

    #[test]
    fn correlate_pending_catch_emits_error_for_correct_key() {
        let mut pending_catch: HashMap<u32, String> = HashMap::new();
        pending_catch.insert(9, "signal:BOGUS".into());

        let event = correlate_pending_catch(
            "9^error,msg=\"Undefined signal name BOGUS.\"",
            &mut pending_catch,
        );

        match event {
            Some(StateEvent::CatchpointError { key, message }) => {
                assert_eq!(key, "signal:BOGUS");
                assert_eq!(message, "Undefined signal name BOGUS.");
            }
            other => panic!("expected CatchpointError, got {other:?}"),
        }
        assert!(
            !pending_catch.contains_key(&9),
            "token must be removed after a matching ^error"
        );
    }

    // Phase 2b task 4.3 (verify-only): the key is built from `Display for
    // CatchpointKind` (`"syscall"`), so Syscall's rejected creation routes
    // to the same Pending Errors path with no new code.
    #[test]
    fn correlate_pending_catch_emits_error_for_syscall_key() {
        let mut pending_catch: HashMap<u32, String> = HashMap::new();
        pending_catch.insert(11, "syscall:bogus_name".into());

        let event = correlate_pending_catch(
            "11^error,msg=\"Unknown syscall name bogus_name.\"",
            &mut pending_catch,
        );

        match event {
            Some(StateEvent::CatchpointError { key, message }) => {
                assert_eq!(key, "syscall:bogus_name");
                assert_eq!(message, "Unknown syscall name bogus_name.");
            }
            other => panic!("expected CatchpointError, got {other:?}"),
        }
        assert!(!pending_catch.contains_key(&11));
    }

    #[test]
    fn correlate_pending_catch_done_is_cleanup_only_no_event() {
        let mut pending_catch: HashMap<u32, String> = HashMap::new();
        pending_catch.insert(7, "load:libc".into());

        // Success carries no event here: a tokened ^done (Load/Unload's
        // native reply) is parsed separately by parse_line into
        // CatchpointAdded, and the four other kinds never reach this token
        // at all (their creation is untokened async).
        let result = correlate_pending_catch(
            "7^done,bkpt={number=\"3\",type=\"catchpoint\",catch-type=\"load\"}",
            &mut pending_catch,
        );
        assert!(result.is_none());
        assert!(
            !pending_catch.contains_key(&7),
            "token must be removed after a matching ^done"
        );
    }

    #[test]
    fn correlate_pending_catch_ignores_unrelated_tokens() {
        let mut pending_catch: HashMap<u32, String> = HashMap::new();
        pending_catch.insert(1, "fork:".into());

        let event = correlate_pending_catch("2^done", &mut pending_catch);
        assert!(event.is_none());
        assert!(pending_catch.contains_key(&1));
    }

    // ── Catchpoint remove/toggle fire-and-forget (D2, D4) ────────────────────

    #[test]
    fn optimistic_catchpoint_event_remove_emits_catchpoint_removed() {
        let event = optimistic_catchpoint_event(&DebuggerCommand::RemoveCatchpoint(3));
        match event {
            Some(StateEvent::CatchpointRemoved { id }) => assert_eq!(id, 3),
            other => panic!("expected CatchpointRemoved, got {other:?}"),
        }
    }

    #[test]
    fn optimistic_catchpoint_event_toggle_emits_catchpoint_toggled() {
        let enable_event = optimistic_catchpoint_event(&DebuggerCommand::ToggleCatchpoint {
            id: 5,
            enable: true,
        });
        match enable_event {
            Some(StateEvent::CatchpointToggled { id, enabled }) => {
                assert_eq!(id, 5);
                assert!(enabled);
            }
            other => panic!("expected CatchpointToggled, got {other:?}"),
        }
    }

    // D2 regression guard: optimistic dispatch must key off the `Command`
    // variant, not the id — a catchpoint remove/toggle must never be
    // confused with the (structurally identical MI, different Command)
    // breakpoint/watchpoint variants, and `AddCatchpoint` must never emit an
    // optimistic event (A2 — creation is never send-time optimistic).
    #[test]
    fn optimistic_catchpoint_event_ignores_unrelated_and_add_commands() {
        assert!(optimistic_catchpoint_event(&DebuggerCommand::Continue).is_none());
        assert!(optimistic_catchpoint_event(&DebuggerCommand::RemoveBreakpoint(1)).is_none());
        assert!(optimistic_catchpoint_event(&DebuggerCommand::RemoveWatchpoint(1)).is_none());
        assert!(
            optimistic_catchpoint_event(&DebuggerCommand::AddCatchpoint {
                kind: crate::state::CatchpointKind::Fork,
                args: vec![],
            })
            .is_none(),
            "AddCatchpoint must never be optimistic (A2)"
        );
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
        let struct_attempt = correlate_pending_struct("31^done,value=\"7\"", &mut pending_struct);
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
