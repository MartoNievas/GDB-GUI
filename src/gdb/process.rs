use std::{
    collections::{HashMap, HashSet},
    io::{BufRead, BufReader, Write},
    ops::ControlFlow,
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

/// Value type for `PendingRegistry.insert` — the `file`/`line` a pending
/// `Command::AddBreakpoint` was requested for, so a later `^error` can be
/// correlated back to the exact location the row/error belongs to (mirrors
/// `correlate_pending_watch`'s `String` key, but insert needs two fields
/// since there is no single string identity like an expression).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct BreakpointInsertRequest {
    pub file: String,
    pub line: u32,
}

/// Inspects an incoming raw MI line for a token that correlates to a pending
/// `Command::AddBreakpoint`. If the token matches an entry in
/// `pending_insert`, removes it (cleanup on both success and failure,
/// mirroring `correlate_pending_watch`). `^error` returns
/// `BreakpointInsertFailed{file,line,message}` for the caller to emit —
/// there is no GDB id to attach the error to (the insert never succeeded).
/// `^done` is cleanup-only: the self-describing `^done,bkpt={...}` reply
/// falls through to the normal `parse_line`/`parse_result` path, which turns
/// it into `BreakpointAdded`.
fn correlate_pending_insert(
    line: &str,
    pending_insert: &mut HashMap<u32, BreakpointInsertRequest>,
) -> Option<StateEvent> {
    let token = parse_token(line)?;
    let request = pending_insert.get(&token)?.clone();
    let rest = line.trim_start_matches(|c: char| c.is_ascii_digit());

    if rest.starts_with("^error") {
        pending_insert.remove(&token);
        let msg = extract_str(rest, "msg").unwrap_or_else(|| "GDB error".into());
        Some(StateEvent::BreakpointInsertFailed {
            file: request.file,
            line: request.line,
            message: msg,
        })
    } else if rest.starts_with("^done") {
        pending_insert.remove(&token);
        None
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

/// Inspects an incoming raw MI line for a token that correlates to a pending
/// `Command::RequestMemory`. If the token matches an entry in
/// `pending_memory`, removes it (cleanup happens on both success and
/// failure, mirroring `correlate_pending_watch`). `^error` returns
/// `MemoryRequestFailed{address,message}` for the caller to emit — success
/// needs no event here: GDB's `-data-read-memory-bytes` reply is
/// self-describing (`memory=[...]`), parsed by the normal `parse_line` path
/// into `MemoryUpdated` (D1/D2).
fn correlate_pending_memory(
    line: &str,
    pending_memory: &mut HashMap<u32, String>,
) -> Option<StateEvent> {
    let token = parse_token(line)?;
    let address = pending_memory.get(&token)?.clone();
    let rest = line.trim_start_matches(|c: char| c.is_ascii_digit());

    if rest.starts_with("^error") {
        pending_memory.remove(&token);
        let msg = extract_str(rest, "msg").unwrap_or_else(|| "GDB error".into());
        Some(StateEvent::MemoryRequestFailed {
            address,
            message: msg,
        })
    } else if rest.starts_with("^done") {
        pending_memory.remove(&token);
        None
    } else {
        None
    }
}

/// Inspects an incoming raw MI line for a token that correlates to a pending
/// `Command::AttachToProcess`. If the token matches an entry in
/// `pending_attach`, removes it (cleanup on both success and failure,
/// mirroring `correlate_pending_catch`). `^error` returns
/// `ProcessAttachFailed{pid, message}` for the caller to emit — GDB's raw
/// `msg=...` text, verbatim (spec: "Attach Failure Surfaced Verbatim").
/// `^done` is cleanup-only and emits no event: success was already signalled
/// optimistically at dispatch time (design D1, `handle_commands`) — a
/// tokened `^done` here carries nothing new.
fn correlate_pending_attach(
    line: &str,
    pending_attach: &mut HashMap<u32, u32>,
) -> Option<StateEvent> {
    let token = parse_token(line)?;
    let pid = *pending_attach.get(&token)?;
    let rest = line.trim_start_matches(|c: char| c.is_ascii_digit());

    if rest.starts_with("^error") {
        pending_attach.remove(&token);
        let msg = extract_str(rest, "msg").unwrap_or_else(|| "GDB error".into());
        Some(StateEvent::ProcessAttachFailed { pid, message: msg })
    } else if rest.starts_with("^done") {
        pending_attach.remove(&token);
        None
    } else {
        None
    }
}

/// Inspects an incoming raw MI line for a token that correlates to the
/// pending `Command::DetachForShutdown` (design D6). If the token matches an
/// entry in `pending_detach`, removes it (cleanup on both outcomes, mirroring
/// `correlate_pending_probe`'s `HashSet<u32>` shape) and returns
/// `DetachFinished{error}` for the caller to emit on **both** `^done`
/// (`error: None`) and `^error` (`error: Some(GDB's raw message)`) — unlike
/// every other `pending_*` map, both outcomes here carry an event: shutdown
/// needs a definite ack (Finished) either way to unblock `wait_for_detach_ack`
/// (a later chained PR), not just cleanup.
fn correlate_pending_detach(
    line: &str,
    pending_detach: &mut HashSet<u32>,
) -> Option<StateEvent> {
    let token = parse_token(line)?;
    if !pending_detach.contains(&token) {
        return None;
    }
    let rest = line.trim_start_matches(|c: char| c.is_ascii_digit());

    if rest.starts_with("^done") {
        pending_detach.remove(&token);
        Some(StateEvent::DetachFinished { error: None })
    } else if rest.starts_with("^error") {
        pending_detach.remove(&token);
        let msg = extract_str(rest, "msg").unwrap_or_else(|| "GDB error".into());
        Some(StateEvent::DetachFinished { error: Some(msg) })
    } else {
        None
    }
}

/// Inspects an incoming raw MI line for a token that correlates to a
/// pending `Command::ConnectRemote`. If the token matches an entry in
/// `pending.remote_connect`, removes it (cleanup on both outcomes, mirroring
/// `correlate_pending_attach`) and returns the event to emit for either
/// outcome — unlike `correlate_pending_attach`, BOTH outcomes here carry an
/// event (design D3/D4): success is correlated off `^connected`, never
/// optimistic at dispatch time, so `RemoteConnectFailed` needs no rollback
/// (nothing was ever set speculatively). `^connected` -> `RemoteConnected
/// {target}`. `^error` -> `RemoteConnectFailed{target, message}` (GDB's raw
/// `msg=...` text, verbatim, mirroring `ProcessAttachFailed`).
fn correlate_pending_remote_connect(
    line: &str,
    pending_remote_connect: &mut HashMap<u32, String>,
) -> Option<StateEvent> {
    let token = parse_token(line)?;
    let target = pending_remote_connect.get(&token)?.clone();
    let rest = line.trim_start_matches(|c: char| c.is_ascii_digit());

    if rest.starts_with("^connected") {
        pending_remote_connect.remove(&token);
        Some(StateEvent::RemoteConnected { target })
    } else if rest.starts_with("^error") {
        pending_remote_connect.remove(&token);
        let msg = extract_str(rest, "msg").unwrap_or_else(|| "GDB error".into());
        Some(StateEvent::RemoteConnectFailed {
            target,
            message: msg,
        })
    } else {
        None
    }
}

/// `correlate_pending_detach` verbatim, retargeted at
/// `Command::DisconnectForShutdown` (design D6/D7). If the token matches an
/// entry in `pending.remote_disconnect`, removes it (cleanup on both
/// outcomes) and returns `RemoteDisconnected{error}` for the caller to
/// emit on **both** `^done` (`error: None`) and `^error` (`error: Some(GDB's
/// raw message)`) — a shutdown ack, not a per-row error attribution.
fn correlate_pending_remote_disconnect(
    line: &str,
    pending_remote_disconnect: &mut HashSet<u32>,
) -> Option<StateEvent> {
    let token = parse_token(line)?;
    if !pending_remote_disconnect.contains(&token) {
        return None;
    }
    let rest = line.trim_start_matches(|c: char| c.is_ascii_digit());

    if rest.starts_with("^done") {
        pending_remote_disconnect.remove(&token);
        Some(StateEvent::RemoteDisconnected { error: None })
    } else if rest.starts_with("^error") {
        pending_remote_disconnect.remove(&token);
        let msg = extract_str(rest, "msg").unwrap_or_else(|| "GDB error".into());
        Some(StateEvent::RemoteDisconnected { error: Some(msg) })
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

/// Builds the `gdb` subprocess `Command` for `raw_args` — the full,
/// unmodified CLI argument list gdb-gui itself was invoked with (after its
/// own binary name). `raw_args` is forwarded to `gdb` completely verbatim,
/// in the original order, with no `--args` auto-injection: real `gdb` only
/// treats trailing args as the debuggee's argv when the user explicitly
/// writes `--args` themselves, so unconditionally inserting it here would
/// wrongly fold the user's own `-ex`/`-x`/etc. gdb options into the
/// debuggee's argv instead of letting `gdb` recognize them as its own
/// options. Split out from `spawn_gdb` so the exact `Command` shape is
/// testable without spawning a real subprocess.
fn build_gdb_command(raw_args: &[String]) -> Command {
    let mut cmd = Command::new("gdb");
    cmd.arg("--interpreter=mi")
        .arg("--quiet")
        .arg("-nx")
        .args(raw_args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    cmd
}

fn spawn_gdb(
    raw_args: &[String],
) -> std::io::Result<(Child, GdbWriter<ChildStdin>, BufReader<ChildStdout>)> {
    let mut cmd = build_gdb_command(raw_args);
    let mut child = cmd.spawn()?;
    let stdin = child.stdin.take().expect("stdin piped");
    let stdout_raw = child.stdout.take().expect("stdout piped");

    let writer = GdbWriter { stdin, seq: 1 };
    let reader = BufReader::new(stdout_raw);

    Ok((child, writer, reader))
}

// ─── run_loop ─────────────────────────────────────────────────────────────────

/// Spawns the background thread that blocks on `reader.read_until(b'\n', ..)`
/// and forwards each trimmed line through the returned channel.
///
/// Reads raw bytes and decodes each line lossily (`String::from_utf8_lossy`)
/// rather than requiring strict UTF-8 (as `BufRead::read_line` does). GDB's
/// stdout can carry the debuggee's own console output verbatim over the
/// remote protocol (e.g. `target remote` to a qemu/kernel target), with no
/// guarantee of UTF-8 validity — a single malformed byte sequence must
/// degrade gracefully (replaced with U+FFFD, like a real terminal) instead of
/// tearing down the read loop, since `from_utf8_lossy` never fails. Only a
/// genuine I/O error from `read_until` itself (e.g. broken pipe) terminates
/// the thread via the `Err` arm below.
///
/// EOF terminates the thread silently on the reader side; a real read error
/// is also reported to the UI via `event_tx` before the thread exits. The
/// `JoinHandle` is returned for the caller to hold (never joined — the
/// thread is expected to outlive `run_loop`'s use of it and terminate on its
/// own when the child's stdout closes).
///
/// Generic over `R: BufRead` — mirrors `GdbWriter<W: Write>` above — so unit
/// tests can substitute an in-memory reader (e.g. `Cursor<Vec<u8>>`) instead
/// of a real `BufReader<ChildStdout>`, which requires a live subprocess.
fn spawn_reader_thread<R: BufRead + Send + 'static>(
    reader: R,
    event_tx: Sender<DebuggerEvent>,
) -> (thread::JoinHandle<()>, Receiver<String>) {
    let (line_tx, line_rx) = std::sync::mpsc::channel::<String>();
    let event_tx_reader = event_tx.clone();

    let handle = thread::spawn(move || {
        let mut reader = reader;
        let mut byte_buf: Vec<u8> = Vec::new();
        loop {
            byte_buf.clear();
            match reader.read_until(b'\n', &mut byte_buf) {
                Ok(0) => break, // EOF
                Ok(_) => {
                    let decoded = String::from_utf8_lossy(&byte_buf);
                    let line = decoded.trim_end_matches('\n').trim_end_matches('\r').to_owned();
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

    (handle, line_rx)
}

/// Groups the 8 in-flight MI-token correlation maps used by `handle_commands`
/// and `handle_gdb_output` into one cohesive concept. Fields are deliberately
/// **distinctly typed** — never a single `HashMap<u32, PendingKind>` — because
/// type-level mutual isolation between correlation domains is a documented
/// correctness guarantee (Engram #95, finding 1): a reply for one domain can
/// never be mistakenly consumed by another domain's correlator.
#[derive(Clone, Debug, Default)]
struct PendingRegistry {
    /// Token (assigned by `GdbWriter::send`) -> id of the breakpoint whose
    /// `-break-condition` is pending a response. GDB echoes the token in its
    /// result record (`{token}^done`/`{token}^error`), which lets us
    /// correlate an `^error` with the exact row that originated it.
    cond: HashMap<u32, u32>,

    /// Token (assigned by `GdbWriter::send`) -> struct-panel expression
    /// pending a response. Correlated by token, not FIFO: kept separate from
    /// `globals` so a struct response is never consumed by the globals path
    /// (and vice versa), even when both are in flight at the same time after
    /// the same pause.
    struct_: HashMap<u32, String>,

    /// Token (assigned by `GdbWriter::send`) -> global-variable name pending
    /// a response. Correlated by token, not FIFO: kept separate from
    /// `struct_` (and vice versa) so a globals response is never consumed by
    /// the struct path, even when both are in flight at the same time after
    /// the same pause.
    globals: HashMap<u32, String>,

    /// Token (assigned by `GdbWriter::send`) -> `EditTarget` of the
    /// `Command::SetValue` write pending a response. Correlated by token, not
    /// FIFO, like the other pending maps: kept separate so a value-edit reply
    /// is never consumed by the struct/globals/cond paths, even when several
    /// are in flight after the same pause.
    edit: HashMap<u32, EditTarget>,

    /// Token (assigned by `GdbWriter::send`) of the in-flight
    /// `Command::ProbeMainSource` probe, if any. Correlated by token like the
    /// other pending sets: its reply is intercepted and `continue`d on before
    /// `parse_line`, so it never reaches `parse_result` and never becomes a
    /// `BreakpointAdded` row (see `correlate_pending_probe`).
    probe: HashSet<u32>,

    /// Token (assigned by `GdbWriter::send`) -> expression of the
    /// `Command::AddWatchpoint` pending a response. Correlated by token, not
    /// FIFO, like the other pending maps: only `^error` needs correlation
    /// here (success is self-describing and flows through the normal
    /// `parse_line` path into `WatchpointAdded`).
    watch: HashMap<u32, String>,

    /// Token (assigned by `GdbWriter::send`) -> D1 key (`"{kind}:{args}"`) of
    /// the `Command::AddCatchpoint` pending a response. Correlated by token,
    /// not FIFO, like the other pending maps: only `^error` needs correlation
    /// here (a successful creation is never send-time optimistic — see
    /// `correlate_pending_catch` doc comment / design addendum A2/A3).
    catch: HashMap<u32, String>,

    /// Token (assigned by `GdbWriter::send`) -> address of the
    /// `Command::RequestMemory` pending a response. Correlated by token, not
    /// FIFO, like the other pending maps: only `^error` needs correlation
    /// here (success is self-describing and flows through the normal
    /// `parse_line` path into `MemoryUpdated`).
    memory: HashMap<u32, String>,

    /// Token (assigned by `GdbWriter::send`) -> `BreakpointInsertRequest`
    /// (file/line) of the `Command::AddBreakpoint` pending a response.
    /// Correlated by token, not FIFO, like the other pending maps: only
    /// `^error` needs correlation here (success is self-describing and
    /// flows through the normal `parse_line` path into `BreakpointAdded`).
    /// Distinct from `cond` (which tracks `-break-condition` on an
    /// *existing* id, not a new insert).
    insert: HashMap<u32, BreakpointInsertRequest>,

    /// Token (assigned by `GdbWriter::send`) -> pid of the
    /// `Command::AttachToProcess` pending a response. Correlated by token,
    /// not FIFO, like the other pending maps: only `^error` needs
    /// correlation here — success is emitted optimistically at dispatch
    /// time (design D1), since the eventual `*stopped` reply is anonymous
    /// (no `reason=`, no pid) and cannot be the source of the success
    /// event.
    attach: HashMap<u32, u32>,

    /// Token (assigned by `GdbWriter::send`) of the in-flight
    /// `Command::DetachForShutdown`, if any (design D6). Payload-free,
    /// mirroring `probe`'s `HashSet<u32>` shape: unlike `attach`, BOTH
    /// `^done` and `^error` emit `StateEvent::DetachFinished{error}` — a
    /// shutdown ack, not a value to attribute an error to.
    detach: HashSet<u32>,

    /// Token (assigned by `GdbWriter::send`) -> target of the
    /// `Command::ConnectRemote` pending a response. Correlated by token, not
    /// FIFO, like the other pending maps: unlike `attach`, BOTH outcomes
    /// carry an event here (design D3/D4) — success is correlated off
    /// `^connected`, never optimistic, so `RemoteConnectFailed` needs no
    /// rollback.
    remote_connect: HashMap<u32, String>,

    /// Token (assigned by `GdbWriter::send`) of the in-flight
    /// `Command::DisconnectForShutdown`, if any (design D6/D7). Payload-free,
    /// mirroring `detach`'s `HashSet<u32>` shape: BOTH `^done` and `^error`
    /// emit `StateEvent::RemoteDisconnected{error}` — a shutdown ack, not a
    /// value to attribute an error to.
    remote_disconnect: HashSet<u32>,
}

/// Drains `cmd_rx` fully, dispatching each `DebuggerCommand` to GDB and
/// tagging the 7 `PendingRegistry` fields as needed. Returns `Err(())` the
/// moment `writer.send` fails (after reporting the error) so the caller can
/// centralize `child.kill()`; every other error path here is non-fatal
/// (`event_tx.send` failures are ignored like the rest of the loop, matching
/// the previous behavior — only the writer failure aborts the drain).
fn handle_commands<W: Write>(
    cmd_rx: &Receiver<DebuggerCommand>,
    writer: &mut GdbWriter<W>,
    event_tx: &Sender<DebuggerEvent>,
    gdb_pid: u32,
    pending: &mut PendingRegistry,
) -> Result<(), ()> {
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
                return Err(());
            }
        };

        if let DebuggerCommand::SetBreakpointCondition { id, .. } = &cmd {
            pending.cond.insert(token, *id);
        }

        if let DebuggerCommand::AddBreakpoint { file, line, .. } = &cmd {
            pending.insert.insert(
                token,
                BreakpointInsertRequest {
                    file: file.clone(),
                    line: *line,
                },
            );
        }

        if let DebuggerCommand::Evaluate(expr) = &cmd {
            pending.struct_.insert(token, expr.clone());
        }

        if let DebuggerCommand::EvaluateGlobal(name) = &cmd {
            pending.globals.insert(token, name.clone());
        }

        if let DebuggerCommand::SetValue { target, .. } = &cmd {
            pending.edit.insert(token, target.clone());
        }

        if matches!(cmd, DebuggerCommand::ProbeMainSource) {
            pending.probe.insert(token);
        }

        if let DebuggerCommand::AddWatchpoint { expr, .. } = &cmd {
            pending.watch.insert(token, expr.clone());
        }

        if let DebuggerCommand::RequestMemory { address, .. } = &cmd {
            pending.memory.insert(token, address.clone());
        }

        // Attach (design D1): a single arm both inserts pending.attach and
        // emits ProcessAttached optimistically, so the two can never drift
        // apart. Success is signalled here, at dispatch time — never
        // derived from a GDB reply: the eventual `*stopped` record is
        // anonymous (no `reason=`, no pid). Only `^error` is correlated
        // back via `correlate_pending_attach`.
        if let DebuggerCommand::AttachToProcess(pid) = &cmd {
            pending.attach.insert(token, *pid);
            let _ = event_tx.send(DebuggerEvent::State(StateEvent::ProcessAttached {
                pid: *pid,
            }));
        }

        // Detach-on-shutdown (design D6): payload-free, mirrors
        // `pending.probe`'s insert. Both `^done` and `^error` are
        // correlated via `correlate_pending_detach` into
        // `DetachFinished{error}` — there is no optimistic event here since
        // the ack itself (not a value) is what the caller needs.
        if matches!(cmd, DebuggerCommand::DetachForShutdown) {
            pending.detach.insert(token);
        }

        // Remote-connect (design D3/D4): unlike attach, no optimistic event
        // is emitted here — success is only ever signalled off the
        // eventual `^connected` reply via `correlate_pending_remote_connect`.
        if let DebuggerCommand::ConnectRemote { target } = &cmd {
            pending.remote_connect.insert(token, target.clone());
        }

        // Remote-disconnect-on-shutdown (design D6/D7): payload-free,
        // mirrors `pending.detach`'s insert. Both `^done` and `^error` are
        // correlated via `correlate_pending_remote_disconnect` into
        // `RemoteDisconnected{error}`.
        if matches!(cmd, DebuggerCommand::DisconnectForShutdown) {
            pending.remote_disconnect.insert(token);
        }

        // D1 key: "{kind}:{args joined by ','}" — matches
        // `catchpoint_errors`' key shape so a later `^error` attributes
        // straight to the row the panel is tracking by (kind, args).
        if let DebuggerCommand::AddCatchpoint { kind, args } = &cmd {
            pending.catch.insert(token, format!("{kind}:{}", args.join(",")));
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

    Ok(())
}

/// Runs the 9 ordered correlation checks against a single raw MI `line`,
/// then falls through to `parse_line`. CRITICAL: exact check order is
/// struct → globals → probe → cond → edit → watch → catch → insert →
/// memory → parse_line. struct/globals/probe early-return `ControlFlow::Continue(())`
/// on a match (short-circuit: the rest of the checks, including
/// `parse_line`, are skipped for this line). cond/edit/watch/catch/memory do
/// NOT early-return on a match: they fall through to the next check and
/// eventually to `parse_line`, so a single line can emit two events. Any
/// `event_tx.send`
/// or `writer.send` error anywhere returns `ControlFlow::Break(())`, which
/// tells the caller to kill the child and stop the loop.
fn handle_gdb_output<W: Write>(
    line: &str,
    writer: &mut GdbWriter<W>,
    event_tx: &Sender<DebuggerEvent>,
    pending: &mut PendingRegistry,
) -> ControlFlow<()> {
    // Struct-panel correlation: checked FIRST, before globals. Both sides are
    // token-keyed maps, so isolation is mutual — neither path can consume the
    // other's reply, regardless of check order.
    if let Some(event) = correlate_pending_struct(line, &mut pending.struct_) {
        if event_tx.send(DebuggerEvent::State(event)).is_err() {
            return ControlFlow::Break(());
        }
        return ControlFlow::Continue(());
    }

    if let Some(event) = correlate_pending_global(line, &mut pending.globals) {
        if event_tx.send(DebuggerEvent::State(event)).is_err() {
            return ControlFlow::Break(());
        }
        return ControlFlow::Continue(());
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
    if let Some(outcome) = correlate_pending_probe(line, &mut pending.probe) {
        if let ProbeOutcome::Resolved { number, file } = outcome {
            if let Err(e) = writer.send(&format!("-break-delete {number}")) {
                let _ = event_tx.send(DebuggerEvent::Ui(UiEvent::GdbError(format!(
                    "Error escribiendo a GDB: {e}"
                ))));
                return ControlFlow::Break(());
            }
            if event_tx
                .send(DebuggerEvent::State(StateEvent::SourcePreviewResolved {
                    file,
                }))
                .is_err()
            {
                return ControlFlow::Break(());
            }
        }
        return ControlFlow::Continue(());
    }

    // -break-condition correlation: an `^error` whose token is in
    // pending_cond is translated into a BreakpointConditionError for the
    // exact row. The console GdbError from parse_line below is still
    // emitted regardless (not replaced), so the log loses nothing.
    if let Some(event) = correlate_pending_cond(line, &mut pending.cond) {
        if event_tx.send(DebuggerEvent::State(event)).is_err() {
            return ControlFlow::Break(());
        }
    }

    // Value-edit correlation: like cond, does not `continue` — an `^error`
    // still falls through to parse_line below so the console log also shows
    // it (not replaced, just supplemented).
    if let Some(event) = correlate_pending_edit(line, &mut pending.edit) {
        if event_tx.send(DebuggerEvent::State(event)).is_err() {
            return ControlFlow::Break(());
        }
    }

    // Watchpoint-creation correlation: like cond/edit, does not `continue`
    // on `^error` — the console log still shows it via parse_line below. On
    // `^done`, the token is cleaned up and the line falls through to
    // parse_line, which turns the self-describing `wpt=`/`hw-rwpt=`/
    // `hw-awpt=` reply into `WatchpointAdded`.
    if let Some(event) = correlate_pending_watch(line, &mut pending.watch) {
        if event_tx.send(DebuggerEvent::State(event)).is_err() {
            return ControlFlow::Break(());
        }
    }

    // Catchpoint-creation correlation: like watch, does not `continue` on
    // `^error` — the console log still shows it via parse_line below. On
    // `^done` (Load/Unload's native, tokened reply), the token is cleaned up
    // and the line falls through to parse_line, which turns its
    // self-describing `bkpt={catch-type=...}` payload into `CatchpointAdded`
    // (same fn the untokened notify-async path uses — A6).
    if let Some(event) = correlate_pending_catch(line, &mut pending.catch) {
        if event_tx.send(DebuggerEvent::State(event)).is_err() {
            return ControlFlow::Break(());
        }
    }

    // Attach correlation (design D1): like watch/catch, does not `continue`
    // on `^error` — the console log still shows it via parse_line below. On
    // `^done`, the token is cleaned up and no event is emitted here: success
    // was already signalled optimistically at dispatch time
    // (`handle_commands`), so a tokened `^done` here carries nothing new.
    if let Some(event) = correlate_pending_attach(line, &mut pending.attach) {
        if event_tx.send(DebuggerEvent::State(event)).is_err() {
            return ControlFlow::Break(());
        }
    }

    // Detach correlation (design D6): like watch/catch/attach, does not
    // `continue` — the console log still shows it via parse_line below.
    // Unlike attach, BOTH `^done` and `^error` emit `DetachFinished{error}`
    // here (a shutdown ack, not a per-row error attribution).
    if let Some(event) = correlate_pending_detach(line, &mut pending.detach) {
        if event_tx.send(DebuggerEvent::State(event)).is_err() {
            return ControlFlow::Break(());
        }
    }

    // Remote-connect correlation (design D3/D4): like attach/detach, does
    // not `continue` — the console log still shows an `^error` via
    // parse_line below. On `^connected`, the token is cleaned up and
    // `RemoteConnected{target}` is emitted directly here (never derived
    // from `parse_line`/`parse_result`, which hits `_ => None` for the
    // "connected" class — verified).
    if let Some(event) = correlate_pending_remote_connect(line, &mut pending.remote_connect) {
        if event_tx.send(DebuggerEvent::State(event)).is_err() {
            return ControlFlow::Break(());
        }
    }

    // Remote-disconnect correlation (design D6/D7): like detach, does not
    // `continue`. Unlike attach, BOTH `^done` and `^error` emit
    // `RemoteDisconnected{error}` here (a shutdown ack, not a per-row error
    // attribution).
    if let Some(event) = correlate_pending_remote_disconnect(line, &mut pending.remote_disconnect)
    {
        if event_tx.send(DebuggerEvent::State(event)).is_err() {
            return ControlFlow::Break(());
        }
    }

    // Breakpoint-insert correlation: like watch/catch, does not `continue`
    // on `^error` — the console log still shows it via parse_line below. On
    // `^done`, the token is cleaned up and the line falls through to
    // parse_line, which turns the self-describing `bkpt={...}` reply into
    // `BreakpointAdded` (same path a real, uncorrelated insert already
    // used).
    if let Some(event) = correlate_pending_insert(line, &mut pending.insert) {
        if event_tx.send(DebuggerEvent::State(event)).is_err() {
            return ControlFlow::Break(());
        }
    }

    // Memory-read correlation: like watch/catch, does not `continue` on
    // `^error` — the console log still shows it via parse_line below. On
    // `^done`, the token is cleaned up and the line falls through to
    // parse_line, which turns the self-describing `memory=[...]` reply into
    // `MemoryUpdated`.
    if let Some(event) = correlate_pending_memory(line, &mut pending.memory) {
        if event_tx.send(DebuggerEvent::State(event)).is_err() {
            return ControlFlow::Break(());
        }
    }

    if let Some(event) = parse_line(line) {
        // None = ignorable line, not an error
        if event_tx.send(event).is_err() {
            return ControlFlow::Break(());
        }
    }

    ControlFlow::Continue(())
}

pub fn run_loop(
    executable: Option<String>,
    raw_args: Vec<String>,
    cmd_rx: Receiver<DebuggerCommand>,
    event_tx: Sender<DebuggerEvent>,
) {
    let (mut child, mut writer, reader) = match spawn_gdb(&raw_args) {
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

    let (_reader_handle, line_rx) = spawn_reader_thread(reader, event_tx.clone());

    // In-flight MI-token correlation state for all 7 correlation domains —
    // see `PendingRegistry`'s field doc comments for what each one tracks.
    let mut pending = PendingRegistry::default();

    loop {
        if handle_commands(&cmd_rx, &mut writer, &event_tx, gdb_pid, &mut pending).is_err() {
            let _ = child.kill();
            return;
        }

        // Raw MI protocol records (^done, *stopped, =notify-async, …) are not
        // echoed to the console: parse_line already translates them into state
        // events, and real errors arrive separately as GdbError. Only stream
        // records (~ @) produce readable text for the user.
        while let Ok(line) = line_rx.try_recv() {
            if handle_gdb_output(&line, &mut writer, &event_tx, &mut pending).is_break() {
                let _ = child.kill();
                return;
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

    fn command_args(cmd: &Command) -> Vec<String> {
        cmd.get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn build_gdb_command_forwards_raw_args_verbatim_no_args_injection() {
        // Exact bug-report shape: the executable followed by `-ex` flags must
        // reach `gdb` completely unmodified, in the original order, with no
        // `--args` inserted anywhere — otherwise gdb folds the `-ex` values
        // into the debuggee's own argv instead of executing them.
        let raw_args = vec![
            "kernel.elf".to_string(),
            "-ex".to_string(),
            "source orga2.py".to_string(),
            "-ex".to_string(),
            "target remote localhost:1234".to_string(),
        ];
        let cmd = build_gdb_command(&raw_args);

        assert_eq!(cmd.get_program().to_string_lossy(), "gdb");
        assert_eq!(
            command_args(&cmd),
            vec![
                "--interpreter=mi".to_string(),
                "--quiet".to_string(),
                "-nx".to_string(),
                "kernel.elf".to_string(),
                "-ex".to_string(),
                "source orga2.py".to_string(),
                "-ex".to_string(),
                "target remote localhost:1234".to_string(),
            ]
        );
    }

    #[test]
    fn build_gdb_command_with_no_raw_args_has_only_base_flags() {
        let cmd = build_gdb_command(&[]);
        assert_eq!(
            command_args(&cmd),
            vec![
                "--interpreter=mi".to_string(),
                "--quiet".to_string(),
                "-nx".to_string(),
            ]
        );
    }

    #[test]
    fn build_gdb_command_preserves_explicit_user_args_flag_verbatim() {
        // If the user explicitly writes --args themselves, it must be
        // forwarded as-is (gdb-gui never inserts or strips it).
        let raw_args = vec![
            "--args".to_string(),
            "kernel.elf".to_string(),
            "foo".to_string(),
            "bar".to_string(),
        ];
        let cmd = build_gdb_command(&raw_args);
        assert_eq!(
            command_args(&cmd),
            vec![
                "--interpreter=mi".to_string(),
                "--quiet".to_string(),
                "-nx".to_string(),
                "--args".to_string(),
                "kernel.elf".to_string(),
                "foo".to_string(),
                "bar".to_string(),
            ]
        );
    }

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

    // Phase 2 (persistence-serialization): mirrors
    // `pending_cond_insert_and_removal_on_matching_reply` /
    // `correlate_pending_cond_emits_error_for_correct_row` — a failed
    // `-break-insert` has no id to attach an error to, so it is correlated
    // by token back to the requested `file`/`line` instead.
    #[test]
    fn pending_insert_insert_and_removal_on_matching_done_reply() {
        let mut pending_insert: HashMap<u32, BreakpointInsertRequest> = HashMap::new();
        pending_insert.insert(
            7,
            BreakpointInsertRequest {
                file: "/tmp/main.c".into(),
                line: 10,
            },
        );

        // A `^done` (success) for the matching token must remove the entry
        // and emit no new event — the self-describing `^done,bkpt={...}`
        // reply is parsed separately by `parse_line`/`parse_result` into
        // `BreakpointAdded`.
        let result = correlate_pending_insert(
            "7^done,bkpt={number=\"3\",fullname=\"/tmp/main.c\",line=\"10\"}",
            &mut pending_insert,
        );
        assert!(result.is_none());
        assert!(
            !pending_insert.contains_key(&7),
            "token must be removed after a matching ^done"
        );
    }

    #[test]
    fn correlate_pending_insert_emits_error_for_correct_file_and_line() {
        let mut pending_insert: HashMap<u32, BreakpointInsertRequest> = HashMap::new();
        pending_insert.insert(
            9,
            BreakpointInsertRequest {
                file: "/tmp/deleted.c".into(),
                line: 42,
            },
        );

        let event = correlate_pending_insert(
            "9^error,msg=\"No such file or directory.\"",
            &mut pending_insert,
        );

        match event {
            Some(StateEvent::BreakpointInsertFailed { file, line, message }) => {
                assert_eq!(file, "/tmp/deleted.c");
                assert_eq!(line, 42);
                assert_eq!(message, "No such file or directory.");
            }
            other => panic!("expected BreakpointInsertFailed, got {other:?}"),
        }
        assert!(
            !pending_insert.contains_key(&9),
            "token must be removed after a matching ^error"
        );
    }

    #[test]
    fn correlate_pending_insert_ignores_unrelated_tokens() {
        let mut pending_insert: HashMap<u32, BreakpointInsertRequest> = HashMap::new();
        pending_insert.insert(
            1,
            BreakpointInsertRequest {
                file: "/tmp/a.c".into(),
                line: 5,
            },
        );

        let event = correlate_pending_insert("2^done", &mut pending_insert);
        assert!(event.is_none());
        assert!(pending_insert.contains_key(&1));
    }

    #[test]
    fn handle_commands_inserts_pending_insert_entry_on_add_breakpoint_dispatch() {
        let mut buf: Vec<u8> = Vec::new();
        let mut writer = GdbWriter { stdin: &mut buf, seq: 0 };
        let (cmd_tx, cmd_rx) = std::sync::mpsc::channel();
        let (event_tx, _event_rx) = std::sync::mpsc::channel();
        let mut pending = PendingRegistry::default();

        cmd_tx
            .send(DebuggerCommand::AddBreakpoint {
                file: "/tmp/main.c".into(),
                line: 10,
                condition: None,
            })
            .unwrap();

        handle_commands(&cmd_rx, &mut writer, &event_tx, 1234, &mut pending).unwrap();

        assert_eq!(
            pending.insert.get(&0),
            Some(&BreakpointInsertRequest {
                file: "/tmp/main.c".into(),
                line: 10,
            })
        );
    }

    #[test]
    fn handle_gdb_output_insert_error_falls_through_to_parse_line() {
        let (event_tx, event_rx) = std::sync::mpsc::channel::<DebuggerEvent>();
        let mut writer = GdbWriter {
            stdin: Vec::<u8>::new(),
            seq: 1,
        };
        let mut pending = PendingRegistry::default();
        pending.insert.insert(
            7,
            BreakpointInsertRequest {
                file: "/tmp/deleted.c".into(),
                line: 42,
            },
        );

        let flow = handle_gdb_output(
            "7^error,msg=\"No such file or directory.\"",
            &mut writer,
            &event_tx,
            &mut pending,
        );

        assert_eq!(flow, ControlFlow::Continue(()));
        assert!(!pending.insert.contains_key(&7));

        // Fall-through: the same line emits BOTH BreakpointInsertFailed AND
        // the GdbError from parse_line, mirroring cond/edit/watch/catch/memory.
        let first = event_rx.try_recv().expect("BreakpointInsertFailed event");
        match first {
            DebuggerEvent::State(StateEvent::BreakpointInsertFailed { file, line, message }) => {
                assert_eq!(file, "/tmp/deleted.c");
                assert_eq!(line, 42);
                assert_eq!(message, "No such file or directory.");
            }
            other => panic!("expected BreakpointInsertFailed, got {other:?}"),
        }

        let second = event_rx
            .try_recv()
            .expect("GdbError event from parse_line fall-through");
        match second {
            DebuggerEvent::Ui(UiEvent::GdbError(msg)) => {
                assert_eq!(msg, "No such file or directory.");
            }
            other => panic!("expected GdbError, got {other:?}"),
        }
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

    // Phase 2c task 4.1 (verify-only — design deviation, see
    // apply-progress): the key is built from `Display for CatchpointKind`
    // (`"throw"`), so a rejected Throw creation routes to the same Pending
    // Errors path with no new code. `pending_catch`/`correlate_pending_catch`
    // are left as `HashMap<u32, String>` — the process.rs `PendingCatch`
    // struct and tokened-echo correlation the design (D4) anticipated are
    // unnecessary because the Phase 0 spike found the regexp round-trips
    // through a dedicated `regexp=` field on the parser's self-describing
    // ingress path (see `parse_catchpoint_field`), the same mechanism every
    // other catchpoint kind already uses.
    #[test]
    fn correlate_pending_catch_emits_error_for_throw_key() {
        let mut pending_catch: HashMap<u32, String> = HashMap::new();
        pending_catch.insert(12, "throw:std::runtime_error".into());

        let event = correlate_pending_catch(
            "12^error,msg=\"Junk after catchpoint condition\"",
            &mut pending_catch,
        );

        match event {
            Some(StateEvent::CatchpointError { key, message }) => {
                assert_eq!(key, "throw:std::runtime_error");
                assert_eq!(message, "Junk after catchpoint condition");
            }
            other => panic!("expected CatchpointError, got {other:?}"),
        }
        assert!(!pending_catch.contains_key(&12));
    }

    // Same cleanup-only shape as Load's tokened ^done (verified live: the
    // Phase 0 spike's `-catch-throw` reply is ALSO tokened) — the
    // self-describing `bkpt={..., regexp=...}` payload is parsed separately
    // by `parse_line` into `CatchpointAdded` with the correct args already
    // attached; this token exists only to route a rejected creation's
    // `^error`.
    #[test]
    fn correlate_pending_catch_throw_done_is_cleanup_only_no_event() {
        let mut pending_catch: HashMap<u32, String> = HashMap::new();
        pending_catch.insert(13, "throw:std::runtime_error".into());

        let result = correlate_pending_catch(
            "13^done,bkpt={number=\"4\",type=\"catchpoint\",catch-type=\"throw\",regexp=\"std::runtime_error\"}",
            &mut pending_catch,
        );
        assert!(result.is_none());
        assert!(!pending_catch.contains_key(&13));
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

    #[test]
    fn handle_gdb_output_probe_resolved_short_circuits_skips_parse_line() {
        let (event_tx, event_rx) = std::sync::mpsc::channel::<DebuggerEvent>();
        let mut writer = GdbWriter {
            stdin: Vec::<u8>::new(),
            seq: 1,
        };
        let mut pending = PendingRegistry::default();
        pending.probe.insert(9);

        let flow = handle_gdb_output(
            "9^done,bkpt={number=\"3\",fullname=\"/tmp/main.c\",line=\"5\"}",
            &mut writer,
            &event_tx,
            &mut pending,
        );

        assert_eq!(flow, ControlFlow::Continue(()));
        assert!(
            !pending.probe.contains(&9),
            "token must be removed after a matching ^done"
        );

        // The probe reply must write its own -break-delete directly through
        // the writer, bypassing Command::RemoveBreakpoint. `GdbWriter::send`
        // prefixes the MI token it assigned (seq=1 here) to the raw command.
        assert_eq!(String::from_utf8(writer.stdin).unwrap(), "1-break-delete 3\n");

        // Exactly one event: SourcePreviewResolved. The short-circuit means
        // the same line never reaches parse_line, so it must NOT also
        // become a BreakpointAdded row.
        let event = event_rx.try_recv().expect("SourcePreviewResolved event");
        match event {
            DebuggerEvent::State(StateEvent::SourcePreviewResolved { file }) => {
                assert_eq!(file, "/tmp/main.c");
            }
            other => panic!("expected SourcePreviewResolved, got {other:?}"),
        }
        assert!(
            event_rx.try_recv().is_err(),
            "probe short-circuit must skip parse_line — no second event allowed"
        );
    }

    #[test]
    fn handle_gdb_output_cond_error_falls_through_to_parse_line() {
        let (event_tx, event_rx) = std::sync::mpsc::channel::<DebuggerEvent>();
        let mut writer = GdbWriter {
            stdin: Vec::<u8>::new(),
            seq: 1,
        };
        let mut pending = PendingRegistry::default();
        pending.cond.insert(7, 42);

        let flow = handle_gdb_output(
            "7^error,msg=\"No symbol \\\"unknown_symbol_xyz\\\" in current context.\"",
            &mut writer,
            &event_tx,
            &mut pending,
        );

        assert_eq!(flow, ControlFlow::Continue(()));
        assert!(!pending.cond.contains_key(&7));

        // Fall-through: the same line emits BOTH BreakpointConditionError
        // AND the GdbError from parse_line — the console log must not lose
        // the raw error just because it was correlated to a row.
        let first = event_rx.try_recv().expect("BreakpointConditionError event");
        match first {
            DebuggerEvent::State(StateEvent::BreakpointConditionError { id, message }) => {
                assert_eq!(id, 42);
                assert_eq!(message, "No symbol \"unknown_symbol_xyz\" in current context.");
            }
            other => panic!("expected BreakpointConditionError, got {other:?}"),
        }

        let second = event_rx
            .try_recv()
            .expect("GdbError event from parse_line fall-through");
        match second {
            DebuggerEvent::Ui(UiEvent::GdbError(msg)) => {
                assert_eq!(msg, "No symbol \"unknown_symbol_xyz\" in current context.");
            }
            other => panic!("expected GdbError, got {other:?}"),
        }
    }

    // ── Memory-read correlation (^error only, D2) ────────────────────────────

    #[test]
    fn correlate_pending_memory_emits_error_for_correct_address() {
        let mut pending_memory: HashMap<u32, String> = HashMap::new();
        pending_memory.insert(9, "&&x".into());

        let event = correlate_pending_memory(
            "9^error,msg=\"No symbol \\\"x\\\" in current context.\"",
            &mut pending_memory,
        );

        match event {
            Some(StateEvent::MemoryRequestFailed { address, message }) => {
                assert_eq!(address, "&&x");
                assert_eq!(message, "No symbol \"x\" in current context.");
            }
            other => panic!("expected MemoryRequestFailed, got {other:?}"),
        }
        assert!(
            !pending_memory.contains_key(&9),
            "token must be removed after a matching ^error"
        );
    }

    #[test]
    fn correlate_pending_memory_done_is_cleanup_only_no_event() {
        let mut pending_memory: HashMap<u32, String> = HashMap::new();
        pending_memory.insert(7, "$sp".into());

        // Success carries no event here: the self-describing memory=[...]
        // reply is parsed separately by parse_line into MemoryUpdated.
        let result = correlate_pending_memory(
            "7^done,memory=[{begin=\"0x1\",offset=\"0x0\",end=\"0x2\",contents=\"ab\"}]",
            &mut pending_memory,
        );
        assert!(result.is_none());
        assert!(
            !pending_memory.contains_key(&7),
            "token must be removed after a matching ^done"
        );
    }

    #[test]
    fn correlate_pending_memory_ignores_unrelated_tokens() {
        let mut pending_memory: HashMap<u32, String> = HashMap::new();
        pending_memory.insert(1, "kept".into());

        let event = correlate_pending_memory("2^done", &mut pending_memory);
        assert!(event.is_none());
        assert!(pending_memory.contains_key(&1));
    }

    #[test]
    fn handle_commands_inserts_pending_memory_entry_on_request_memory_dispatch() {
        let mut buf: Vec<u8> = Vec::new();
        let mut writer = GdbWriter { stdin: &mut buf, seq: 0 };
        let (cmd_tx, cmd_rx) = std::sync::mpsc::channel();
        let (event_tx, _event_rx) = std::sync::mpsc::channel();
        let mut pending = PendingRegistry::default();

        cmd_tx
            .send(DebuggerCommand::RequestMemory {
                address: "$sp".into(),
                count: 256,
            })
            .unwrap();

        handle_commands(&cmd_rx, &mut writer, &event_tx, 1234, &mut pending).unwrap();

        assert_eq!(pending.memory.get(&0), Some(&"$sp".to_string()));
    }

    // ── Attach / Detach (Phase 2) ────────────────────────────────────────────
    //
    // pending.attach: HashMap<u32, u32> (token -> pid), error-only —
    // mirrors pending.catch/pending.watch's lifecycle exactly. D1: success
    // (`ProcessAttached`) is emitted OPTIMISTICALLY at dispatch time, in the
    // same handle_commands arm that inserts into pending.attach — the
    // eventual `*stopped` reply is anonymous (no pid, no reason=), so it
    // cannot be the signal.

    // (a) dispatch inserts pending.attach[token]=pid AND emits
    // ProcessAttached optimistically.
    #[test]
    fn attach_dispatch_inserts_pending_attach_and_emits_process_attached_optimistically() {
        let mut buf: Vec<u8> = Vec::new();
        let mut writer = GdbWriter { stdin: &mut buf, seq: 0 };
        let (cmd_tx, cmd_rx) = std::sync::mpsc::channel();
        let (event_tx, event_rx) = std::sync::mpsc::channel();
        let mut pending = PendingRegistry::default();

        cmd_tx.send(DebuggerCommand::AttachToProcess(4242)).unwrap();

        handle_commands(&cmd_rx, &mut writer, &event_tx, 1234, &mut pending).unwrap();

        assert_eq!(pending.attach.get(&0), Some(&4242));

        // The console echo of the composed MI command is sent first; the
        // optimistic ProcessAttached event follows it.
        let console = event_rx.try_recv().expect("console echo event");
        match console {
            DebuggerEvent::Ui(UiEvent::ConsoleOutput(text)) => {
                assert_eq!(text, "> -target-attach 4242");
            }
            other => panic!("expected ConsoleOutput echo, got {other:?}"),
        }

        let event = event_rx.try_recv().expect("ProcessAttached event");
        match event {
            DebuggerEvent::State(StateEvent::ProcessAttached { pid }) => {
                assert_eq!(pid, 4242);
            }
            other => panic!("expected ProcessAttached, got {other:?}"),
        }
    }

    // (d) unrelated token ignored (correlate_pending_attach half).
    #[test]
    fn correlate_pending_attach_ignores_unrelated_tokens() {
        let mut pending_attach: HashMap<u32, u32> = HashMap::new();
        pending_attach.insert(1, 4242);

        let event = correlate_pending_attach("2^done", &mut pending_attach);
        assert!(event.is_none());
        assert!(
            pending_attach.contains_key(&1),
            "unrelated entry must survive untouched"
        );
    }

    // (b) `{t}^error,msg="..."` -> ProcessAttachFailed{pid, verbatim msg} +
    // token removed.
    #[test]
    fn correlate_pending_attach_emits_error_with_verbatim_message_and_removes_token() {
        let mut pending_attach: HashMap<u32, u32> = HashMap::new();
        pending_attach.insert(9, 4242);

        let event = correlate_pending_attach(
            "9^error,msg=\"ptrace: Operation not permitted.\"",
            &mut pending_attach,
        );

        match event {
            Some(StateEvent::ProcessAttachFailed { pid, message }) => {
                assert_eq!(pid, 4242);
                assert_eq!(message, "ptrace: Operation not permitted.");
            }
            other => panic!("expected ProcessAttachFailed, got {other:?}"),
        }
        assert!(
            !pending_attach.contains_key(&9),
            "token must be removed after a matching ^error"
        );
    }

    // (c) `{t}^done` -> token removed, no event (cleanup-only, mirrors
    // correlate_pending_catch's ^done).
    #[test]
    fn correlate_pending_attach_done_is_cleanup_only_no_event() {
        let mut pending_attach: HashMap<u32, u32> = HashMap::new();
        pending_attach.insert(7, 4242);

        let result = correlate_pending_attach("7^done", &mut pending_attach);
        assert!(result.is_none());
        assert!(
            !pending_attach.contains_key(&7),
            "token must be removed after a matching ^done"
        );
    }

    // (e) attach + catch in flight resolve independently — mirrors
    // struct_and_global_evaluations_in_flight_simultaneously_resolve_independently.
    #[test]
    fn attach_and_catch_in_flight_resolve_independently() {
        let mut pending_attach: HashMap<u32, u32> = HashMap::new();
        pending_attach.insert(30, 4242);
        let mut pending_catch: HashMap<u32, String> = HashMap::new();
        pending_catch.insert(31, "fork:".into());

        // The catch reply arrives first; the attach path must not touch it,
        // since its token isn't tracked there.
        let attach_attempt = correlate_pending_attach("31^done", &mut pending_attach);
        assert!(attach_attempt.is_none());
        assert!(pending_attach.contains_key(&30));
        assert!(pending_catch.contains_key(&31));

        let catch_event = correlate_pending_catch("31^error,msg=\"boom\"", &mut pending_catch);
        match catch_event {
            Some(StateEvent::CatchpointError { key, message }) => {
                assert_eq!(key, "fork:");
                assert_eq!(message, "boom");
            }
            other => panic!("expected CatchpointError, got {other:?}"),
        }
        assert!(!pending_catch.contains_key(&31));

        // The attach reply resolves its own token afterward, unaffected.
        let attach_event = correlate_pending_attach(
            "30^error,msg=\"ptrace: Operation not permitted.\"",
            &mut pending_attach,
        );
        match attach_event {
            Some(StateEvent::ProcessAttachFailed { pid, message }) => {
                assert_eq!(pid, 4242);
                assert_eq!(message, "ptrace: Operation not permitted.");
            }
            other => panic!("expected ProcessAttachFailed, got {other:?}"),
        }
        assert!(!pending_attach.contains_key(&30));
    }

    // (f) detach ^done/^error -> DetachFinished, token removed. pending.detach
    // is a HashSet<u32> (token only, payload-free), mirroring pending.probe.
    #[test]
    fn correlate_pending_detach_done_emits_detach_finished_with_no_error() {
        let mut pending_detach: HashSet<u32> = HashSet::new();
        pending_detach.insert(5);

        let event = correlate_pending_detach("5^done", &mut pending_detach);

        match event {
            Some(StateEvent::DetachFinished { error }) => assert_eq!(error, None),
            other => panic!("expected DetachFinished, got {other:?}"),
        }
        assert!(
            !pending_detach.contains(&5),
            "token must be removed after a matching ^done"
        );
    }

    #[test]
    fn correlate_pending_detach_error_emits_detach_finished_with_verbatim_message() {
        let mut pending_detach: HashSet<u32> = HashSet::new();
        pending_detach.insert(6);

        let event = correlate_pending_detach(
            "6^error,msg=\"Cannot detach: no attached target.\"",
            &mut pending_detach,
        );

        match event {
            Some(StateEvent::DetachFinished { error }) => {
                assert_eq!(error, Some("Cannot detach: no attached target.".to_string()));
            }
            other => panic!("expected DetachFinished, got {other:?}"),
        }
        assert!(
            !pending_detach.contains(&6),
            "token must be removed after a matching ^error"
        );
    }

    #[test]
    fn correlate_pending_detach_ignores_unrelated_tokens() {
        let mut pending_detach: HashSet<u32> = HashSet::new();
        pending_detach.insert(1);

        let event = correlate_pending_detach("2^done", &mut pending_detach);
        assert!(event.is_none());
        assert!(pending_detach.contains(&1));
    }

    #[test]
    fn handle_commands_inserts_pending_detach_entry_on_detach_for_shutdown_dispatch() {
        let mut buf: Vec<u8> = Vec::new();
        let mut writer = GdbWriter { stdin: &mut buf, seq: 0 };
        let (cmd_tx, cmd_rx) = std::sync::mpsc::channel();
        let (event_tx, _event_rx) = std::sync::mpsc::channel();
        let mut pending = PendingRegistry::default();

        cmd_tx.send(DebuggerCommand::DetachForShutdown).unwrap();

        handle_commands(&cmd_rx, &mut writer, &event_tx, 1234, &mut pending).unwrap();

        assert!(pending.detach.contains(&0));
    }

    // Check order in handle_gdb_output: … watch -> catch -> attach ->
    // detach -> insert -> memory -> parse_line (design.md). Fall-through
    // (not short-circuit), like watch/catch/insert/memory: the console log
    // still shows the raw GdbError via parse_line.
    #[test]
    fn handle_gdb_output_attach_error_falls_through_to_parse_line() {
        let (event_tx, event_rx) = std::sync::mpsc::channel::<DebuggerEvent>();
        let mut writer = GdbWriter {
            stdin: Vec::<u8>::new(),
            seq: 1,
        };
        let mut pending = PendingRegistry::default();
        pending.attach.insert(7, 4242);

        let flow = handle_gdb_output(
            "7^error,msg=\"ptrace: Operation not permitted.\"",
            &mut writer,
            &event_tx,
            &mut pending,
        );

        assert_eq!(flow, ControlFlow::Continue(()));
        assert!(!pending.attach.contains_key(&7));

        let first = event_rx.try_recv().expect("ProcessAttachFailed event");
        match first {
            DebuggerEvent::State(StateEvent::ProcessAttachFailed { pid, message }) => {
                assert_eq!(pid, 4242);
                assert_eq!(message, "ptrace: Operation not permitted.");
            }
            other => panic!("expected ProcessAttachFailed, got {other:?}"),
        }

        let second = event_rx
            .try_recv()
            .expect("GdbError event from parse_line fall-through");
        match second {
            DebuggerEvent::Ui(UiEvent::GdbError(msg)) => {
                assert_eq!(msg, "ptrace: Operation not permitted.");
            }
            other => panic!("expected GdbError, got {other:?}"),
        }
    }

    #[test]
    fn handle_gdb_output_detach_done_emits_detach_finished_and_falls_through() {
        let (event_tx, event_rx) = std::sync::mpsc::channel::<DebuggerEvent>();
        let mut writer = GdbWriter {
            stdin: Vec::<u8>::new(),
            seq: 1,
        };
        let mut pending = PendingRegistry::default();
        pending.detach.insert(8);

        let flow = handle_gdb_output("8^done", &mut writer, &event_tx, &mut pending);

        assert_eq!(flow, ControlFlow::Continue(()));
        assert!(!pending.detach.contains(&8));

        let event = event_rx.try_recv().expect("DetachFinished event");
        match event {
            DebuggerEvent::State(StateEvent::DetachFinished { error }) => assert_eq!(error, None),
            other => panic!("expected DetachFinished, got {other:?}"),
        }
    }

    // ── Remote target connect/disconnect (Phase 2) ──────────────────────────
    //
    // pending.remote_connect: HashMap<u32, String> (token -> target), BOTH
    // outcomes carry an event (design D3: success is correlated off
    // `^connected` — never optimistic, unlike attach — so
    // `RemoteConnectFailed` needs no rollback).

    #[test]
    fn correlate_pending_remote_connect_emits_remote_connected_and_removes_token_on_connected() {
        let mut pending_remote_connect: HashMap<u32, String> = HashMap::new();
        pending_remote_connect.insert(4, "localhost:1234".into());

        let event = correlate_pending_remote_connect("4^connected", &mut pending_remote_connect);

        match event {
            Some(StateEvent::RemoteConnected { target }) => {
                assert_eq!(target, "localhost:1234");
            }
            other => panic!("expected RemoteConnected, got {other:?}"),
        }
        assert!(
            !pending_remote_connect.contains_key(&4),
            "token must be removed after a matching ^connected"
        );
    }

    #[test]
    fn correlate_pending_remote_connect_emits_error_with_verbatim_message_and_removes_token() {
        let mut pending_remote_connect: HashMap<u32, String> = HashMap::new();
        pending_remote_connect.insert(9, "localhost:9999".into());

        let event = correlate_pending_remote_connect(
            "9^error,msg=\"localhost:9999: Connection refused.\"",
            &mut pending_remote_connect,
        );

        match event {
            Some(StateEvent::RemoteConnectFailed { target, message }) => {
                assert_eq!(target, "localhost:9999");
                assert_eq!(message, "localhost:9999: Connection refused.");
            }
            other => panic!("expected RemoteConnectFailed, got {other:?}"),
        }
        assert!(
            !pending_remote_connect.contains_key(&9),
            "token must be removed after a matching ^error"
        );
    }

    #[test]
    fn correlate_pending_remote_connect_ignores_unrelated_tokens() {
        let mut pending_remote_connect: HashMap<u32, String> = HashMap::new();
        pending_remote_connect.insert(1, "localhost:1234".into());

        let event = correlate_pending_remote_connect("2^connected", &mut pending_remote_connect);
        assert!(event.is_none());
        assert!(
            pending_remote_connect.contains_key(&1),
            "unrelated entry must survive untouched"
        );
    }

    #[test]
    fn handle_commands_inserts_pending_remote_connect_entry_on_connect_remote_dispatch() {
        let mut buf: Vec<u8> = Vec::new();
        let mut writer = GdbWriter { stdin: &mut buf, seq: 0 };
        let (cmd_tx, cmd_rx) = std::sync::mpsc::channel();
        let (event_tx, _event_rx) = std::sync::mpsc::channel();
        let mut pending = PendingRegistry::default();

        cmd_tx
            .send(DebuggerCommand::ConnectRemote {
                target: "localhost:1234".into(),
            })
            .unwrap();

        handle_commands(&cmd_rx, &mut writer, &event_tx, 1234, &mut pending).unwrap();

        assert_eq!(
            pending.remote_connect.get(&0),
            Some(&"localhost:1234".to_string())
        );
    }

    // pending.remote_disconnect: HashSet<u32> (token only), mirrors
    // pending.detach exactly (design D6/D7).

    #[test]
    fn correlate_pending_remote_disconnect_done_emits_remote_disconnected_with_no_error() {
        let mut pending_remote_disconnect: HashSet<u32> = HashSet::new();
        pending_remote_disconnect.insert(5);

        let event =
            correlate_pending_remote_disconnect("5^done", &mut pending_remote_disconnect);

        match event {
            Some(StateEvent::RemoteDisconnected { error }) => assert_eq!(error, None),
            other => panic!("expected RemoteDisconnected, got {other:?}"),
        }
        assert!(
            !pending_remote_disconnect.contains(&5),
            "token must be removed after a matching ^done"
        );
    }

    #[test]
    fn correlate_pending_remote_disconnect_error_emits_remote_disconnected_with_verbatim_message()
     {
        let mut pending_remote_disconnect: HashSet<u32> = HashSet::new();
        pending_remote_disconnect.insert(6);

        let event = correlate_pending_remote_disconnect(
            "6^error,msg=\"Remote connection closed\"",
            &mut pending_remote_disconnect,
        );

        match event {
            Some(StateEvent::RemoteDisconnected { error }) => {
                assert_eq!(error, Some("Remote connection closed".to_string()));
            }
            other => panic!("expected RemoteDisconnected, got {other:?}"),
        }
        assert!(!pending_remote_disconnect.contains(&6));
    }

    #[test]
    fn correlate_pending_remote_disconnect_ignores_unrelated_tokens() {
        let mut pending_remote_disconnect: HashSet<u32> = HashSet::new();
        pending_remote_disconnect.insert(1);

        let event = correlate_pending_remote_disconnect("2^done", &mut pending_remote_disconnect);
        assert!(event.is_none());
        assert!(pending_remote_disconnect.contains(&1));
    }

    #[test]
    fn handle_commands_inserts_pending_remote_disconnect_entry_on_disconnect_for_shutdown_dispatch()
     {
        let mut buf: Vec<u8> = Vec::new();
        let mut writer = GdbWriter { stdin: &mut buf, seq: 0 };
        let (cmd_tx, cmd_rx) = std::sync::mpsc::channel();
        let (event_tx, _event_rx) = std::sync::mpsc::channel();
        let mut pending = PendingRegistry::default();

        cmd_tx.send(DebuggerCommand::DisconnectForShutdown).unwrap();

        handle_commands(&cmd_rx, &mut writer, &event_tx, 1234, &mut pending).unwrap();

        assert!(pending.remote_disconnect.contains(&0));
    }

    // Attach-and-connect in flight resolve independently — mirrors
    // attach_and_catch_in_flight_resolve_independently.
    #[test]
    fn attach_and_remote_connect_in_flight_resolve_independently() {
        let mut pending_attach: HashMap<u32, u32> = HashMap::new();
        pending_attach.insert(30, 4242);
        let mut pending_remote_connect: HashMap<u32, String> = HashMap::new();
        pending_remote_connect.insert(31, "localhost:1234".into());

        // The remote-connect reply arrives first; the attach path must not
        // touch it, since its token isn't tracked there.
        let attach_attempt = correlate_pending_attach("31^done", &mut pending_attach);
        assert!(attach_attempt.is_none());
        assert!(pending_attach.contains_key(&30));
        assert!(pending_remote_connect.contains_key(&31));

        let connect_event =
            correlate_pending_remote_connect("31^connected", &mut pending_remote_connect);
        match connect_event {
            Some(StateEvent::RemoteConnected { target }) => {
                assert_eq!(target, "localhost:1234");
            }
            other => panic!("expected RemoteConnected, got {other:?}"),
        }
        assert!(!pending_remote_connect.contains_key(&31));

        // The attach reply resolves its own token afterward, unaffected.
        let attach_event = correlate_pending_attach(
            "30^error,msg=\"ptrace: Operation not permitted.\"",
            &mut pending_attach,
        );
        match attach_event {
            Some(StateEvent::ProcessAttachFailed { pid, message }) => {
                assert_eq!(pid, 4242);
                assert_eq!(message, "ptrace: Operation not permitted.");
            }
            other => panic!("expected ProcessAttachFailed, got {other:?}"),
        }
        assert!(!pending_attach.contains_key(&30));
    }

    // Check order in handle_gdb_output (design.md): … detach -> remote_connect
    // -> remote_disconnect -> insert -> memory -> parse_line. Fall-through
    // (not short-circuit): the console log still shows the raw GdbError via
    // parse_line, mirroring watch/catch/attach/detach/insert/memory.
    #[test]
    fn handle_gdb_output_remote_connect_error_falls_through_to_parse_line() {
        let (event_tx, event_rx) = std::sync::mpsc::channel::<DebuggerEvent>();
        let mut writer = GdbWriter {
            stdin: Vec::<u8>::new(),
            seq: 1,
        };
        let mut pending = PendingRegistry::default();
        pending
            .remote_connect
            .insert(7, "localhost:9999".into());

        let flow = handle_gdb_output(
            "7^error,msg=\"localhost:9999: Connection refused.\"",
            &mut writer,
            &event_tx,
            &mut pending,
        );

        assert_eq!(flow, ControlFlow::Continue(()));
        assert!(!pending.remote_connect.contains_key(&7));

        let first = event_rx.try_recv().expect("RemoteConnectFailed event");
        match first {
            DebuggerEvent::State(StateEvent::RemoteConnectFailed { target, message }) => {
                assert_eq!(target, "localhost:9999");
                assert_eq!(message, "localhost:9999: Connection refused.");
            }
            other => panic!("expected RemoteConnectFailed, got {other:?}"),
        }

        let second = event_rx
            .try_recv()
            .expect("GdbError event from parse_line fall-through");
        match second {
            DebuggerEvent::Ui(UiEvent::GdbError(msg)) => {
                assert_eq!(msg, "localhost:9999: Connection refused.");
            }
            other => panic!("expected GdbError, got {other:?}"),
        }
    }

    #[test]
    fn handle_gdb_output_remote_disconnect_done_emits_remote_disconnected_and_falls_through() {
        let (event_tx, event_rx) = std::sync::mpsc::channel::<DebuggerEvent>();
        let mut writer = GdbWriter {
            stdin: Vec::<u8>::new(),
            seq: 1,
        };
        let mut pending = PendingRegistry::default();
        pending.remote_disconnect.insert(8);

        let flow = handle_gdb_output("8^done", &mut writer, &event_tx, &mut pending);

        assert_eq!(flow, ControlFlow::Continue(()));
        assert!(!pending.remote_disconnect.contains(&8));

        let event = event_rx.try_recv().expect("RemoteDisconnected event");
        match event {
            DebuggerEvent::State(StateEvent::RemoteDisconnected { error }) => {
                assert_eq!(error, None)
            }
            other => panic!("expected RemoteDisconnected, got {other:?}"),
        }
    }

    // An untokened `^connected` (e.g. from `gdb-gui -ex "target
    // extended-remote …"`) has no `pending.remote_connect` entry to
    // correlate against, so it falls through harmlessly to parse_line's
    // `_ => None` (design D3 — no generic "connected" arm in
    // `parser.rs::parse_result`, verified). This documents a non-regression,
    // not a new gap.
    #[test]
    fn handle_gdb_output_untokened_connected_falls_through_harmlessly() {
        let (event_tx, event_rx) = std::sync::mpsc::channel::<DebuggerEvent>();
        let mut writer = GdbWriter {
            stdin: Vec::<u8>::new(),
            seq: 1,
        };
        let mut pending = PendingRegistry::default();

        let flow = handle_gdb_output("^connected", &mut writer, &event_tx, &mut pending);

        assert_eq!(flow, ControlFlow::Continue(()));
        assert!(
            event_rx.try_recv().is_err(),
            "an untokened ^connected must emit no event at all"
        );
    }

    // ─── spawn_reader_thread ────────────────────────────────────────────────

    #[test]
    fn spawn_reader_thread_lossily_decodes_invalid_utf8_and_keeps_running() {
        // Lone 0xFF is not valid UTF-8 on its own. Previously `read_line`
        // would error on this byte and the `Err` arm would kill the thread
        // permanently. The fix must decode it lossily (U+FFFD) and keep
        // reading subsequent lines instead of dying.
        let mut bytes: Vec<u8> = vec![0xFF, b'\n'];
        bytes.extend_from_slice(b"hello\n");
        let cursor = std::io::Cursor::new(bytes);
        let (event_tx, _event_rx) = std::sync::mpsc::channel::<DebuggerEvent>();

        let (handle, line_rx) = spawn_reader_thread(cursor, event_tx);

        let first = line_rx.recv().expect("invalid-UTF8 line should still be delivered");
        assert!(
            first.contains('\u{FFFD}'),
            "expected replacement character in lossily-decoded line, got {first:?}"
        );

        let second = line_rx.recv().expect("thread must keep running after invalid UTF-8");
        assert_eq!(second, "hello");

        handle.join().unwrap();
    }

    #[test]
    fn spawn_reader_thread_terminates_cleanly_on_eof() {
        let cursor = std::io::Cursor::new(Vec::<u8>::new());
        let (event_tx, _event_rx) = std::sync::mpsc::channel::<DebuggerEvent>();

        let (handle, line_rx) = spawn_reader_thread(cursor, event_tx);

        assert!(
            line_rx.recv().is_err(),
            "channel should be closed with no lines delivered on immediate EOF"
        );
        handle.join().unwrap();
    }

    #[test]
    fn spawn_reader_thread_skips_empty_lines() {
        let cursor = std::io::Cursor::new(b"\n\nfoo\n\n".to_vec());
        let (event_tx, _event_rx) = std::sync::mpsc::channel::<DebuggerEvent>();

        let (handle, line_rx) = spawn_reader_thread(cursor, event_tx);

        let line = line_rx.recv().expect("non-empty line should be delivered");
        assert_eq!(line, "foo");
        assert!(
            line_rx.recv().is_err(),
            "empty lines must be skipped, not forwarded"
        );
        handle.join().unwrap();
    }
}
