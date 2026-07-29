#[allow(unused_imports)]
use crate::state::{
    Breakpoint, Catchpoint, CatchpointKind, DebuggerEvent, Frame, PauseState, StateEvent,
    StopReason, SyscallPhase, ThreadInfo, UiEvent, Variable, Watchpoint, WatchpointKind,
};

pub fn parse_line(line: &str) -> Option<DebuggerEvent> {
    if line == "(gdb)" || line.is_empty() {
        return None;
    }

    let line = strip_token(line);

    match line.chars().next()? {
        '~' => parse_console_stream(line),
        '@' => parse_target_stream(line),
        '&' => None, // internal log, ignore
        '*' => parse_exec_async(line),
        '=' => parse_notify_async(line),
        '^' => parse_result(line),
        _ => None,
    }
}

fn strip_token(line: &str) -> &str {
    let end = line.find(|c: char| !c.is_ascii_digit()).unwrap_or(0);
    &line[end..]
}

// ─── Stream outputs ───────────────────────────────────────────────────────────

fn parse_console_stream(line: &str) -> Option<DebuggerEvent> {
    // ~"some text\n"
    let text = unquote(&line[1..])?;
    Some(DebuggerEvent::Ui(UiEvent::ConsoleOutput(text)))
}

fn parse_target_stream(line: &str) -> Option<DebuggerEvent> {
    // @"some text\n"  → stdout of the program being debugged
    let text = unquote(&line[1..])?;
    Some(DebuggerEvent::Ui(UiEvent::ConsoleOutput(format!(
        "[target] {text}"
    ))))
}

// ─── Exec async (*) ───────────────────────────────────────────────────────────

fn parse_exec_async(line: &str) -> Option<DebuggerEvent> {
    let rest = &line[1..]; // remove '*'
    let (class, fields) = split_class_fields(rest);

    match class {
        "running" => Some(DebuggerEvent::State(StateEvent::ProgramStarted)),

        "stopped" => {
            // The program may have exited: those *stopped records carry no frame,
            // so they must be handled before requiring one.
            match extract_str(fields, "reason").as_deref() {
                Some("exited-normally") => {
                    return Some(DebuggerEvent::State(StateEvent::ProgramExited {
                        code: Some(0),
                    }));
                }
                Some("exited") => {
                    let code = extract_str(fields, "exit-code")
                        .and_then(|s| i32::from_str_radix(&s, 8).ok());
                    return Some(DebuggerEvent::State(StateEvent::ProgramExited { code }));
                }
                Some("exited-signalled") => {
                    return Some(DebuggerEvent::State(StateEvent::ProgramExited {
                        code: None,
                    }));
                }
                _ => {}
            }

            let reason = parse_stop_reason(fields);

            // D5: `watchpoint-scope` may arrive without a `frame=` field (the
            // scope that triggered removal may have no active frame left).
            // A synthetic frame preserves the pause transition instead of
            // bailing on `?` — which would silently drop the stop and leave
            // a stale watchpoint row (auto-removal never runs). Scoped to
            // this one reason plus the five catchpoint stop reasons (D5,
            // extended for Phase 2a — a caught fork/vfork/exec/solib-event —
            // and Phase 2b's syscall-entry/syscall-return, which share one
            // `SyscallTriggered` variant) may equally arrive with no active
            // frame yet); every other stop still requires a real frame.
            let frame = match (&reason, parse_frame_field(fields)) {
                (_, Some(f)) => f,
                (
                    StopReason::WatchpointScope(_)
                    | StopReason::Forked { .. }
                    | StopReason::Vforked { .. }
                    | StopReason::Execd { .. }
                    | StopReason::SolibEvent { .. }
                    | StopReason::SyscallTriggered { .. },
                    None,
                ) => Frame {
                    addr: 0,
                    function: "??".into(),
                    file: None,
                    line: None,
                },
                (_, None) => return None,
            };
            let stack = vec![frame.clone()];
            let thread_id = extract_str(fields, "thread-id")
                .and_then(|s| s.parse().ok())
                .unwrap_or(1);

            Some(DebuggerEvent::State(StateEvent::ProgramPaused {
                pause: PauseState {
                    thread_id,
                    frame,
                    stack,
                    stop_reason: reason,
                },
            }))
        }

        _ => None,
    }
}

fn parse_stop_reason(fields: &str) -> StopReason {
    match extract_str(fields, "reason").as_deref() {
        Some("breakpoint-hit") => {
            let id = extract_str(fields, "bkptno")
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            StopReason::BreakpointHit(id)
        }
        Some("end-stepping-range") | Some("step-over-range") => StopReason::EndStepping,
        Some("signal-received") => {
            let sig = extract_str(fields, "signal-name").unwrap_or_default();
            StopReason::Signal(sig)
        }
        Some("watchpoint-trigger") => {
            // The id and expr live inside the `wpt={number="...",exp="..."}`
            // block, not as bare fields.
            let wpt_block = extract_block(fields, "wpt");
            let id = wpt_block
                .and_then(|b| extract_str(b, "number"))
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            let expr = wpt_block
                .and_then(|b| extract_str(b, "exp"))
                .unwrap_or_default();
            // old/new are blocks (`old={value="..."}`), not bare string
            // fields: a plain extract_str(fields, "value") would match
            // whichever of `old`/`new` occurs first for both.
            let old = extract_block(fields, "old")
                .and_then(|b| extract_str(b, "value"))
                .unwrap_or_default();
            let new = extract_block(fields, "new")
                .and_then(|b| extract_str(b, "value"))
                .unwrap_or_default();
            StopReason::WatchpointTriggered { id, expr, old, new }
        }
        Some("watchpoint-scope") => {
            let id = extract_str(fields, "wpnum")
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            StopReason::WatchpointScope(id)
        }
        // D4: catchpoint stop reasons, mapped 1:1 to GDB's literal reason
        // strings (verified live against GDB 17.2 for fork/solib-event —
        // apply-blocker #68).
        Some("fork") => {
            let newpid = extract_str(fields, "newpid").and_then(|s| s.parse().ok());
            StopReason::Forked { newpid }
        }
        Some("vforked") => {
            let newpid = extract_str(fields, "newpid").and_then(|s| s.parse().ok());
            StopReason::Vforked { newpid }
        }
        Some("exec") => {
            let path = extract_str(fields, "new-exec-path");
            StopReason::Execd { path }
        }
        Some("solib-event") => {
            // Verified live shape (apply-blocker #68):
            // `added=[library="..."]`. `extract_str` scans the whole
            // fields string for the first `library="..."` occurrence
            // regardless of which list it is nested in (`added=[...]` for
            // load, presumably `removed=[...]` for unload), so this stays
            // correct for both without needing two separate extractions.
            let library = extract_str(fields, "library");
            StopReason::SolibEvent { library }
        }
        // D3: `catch syscall` stops on both entry and return by default —
        // mapping only entry would drop every second stop into `Unknown`
        // with no banner. `syscall-number`/`syscall-name` are each
        // independently optional; neither present must not panic.
        Some("syscall-entry") => {
            let number = extract_str(fields, "syscall-number");
            let name = extract_str(fields, "syscall-name");
            StopReason::SyscallTriggered {
                phase: SyscallPhase::Entry,
                number,
                name,
            }
        }
        Some("syscall-return") => {
            let number = extract_str(fields, "syscall-number");
            let name = extract_str(fields, "syscall-name");
            StopReason::SyscallTriggered {
                phase: SyscallPhase::Return,
                number,
                name,
            }
        }
        _ => StopReason::Unknown,
    }
}

// ─── Notify async (=) ─────────────────────────────────────────────────────────

fn parse_notify_async(line: &str) -> Option<DebuggerEvent> {
    let rest = &line[1..];
    let (class, fields) = split_class_fields(rest);

    match class {
        "breakpoint-created" | "breakpoint-modified" => {
            // A6 (design addendum): checked BEFORE parse_breakpoint_field —
            // most-specific-first, same discipline as D3. This is the ONLY
            // ingress path for 4 of 6 catchpoint kinds (Fork/Vfork/Exec/
            // Signal): their creation is untokened async, verified live
            // against GDB 17.2 (apply-blocker #68). `breakpoint-modified`
            // for a catchpoint re-emits CatchpointAdded — idempotent
            // replace-by-id in `DebuggerState::apply`.
            if fields.contains("catch-type=") {
                let block = extract_block(fields, "bkpt")?;
                let cp = parse_catchpoint_field(block)?;
                return Some(DebuggerEvent::State(StateEvent::CatchpointAdded {
                    catchpoint: cp,
                }));
            }
            let bp = parse_breakpoint_field(fields, "bkpt")?;
            Some(DebuggerEvent::State(StateEvent::BreakpointAdded {
                breakpoint: bp,
            }))
        }
        "breakpoint-deleted" => {
            let id = extract_str(fields, "id").and_then(|s| s.parse().ok())?;
            Some(DebuggerEvent::State(StateEvent::BreakpointRemoved { id }))
        }
        _ => None,
    }
}

// ─── Result (^) ───────────────────────────────────────────────────────────────

fn parse_result(line: &str) -> Option<DebuggerEvent> {
    let rest = &line[1..];
    let (class, fields) = split_class_fields(rest);

    match class {
        "error" => {
            let msg = extract_str(&fields, "msg").unwrap_or_else(|| "GDB error".into());
            Some(DebuggerEvent::Ui(UiEvent::GdbError(msg)))
        }

        "done" => {
            // -catch-load/-catch-unload (with an arg) → ^done,bkpt={...,
            // catch-type="load"|"unload",...} — the only two kinds whose
            // creation reply is tokened/native (design addendum A1/A6).
            // Checked BEFORE the plain bkpt= branch below (D3/A6
            // most-specific-first discipline): catch-type= never appears on
            // a real breakpoint reply.
            if fields.contains("catch-type=") {
                if let Some(block) = extract_block(&fields, "bkpt") {
                    if let Some(cp) = parse_catchpoint_field(block) {
                        return Some(DebuggerEvent::State(StateEvent::CatchpointAdded {
                            catchpoint: cp,
                        }));
                    }
                }
            }

            // -break-insert → ^done,bkpt={...}
            if fields.contains("bkpt=") {
                if let Some(bp) = parse_breakpoint_field(&fields, "bkpt") {
                    return Some(DebuggerEvent::State(StateEvent::BreakpointAdded {
                        breakpoint: bp,
                    }));
                }
            }

            // -break-watch → ^done,wpt={number=...,exp=...} (Write, default)
            //              → ^done,hw-rwpt={...} (Read, `-r`)
            //              → ^done,hw-awpt={...} (Access, `-a`)
            // Checked most-specific-key-first: "hw-rwpt="/"hw-awpt=" both
            // contain "wpt=" as a substring, so a plain `extract_block(_,
            // "wpt")` would also match inside them if tried first.
            if fields.contains("wpt=") {
                let parsed = parse_watchpoint_field(&fields, "hw-rwpt", WatchpointKind::Read)
                    .or_else(|| parse_watchpoint_field(&fields, "hw-awpt", WatchpointKind::Access))
                    .or_else(|| parse_watchpoint_field(&fields, "wpt", WatchpointKind::Write));
                if let Some(watchpoint) = parsed {
                    return Some(DebuggerEvent::State(StateEvent::WatchpointAdded {
                        watchpoint,
                    }));
                }
            }

            // -stack-list-frames → ^done,stack=[frame={...},frame={...}]
            if fields.contains("stack=") {
                let frames = parse_stack(fields);
                return Some(DebuggerEvent::State(StateEvent::StackUpdated { frames }));
            }

            // -stack-list-variables → ^done,variables=[...]
            if fields.contains("variables=") {
                let vars = parse_variables(fields);
                return Some(DebuggerEvent::State(StateEvent::LocalsUpdated { vars }));
            }

            // -data-list-register-names → ^done,register-names=["rax","rbx",...]
            if fields.contains("register-names=") {
                let names = parse_register_names(fields);
                return Some(DebuggerEvent::State(StateEvent::RegisterNamesReceived {
                    names,
                }));
            }

            // -data-list-register-values → ^done,register-values=[{number="0",value="0x..."}...]
            if fields.contains("register-values=") {
                let regs = parse_registers(fields);
                return Some(DebuggerEvent::State(StateEvent::RegistersUpdated {
                    registers: regs,
                }));
            }

            // -data-disassemble → ^done,asm_insns=[{address="0x...",inst="..."}...]
            if fields.contains("asm_insns=") {
                let lines = parse_disasm(fields);
                return Some(DebuggerEvent::State(StateEvent::DisasmUpdated { lines }));
            }

            // -symbol-info-variables → ^done,symbols={debug=[{filename="...",symbols=[...]}]}
            if fields.contains("symbols=") && fields.contains("debug=") {
                let names = parse_global_names(fields);
                return Some(DebuggerEvent::State(StateEvent::GlobalNamesReceived {
                    names,
                }));
            }

            // -thread-info → ^done,threads=[{id="1",...},...],current-thread-id="N"
            if fields.contains("threads=[") {
                let threads = parse_threads(fields);
                let current =
                    extract_field_str(fields, "current-thread-id").and_then(|s| s.parse().ok());
                return Some(DebuggerEvent::State(StateEvent::ThreadsUpdated {
                    threads,
                    current,
                }));
            }

            // -thread-select → ^done,new-thread-id="N",frame={...}. The
            // frame is deliberately ignored (design.md: "Post-switch frame").
            if fields.contains("new-thread-id=")
                && let Some(id) =
                    extract_field_str(fields, "new-thread-id").and_then(|s| s.parse().ok())
            {
                return Some(DebuggerEvent::State(StateEvent::ThreadSelected { id }));
            }

            None
        }

        "running" => Some(DebuggerEvent::State(StateEvent::ProgramStarted)),

        "exit" => Some(DebuggerEvent::State(StateEvent::ProgramExited {
            code: None,
        })),

        _ => None,
    }
}

pub fn extract_str(fields: &str, key: &str) -> Option<String> {
    let needle = format!("{key}=\"");
    let start = fields.find(&needle)? + needle.len();
    let rest = &fields[start..];
    let end = find_closing_quote(rest)?;
    Some(unescape(&rest[..end]))
}

/// Like `extract_str`, but boundary-anchored: only matches `key="` at index 0
/// or immediately after `,` / `{`. This avoids `extract_str`'s substring
/// collision — e.g. `extract_str(fields, "id")` would match inside
/// `target-id="..."` if `id="..."` never occurs on its own. Needed for keys
/// (like `id`) that are a suffix of another key present in the same block.
pub fn extract_field_str(fields: &str, key: &str) -> Option<String> {
    let needle = format!("{key}=\"");
    let mut search_from = 0usize;

    while search_from <= fields.len() {
        let idx = fields[search_from..].find(&needle)? + search_from;
        let boundary_ok = idx == 0 || matches!(fields.as_bytes()[idx - 1], b',' | b'{');
        if boundary_ok {
            let start = idx + needle.len();
            let rest = &fields[start..];
            let end = find_closing_quote(rest)?;
            return Some(unescape(&rest[..end]));
        }
        search_from = idx + 1;
    }

    None
}

fn extract_block<'a>(fields: &'a str, key: &str) -> Option<&'a str> {
    let needle = format!("{key}={{");
    let start = fields.find(&needle)? + needle.len();
    let rest = &fields[start..];
    let end = find_closing_brace(rest)?;
    Some(&rest[..end])
}

fn extract_list<'a>(fields: &'a str, key: &str) -> Option<&'a str> {
    let needle_bracket = format!("{key}=[");
    if let Some(start) = fields.find(&needle_bracket) {
        let rest = &fields[start + needle_bracket.len()..];
        if let Some(end) = find_closing_bracket(rest) {
            return Some(&rest[..end]);
        }
    }

    let needle_brace = format!("{key}={{");
    if let Some(start) = fields.find(&needle_brace) {
        let rest = &fields[start + needle_brace.len()..];
        if let Some(end) = find_closing_brace(rest) {
            return Some(&rest[..end]);
        }
    }

    None
}

fn parse_frame_field(fields: &str) -> Option<Frame> {
    let block = extract_block(fields, "frame")?;
    parse_frame(block)
}

fn parse_frame(block: &str) -> Option<Frame> {
    let addr = extract_str(block, "addr")
        .and_then(|s| u64::from_str_radix(s.trim_start_matches("0x"), 16).ok())
        .unwrap_or(0);
    let function = extract_str(block, "func").unwrap_or_else(|| "??".into());
    let file = extract_str(block, "fullname").or_else(|| extract_str(block, "file"));
    let line = extract_str(block, "line").and_then(|s| s.parse().ok());

    Some(Frame {
        addr,
        function,
        file,
        line,
    })
}

fn parse_stack(fields: &str) -> Vec<Frame> {
    let list = match extract_list(fields, "stack") {
        Some(l) => l,
        None => return vec![],
    };

    let mut frames = vec![];
    let mut rest = list;

    while let Some(start) = rest.find('{') {
        rest = &rest[start + 1..];
        if let Some(end) = find_closing_brace(rest) {
            if let Some(frame) = parse_frame(&rest[..end]) {
                frames.push(frame);
            }
            rest = &rest[end + 1..];
        } else {
            break;
        }
    }

    frames
}

// ─── Threads ──────────────────────────────────────────────────────────────────

fn parse_thread(block: &str) -> Option<ThreadInfo> {
    // `id` collides as a substring of `target-id=`, so it needs the
    // boundary-anchored extractor; `target-id` and `state` are unambiguous.
    let id = extract_field_str(block, "id").and_then(|s| s.parse().ok())?;
    let target_id = extract_str(block, "target-id").unwrap_or_default();
    let state = extract_str(block, "state").unwrap_or_default();
    let frame = extract_block(block, "frame").and_then(parse_frame);

    Some(ThreadInfo {
        id,
        target_id,
        state,
        frame,
    })
}

fn parse_threads(fields: &str) -> Vec<ThreadInfo> {
    let list = match extract_list(fields, "threads") {
        Some(l) => l,
        None => return vec![],
    };

    let mut threads = vec![];
    let mut rest = list;

    while let Some(start) = rest.find('{') {
        rest = &rest[start + 1..];
        if let Some(end) = find_closing_brace(rest) {
            if let Some(thread) = parse_thread(&rest[..end]) {
                threads.push(thread);
            }
            rest = &rest[end + 1..];
        } else {
            break;
        }
    }

    threads
}

pub(crate) fn parse_breakpoint_field(fields: &str, key: &str) -> Option<Breakpoint> {
    let block = extract_block(fields, key)?;

    let id = extract_str(block, "number")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let file = extract_str(block, "fullname").or_else(|| extract_str(&block, "file"))?;
    let line = extract_str(block, "line")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let enabled = extract_str(block, "enabled")
        .map(|s| s == "y")
        .unwrap_or(true);

    // GDB relocates breakpoints requested on a function name's line to the
    // first executable line. `original-location` preserves what was requested
    // (e.g. "example.c:6"); we store its line number so toggling works from
    // the line the user actually clicked.
    let requested_line = extract_str(block, "original-location")
        .and_then(|loc| loc.rsplit_once(':').map(|(_, n)| n.to_owned()))
        .and_then(|n| n.parse().ok());

    // `cond=` is absent when the breakpoint has no condition: extract_str
    // returns None in that case (never Some("")), preserving the distinction.
    let condition = extract_str(block, "cond");

    Some(Breakpoint {
        id,
        file,
        line,
        requested_line,
        enabled,
        condition,
        condition_error: None,
    })
}

/// Parses a `<key>={number="...",exp="..."}` block into a `Watchpoint` of
/// the given `kind`. Used for `wpt=` (Write), `hw-rwpt=` (Read), and
/// `hw-awpt=` (Access) — GDB's `-break-watch` reply is self-describing, so
/// no token correlation is needed for a successful creation.
pub(crate) fn parse_watchpoint_field(
    fields: &str,
    key: &str,
    kind: WatchpointKind,
) -> Option<Watchpoint> {
    let block = extract_block(fields, key)?;
    let id = extract_str(block, "number")?.parse().ok()?;
    let expr = extract_str(block, "exp")?;

    Some(Watchpoint {
        id,
        expr,
        kind,
        enabled: true,
    })
}

/// Parses a `catch-type=...` block (the content of a `bkpt={...}` catchpoint
/// record, already unwrapped by the caller) into a `Catchpoint`. Shared by
/// BOTH ingress paths — `parse_notify_async`'s untokened
/// `breakpoint-created`/`-modified` (all 7 kinds) and `parse_result`'s
/// tokened `done` arm (Load/Unload only) — so a duplicate record on either
/// path produces a byte-identical `Catchpoint`, making `apply`'s
/// replace-by-id idempotent regardless of which path wins the race (design
/// addendum: "one shared `parse_catchpoint_field` serves both").
///
/// Requires `number=` and a recognized `catch-type=`; returns `None` for an
/// unknown `catch-type` (Phase 2c kinds: throw/catch/rethrow) so no
/// unsupported record is silently turned into a wrong-shaped row.
pub(crate) fn parse_catchpoint_field(block: &str) -> Option<Catchpoint> {
    let catch_type = extract_str(block, "catch-type")?;
    let kind = parse_catchpoint_kind(&catch_type)?;
    let id = extract_str(block, "number")?.parse().ok()?;
    let enabled = extract_str(block, "enabled")
        .map(|s| s == "y")
        .unwrap_or(true);

    // Phase 2c (design deviation from D4/D5 — see apply-progress): the
    // Phase 0 live spike (GDB 17.2) found `-catch-throw/-rethrow/-catch`
    // carry their optional `-r <regexp>` filter in a DEDICATED `regexp=`
    // field on both the tokened `^done` reply and every `=breakpoint-*`
    // notify — present only when a filter was given, absent (not empty)
    // otherwise. `what=` stays the fixed "exception throw"/"rethrow"/
    // "catch" prose regardless, so it is never consulted for these 3
    // kinds. This makes the regexp self-describing on every ingress path,
    // exactly like every other catchpoint kind — no process.rs echo/
    // correlation mechanism is needed.
    let args = match kind {
        CatchpointKind::Throw | CatchpointKind::Rethrow | CatchpointKind::Catch => {
            extract_str(block, "regexp").map(|r| vec![r]).unwrap_or_default()
        }
        _ => {
            let what = extract_str(block, "what");
            catchpoint_args_from_what(kind, what.as_deref())
        }
    };

    Some(Catchpoint {
        id,
        kind,
        args,
        enabled,
    })
}

fn parse_catchpoint_kind(catch_type: &str) -> Option<CatchpointKind> {
    match catch_type {
        "fork" => Some(CatchpointKind::Fork),
        "vfork" => Some(CatchpointKind::Vfork),
        "exec" => Some(CatchpointKind::Exec),
        "signal" => Some(CatchpointKind::Signal),
        "load" => Some(CatchpointKind::Load),
        "unload" => Some(CatchpointKind::Unload),
        "syscall" => Some(CatchpointKind::Syscall),
        "throw" => Some(CatchpointKind::Throw),
        "rethrow" => Some(CatchpointKind::Rethrow),
        "catch" => Some(CatchpointKind::Catch),
        _ => None,
    }
}

/// A4 (design addendum): recovers `args` from the `what=` prose field, which
/// is the only place GDB 17.2 puts them (verified live — apply-blocker #68;
/// no clean `regexp=` field exists as design #65 originally assumed).
///
/// Fork/Vfork/Exec never carry args (verified: `what=` is absent for them).
/// Signal's `what=` is a bare, clean signal name (e.g. `"SIGINT"`) — used
/// as-is. Load/Unload's `what=` is prose: `"load of library matching X"` →
/// `["X"]` via `rsplit_once(" matching ")`; the bare `"load of library"` /
/// `"unload of library"` (no-arg console form) → `[]`; anything else (a
/// future GDB version's differently worded prose) falls back to
/// `[raw what text]` — wrong-looking in the UI at worst, never a panic.
fn catchpoint_args_from_what(kind: CatchpointKind, what: Option<&str>) -> Vec<String> {
    match kind {
        CatchpointKind::Fork | CatchpointKind::Vfork | CatchpointKind::Exec => vec![],
        CatchpointKind::Signal => what.map(|w| vec![w.to_string()]).unwrap_or_default(),
        CatchpointKind::Load | CatchpointKind::Unload => match what {
            None => vec![],
            Some(w) => {
                if let Some((_, matched)) = w.rsplit_once(" matching ") {
                    vec![matched.to_string()]
                } else if w == "load of library" || w == "unload of library" {
                    vec![]
                } else {
                    vec![w.to_string()]
                }
            }
        },
        // D4: `what=` normally carries the raw name/number/`group:foo`
        // token verbatim (spec: direct passthrough, no GDB prose to strip).
        // Defensive fallback for the unverified `print_one_catch_syscall`
        // prose shape (`syscall "x"` / `syscalls "x y"`) — stripped the same
        // way Load/Unload's `" matching "` prose is, so a live mismatch
        // degrades to the raw text rather than ever panicking.
        CatchpointKind::Syscall => match what {
            None => vec![],
            Some(w) if w.is_empty() => vec![],
            Some("syscall") | Some("syscalls") => vec![],
            Some(w) => {
                let prose = w
                    .strip_prefix("syscalls ")
                    .or_else(|| w.strip_prefix("syscall "));
                match prose {
                    Some(quoted) => quoted
                        .trim_matches('"')
                        .split_whitespace()
                        .map(|s| s.to_string())
                        .collect(),
                    None => vec![w.to_string()],
                }
            }
        },
        // Phase 2c: never reached from `parse_catchpoint_field`, which
        // extracts the dedicated `regexp=` field for these 3 kinds instead
        // (see its doc comment) — `what=` is fixed prose here regardless of
        // filter. Kept exhaustive and documented for any other caller.
        CatchpointKind::Throw | CatchpointKind::Rethrow | CatchpointKind::Catch => vec![],
    }
}

/// Extracts the leading numeric token from a raw MI line (e.g. `"12^error,..."`
/// → `Some(12)`). `None` if the line does not start with digits (async/stream
/// records with no token, like `*stopped` or `^error` with no prefix).
pub fn parse_token(line: &str) -> Option<u32> {
    let end = line.find(|c: char| !c.is_ascii_digit()).unwrap_or(0);
    if end == 0 {
        None
    } else {
        line[..end].parse().ok()
    }
}

fn parse_variables(fields: &str) -> Vec<Variable> {
    let list = match extract_list(fields, "variables") {
        Some(l) => l,
        None => return vec![],
    };

    let mut vars = vec![];

    if list.contains('{') {
        let mut rest = list;
        while let Some(start) = rest.find('{') {
            rest = &rest[start + 1..];
            if let Some(end) = find_closing_brace(rest) {
                let block = &rest[..end];
                if let Some(var) = parse_single_variable(block) {
                    vars.push(var);
                }
                rest = &rest[end + 1..];
            } else {
                break;
            }
        }
    } else {
        if let Some(var) = parse_single_variable(list) {
            vars.push(var);
        }
    }

    vars
}

fn parse_single_variable(block: &str) -> Option<Variable> {
    let name = extract_str(block, "name")?;
    let value = extract_str(block, "value").unwrap_or_default();
    let type_ = extract_str(block, "type").unwrap_or_default();

    if name.is_empty() {
        return None;
    }

    Some(Variable { name, value, type_ })
}

// ─── Global variable names ────────────────────────────────────────────────────

fn parse_global_names(fields: &str) -> Vec<String> {
    let symbols_block = match extract_block(fields, "symbols") {
        Some(b) => b,
        None => return vec![],
    };
    let debug_list = match extract_list(symbols_block, "debug") {
        Some(l) => l,
        None => return vec![],
    };

    let mut names = vec![];
    let mut rest = debug_list;

    while let Some(start) = rest.find('{') {
        rest = &rest[start + 1..];
        let end = match find_closing_brace(rest) {
            Some(e) => e,
            None => break,
        };
        let file_block = &rest[..end];

        if let Some(sym_list) = extract_list(file_block, "symbols") {
            let mut inner = sym_list;
            while let Some(s2) = inner.find('{') {
                inner = &inner[s2 + 1..];
                let e2 = match find_closing_brace(inner) {
                    Some(e) => e,
                    None => break,
                };
                let sym_block = &inner[..e2];
                if let Some(name) = extract_str(sym_block, "name") {
                    if !names.contains(&name) {
                        names.push(name);
                    }
                }
                inner = &inner[e2 + 1..];
            }
        }

        rest = &rest[end + 1..];
    }

    names
}

// ─── String utilities ─────────────────────────────────────────────────────

fn split_class_fields(s: &str) -> (&str, &str) {
    match s.find(',') {
        Some(i) => (&s[..i], &s[i + 1..]),
        None => (s, ""),
    }
}

fn unquote(s: &str) -> Option<String> {
    let s = s.trim();
    if s.starts_with('"') && s.ends_with('"') && s.len() >= 2 {
        Some(unescape(&s[1..s.len() - 1]))
    } else if s.starts_with('"') {
        Some(unescape(&s[1..]))
    } else {
        Some(s.to_owned())
    }
}

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('"') => out.push('"'),
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('\\') => out.push('\\'),
                Some(x) => {
                    out.push('\\');
                    out.push(x);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn find_closing_quote(s: &str) -> Option<usize> {
    let mut escaped = false;
    for (i, c) in s.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if c == '\\' {
            escaped = true;
            continue;
        }
        if c == '"' {
            return Some(i);
        }
    }
    None
}

fn find_closing_brace(s: &str) -> Option<usize> {
    find_closing(s, '{', '}')
}

fn find_closing_bracket(s: &str) -> Option<usize> {
    find_closing(s, '[', ']')
}

fn find_closing(s: &str, open: char, close: char) -> Option<usize> {
    let mut depth = 1usize;
    let mut in_str = false;
    let mut escaped = false;

    for (i, c) in s.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if c == '\\' {
            escaped = true;
            continue;
        }
        if c == '"' {
            in_str = !in_str;
            continue;
        }
        if in_str {
            continue;
        }
        if c == open {
            depth += 1;
        } else if c == close {
            depth -= 1;
            if depth == 0 {
                return Some(i);
            }
        }
    }
    None
}

// ─── Register names ─────────────────────────────────────────────────────────

fn parse_register_names(fields: &str) -> Vec<String> {
    let list = match extract_list(fields, "register-names") {
        Some(l) => l,
        None => return vec![],
    };

    // The list is: "rax","rbx","rcx",... (comma-separated strings)
    let mut names = vec![];
    let mut rest = list;

    while let Some(q) = rest.find('"') {
        rest = &rest[q + 1..];
        if let Some(end) = find_closing_quote(rest) {
            names.push(rest[..end].to_owned());
            rest = &rest[end + 1..];
        } else {
            break;
        }
    }

    names
}

// ─── Registers ───────────────────────────────────────────────────────────────

fn parse_registers(fields: &str) -> Vec<crate::state::Register> {
    let list = match extract_list(fields, "register-values") {
        Some(l) => l,
        None => return vec![],
    };

    let mut regs = vec![];
    let mut rest = list;

    while let Some(start) = rest.find('{') {
        rest = &rest[start + 1..];
        if let Some(end) = find_closing_brace(rest) {
            let block = &rest[..end];
            let number = extract_str(block, "number")
                .and_then(|s| s.parse().ok())
                .unwrap_or(0u32);
            let value = extract_str(block, "value").unwrap_or_default();
            // The name is cross-referenced in DebuggerState::apply using register_names[number]
            // We leave it empty here; the UI reads state.register_names for display.
            regs.push(crate::state::Register {
                number,
                name: String::new(),
                value,
            });
            rest = &rest[end + 1..];
        } else {
            break;
        }
    }

    regs
}

// ─── Disassembly ─────────────────────────────────────────────────────────────

fn parse_disasm(fields: &str) -> Vec<crate::state::AsmLine> {
    let list = match extract_list(fields, "asm_insns") {
        Some(l) => l,
        None => return vec![],
    };

    let mut lines = vec![];
    let mut rest = list;

    while let Some(start) = rest.find('{') {
        rest = &rest[start + 1..];
        if let Some(end) = find_closing_brace(rest) {
            let block = &rest[..end];
            let addr = extract_str(block, "address")
                .and_then(|s| u64::from_str_radix(s.trim_start_matches("0x"), 16).ok())
                .unwrap_or(0);
            let inst = extract_str(block, "inst").unwrap_or_default();
            lines.push(crate::state::AsmLine { addr, inst });
            rest = &rest[end + 1..];
        } else {
            break;
        }
    }

    lines
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_token() {
        assert_eq!(strip_token("42^done"), "^done");
        assert_eq!(strip_token("^done"), "^done");
        assert_eq!(
            strip_token("*stopped,reason=\"end-stepping-range\""),
            "*stopped,reason=\"end-stepping-range\""
        );
    }

    #[test]
    fn test_extract_str() {
        let s = r#"number="1",file="main.c",line="42",enabled="y""#;
        assert_eq!(extract_str(s, "number"), Some("1".into()));
        assert_eq!(extract_str(s, "file"), Some("main.c".into()));
        assert_eq!(extract_str(s, "line"), Some("42".into()));
        assert_eq!(extract_str(s, "missing"), None);
    }

    #[test]
    fn test_parse_running() {
        let event = parse_line("*running,thread-id=\"all\"");
        assert!(matches!(
            event,
            Some(DebuggerEvent::State(StateEvent::ProgramStarted))
        ));
    }

    #[test]
    fn test_parse_error() {
        let event = parse_line("^error,msg=\"No symbol table\"");
        assert!(matches!(
            event,
            Some(DebuggerEvent::Ui(UiEvent::GdbError(_)))
        ));
    }

    #[test]
    fn test_console_stream() {
        let event = parse_line("~\"Breakpoint 1 at 0x1234\\n\"");
        assert!(matches!(
            event,
            Some(DebuggerEvent::Ui(UiEvent::ConsoleOutput(_)))
        ));
    }

    #[test]
    fn test_ignore_prompt() {
        assert!(parse_line("(gdb)").is_none());
        assert!(parse_line("").is_none());
    }

    #[test]
    fn test_parse_global_names() {
        // Real capture of `-symbol-info-variables` against GDB 17.2.
        let line = r#"4^done,symbols={debug=[{filename="globaltest.c",fullname="/tmp/globaltest.c",symbols=[{line="3",name="global_counter",type="int",description="int global_counter;"},{line="4",name="static_thing",type="int",description="static int static_thing;"}]}]}"#;
        let event = parse_line(line);
        match event {
            Some(DebuggerEvent::State(StateEvent::GlobalNamesReceived { names })) => {
                assert_eq!(
                    names,
                    vec!["global_counter".to_string(), "static_thing".to_string()]
                );
            }
            other => panic!("expected GlobalNamesReceived, got {other:?}"),
        }
    }

    #[test]
    fn test_program_exit_normally() {
        // *stopped for program termination carries no frame; it used to be discarded.
        let event = parse_line(r#"*stopped,reason="exited-normally""#);
        assert!(matches!(
            event,
            Some(DebuggerEvent::State(StateEvent::ProgramExited {
                code: Some(0)
            }))
        ));
    }

    #[test]
    fn test_program_exit_with_code() {
        // exit-code comes in octal in the MI output.
        let event = parse_line(r#"*stopped,reason="exited",exit-code="02""#);
        assert!(matches!(
            event,
            Some(DebuggerEvent::State(StateEvent::ProgramExited {
                code: Some(2)
            }))
        ));
    }

    #[test]
    fn test_parse_stack() {
        let line = r#"6^done,stack=[frame={level="0",addr="0x1149",func="foo",file="a.c",fullname="/a.c",line="3"},frame={level="1",addr="0x1170",func="main",file="a.c",fullname="/a.c",line="9"}]"#;
        match parse_line(line) {
            Some(DebuggerEvent::State(StateEvent::StackUpdated { frames })) => {
                assert_eq!(frames.len(), 2);
                assert_eq!(frames[0].function, "foo");
                assert_eq!(frames[0].line, Some(3));
                assert_eq!(frames[1].function, "main");
            }
            other => panic!("expected StackUpdated, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_breakpoint_cond_present() {
        let line = r#"=breakpoint-modified,bkpt={number="1",type="breakpoint",disp="keep",enabled="y",addr="0x1149",func="main",file="a.c",fullname="/a.c",line="10",cond="i == 10",times="0",original-location="a.c:10"}"#;
        match parse_line(line) {
            Some(DebuggerEvent::State(StateEvent::BreakpointAdded { breakpoint })) => {
                assert_eq!(breakpoint.condition, Some("i == 10".to_string()));
            }
            other => panic!("expected BreakpointAdded, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_breakpoint_cond_absent() {
        let line = r#"=breakpoint-modified,bkpt={number="1",type="breakpoint",disp="keep",enabled="y",addr="0x1149",func="main",file="a.c",fullname="/a.c",line="10",times="0",original-location="a.c:10"}"#;
        match parse_line(line) {
            Some(DebuggerEvent::State(StateEvent::BreakpointAdded { breakpoint })) => {
                assert_eq!(breakpoint.condition, None);
            }
            other => panic!("expected BreakpointAdded, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_token_present() {
        assert_eq!(parse_token("12^error,msg=\"bad\""), Some(12));
    }

    #[test]
    fn test_parse_token_absent() {
        assert_eq!(parse_token("^error,msg=\"bad\""), None);
        assert_eq!(parse_token("*stopped"), None);
    }

    #[test]
    fn test_bare_value_done_not_confused_with_globals() {
        // -data-evaluate-expression → ^done,value="42" must not match the globals branch.
        let line = r#"5^done,value="42""#;
        let event = parse_line(line);
        assert!(event.is_none());
    }

    // ─── Thread list parsing ────────────────────────────────────────────────

    #[test]
    fn test_extract_field_str_rejects_substring_match() {
        // "id=\"" is a substring of "target-id=\"" — extract_field_str must not
        // match inside it (boundary-anchored: only at index 0 or right after
        // ',' / '{').
        let fields = r#"target-id="Thread 2","#;
        assert_eq!(extract_field_str(fields, "id"), None);

        // Leading match (index 0).
        let leading = r#"id="1",target-id="Thread 2""#;
        assert_eq!(extract_field_str(leading, "id"), Some("1".into()));

        // Match right after a comma.
        let after_comma = r#"target-id="Thread 2",id="3""#;
        assert_eq!(extract_field_str(after_comma, "id"), Some("3".into()));

        // Match right after an opening brace.
        let after_brace = r#"{id="5",func="main"}"#;
        assert_eq!(extract_field_str(after_brace, "id"), Some("5".into()));
    }

    #[test]
    fn test_parse_threads_happy_path() {
        // Real -thread-info capture shape (GDB MI docs example).
        let line = r#"^done,threads=[{id="2",target-id="Thread 0xb7e14b90 (LWP 21257)",frame={level="0",addr="0x0804891f",func="foo",args=[{name="i",value="10"}],file="test.c",fullname="/home/foo/bar/test.c",line="158"},state="stopped"},{id="1",target-id="process 21257",frame={level="0",addr="0x0804891f",func="foo",args=[{name="i",value="10"}],file="test.c",fullname="/home/foo/bar/test.c",line="158"},state="stopped"}],current-thread-id="1""#;

        let rest = &line[1..];
        let (class, fields) = split_class_fields(rest);
        assert_eq!(class, "done");

        let threads = parse_threads(fields);
        assert_eq!(threads.len(), 2);
        assert_eq!(threads[0].id, 2);
        assert_eq!(threads[0].target_id, "Thread 0xb7e14b90 (LWP 21257)");
        assert_eq!(threads[0].state, "stopped");
        assert!(threads[0].frame.is_some());
        assert_eq!(threads[0].frame.as_ref().unwrap().function, "foo");
        assert_eq!(threads[1].id, 1);
        assert_eq!(threads[1].target_id, "process 21257");

        assert_eq!(
            extract_field_str(fields, "current-thread-id"),
            Some("1".into())
        );
    }

    #[test]
    fn test_parse_threads_single_thread() {
        let line = r#"^done,threads=[{id="1",target-id="Thread 0x7ffff7fc2740 (LWP 12345)",frame={level="0",addr="0x0000555555555185",func="main",args=[],file="a.c",fullname="/tmp/a.c",line="10"},state="stopped"}],current-thread-id="1""#;
        let rest = &line[1..];
        let (_, fields) = split_class_fields(rest);
        let threads = parse_threads(fields);
        assert_eq!(threads.len(), 1);
        assert_eq!(threads[0].id, 1);
    }

    // Threat-matrix RED: target-id containing braces, an escaped quote, and
    // commas must not confuse the brace/comma-based scanning (mirrors
    // parse_stack's in_str handling in find_closing).
    #[test]
    fn test_parse_threads_robustness_braces_quotes_commas() {
        let line = r#"^done,threads=[{id="1",target-id="Thread \"weird, {name}\"",frame={level="0",addr="0x1",func="main",args=[],file="a.c",fullname="/tmp/a.c",line="1"},state="stopped"}],current-thread-id="1""#;
        let rest = &line[1..];
        let (_, fields) = split_class_fields(rest);
        let threads = parse_threads(fields);
        assert_eq!(threads.len(), 1);
        assert_eq!(threads[0].id, 1);
        assert_eq!(threads[0].target_id, "Thread \"weird, {name}\"");
        assert_eq!(threads[0].state, "stopped");
    }

    // Branch order-independence: a -thread-info-shaped ^done,threads=[...]
    // reply must not also emit ThreadSelected (keys are disjoint from
    // `new-thread-id=`).
    #[test]
    fn test_threads_branch_disjoint_from_new_thread_id() {
        let line = r#"^done,threads=[{id="1",target-id="Thread 1",frame={level="0",addr="0x1",func="main",args=[],file="a.c",fullname="/tmp/a.c",line="1"},state="stopped"}],current-thread-id="1""#;
        let event = parse_line(line);
        match event {
            Some(DebuggerEvent::State(StateEvent::ThreadsUpdated { threads, current })) => {
                assert_eq!(threads.len(), 1);
                assert_eq!(current, Some(1));
            }
            other => panic!("expected ThreadsUpdated, got {other:?}"),
        }
    }

    // ─── Thread select (-thread-select) reply ──────────────────────────────

    // -thread-select's reply → ^done,new-thread-id="N",frame={...}. Must emit
    // ThreadSelected{id}, and — the reverse direction of the disjointness
    // check above — a -thread-info-shaped threads=[...] reply must not hit
    // this branch either.
    #[test]
    fn test_new_thread_id_branch_emits_thread_selected() {
        let line = r#"^done,new-thread-id="3",frame={level="0",addr="0x1",func="foo",args=[],file="a.c",fullname="/tmp/a.c",line="1"}"#;
        match parse_line(line) {
            Some(DebuggerEvent::State(StateEvent::ThreadSelected { id })) => {
                assert_eq!(id, 3);
            }
            other => panic!("expected ThreadSelected, got {other:?}"),
        }

        let threads_line = r#"^done,threads=[{id="1",target-id="Thread 1",frame={level="0",addr="0x1",func="main",args=[],file="a.c",fullname="/tmp/a.c",line="1"},state="stopped"}],current-thread-id="1""#;
        match parse_line(threads_line) {
            Some(DebuggerEvent::State(StateEvent::ThreadsUpdated { .. })) => {}
            other => panic!(
                "expected ThreadsUpdated (not ThreadSelected) for a threads=[ reply, got {other:?}"
            ),
        }
    }

    // Threat-matrix RED: the `new-thread-id=` branch must ignore the reply's
    // `frame={...}` payload entirely — `ThreadSelected` carries only `id`, so
    // there is structurally no second writer of `pause.frame` from this path
    // (design.md: "Post-switch frame" decision).
    #[test]
    fn test_new_thread_id_branch_ignores_frame_payload() {
        let line = r#"^done,new-thread-id="7",frame={level="0",addr="0xdead",func="weird_frame",args=[],file="z.c",fullname="/tmp/z.c",line="99"}"#;
        match parse_line(line) {
            Some(DebuggerEvent::State(StateEvent::ThreadSelected { id })) => {
                assert_eq!(id, 7);
                // ThreadSelected has no frame field by construction — the
                // frame payload above is parsed by nothing.
            }
            other => panic!("expected ThreadSelected, got {other:?}"),
        }
    }

    // Error-path: an ^error reply to -thread-select must route only through
    // the existing "error" arm (GdbError), never reach the "done" arm
    // branches above.
    #[test]
    fn test_thread_select_error_reply_routes_to_error_arm() {
        let line = r#"^error,msg="Thread ID 99 not known.""#;
        match parse_line(line) {
            Some(DebuggerEvent::Ui(UiEvent::GdbError(msg))) => {
                assert_eq!(msg, "Thread ID 99 not known.");
            }
            other => panic!("expected GdbError, got {other:?}"),
        }
    }

    // Spec: "Thread List Refresh Scope Excludes Live Lifecycle Events" — a
    // `=thread-created` / `=thread-exited` notify-async record must not be
    // parsed into any state change; the roster refreshes only on pause or a
    // successful switch. `parse_notify_async`'s `_ => None` arm already
    // covers this; this test pins that behavior against regressions (e.g. a
    // future contributor adding a match arm for these classes).
    #[test]
    fn test_thread_lifecycle_notify_async_records_are_not_parsed_into_state_changes() {
        assert!(parse_line(r#"=thread-created,id="2",group-id="i1""#).is_none());
        assert!(parse_line(r#"=thread-exited,id="2",group-id="i1""#).is_none());
    }

    // ─── Watchpoints ────────────────────────────────────────────────────────

    // Real GDB MI capture shape for `-break-watch x` (Write, default kind).
    #[test]
    fn test_break_watch_write_reply() {
        let line = r#"5^done,wpt={number="2",exp="x"}"#;
        match parse_line(line) {
            Some(DebuggerEvent::State(StateEvent::WatchpointAdded { watchpoint })) => {
                assert_eq!(watchpoint.id, 2);
                assert_eq!(watchpoint.expr, "x");
                assert_eq!(watchpoint.kind, WatchpointKind::Write);
                assert!(watchpoint.enabled);
            }
            other => panic!("expected WatchpointAdded, got {other:?}"),
        }
    }

    // `-break-watch -r c` reply shape.
    #[test]
    fn test_break_watch_read_reply() {
        let line = r#"6^done,hw-rwpt={number="3",exp="c"}"#;
        match parse_line(line) {
            Some(DebuggerEvent::State(StateEvent::WatchpointAdded { watchpoint })) => {
                assert_eq!(watchpoint.id, 3);
                assert_eq!(watchpoint.expr, "c");
                assert_eq!(watchpoint.kind, WatchpointKind::Read);
            }
            other => panic!("expected WatchpointAdded, got {other:?}"),
        }
    }

    // `-break-watch -a c` reply shape.
    #[test]
    fn test_break_watch_access_reply() {
        let line = r#"7^done,hw-awpt={number="5",exp="c"}"#;
        match parse_line(line) {
            Some(DebuggerEvent::State(StateEvent::WatchpointAdded { watchpoint })) => {
                assert_eq!(watchpoint.id, 5);
                assert_eq!(watchpoint.expr, "c");
                assert_eq!(watchpoint.kind, WatchpointKind::Access);
            }
            other => panic!("expected WatchpointAdded, got {other:?}"),
        }
    }

    // Branch disjointness: a plain -break-insert reply (bkpt=) must never be
    // mistaken for a watchpoint reply, and vice versa — pins the checking
    // order against regressions.
    #[test]
    fn test_break_insert_reply_is_disjoint_from_watchpoint_branch() {
        let line = r#"8^done,bkpt={number="1",type="breakpoint",disp="keep",enabled="y",addr="0x1149",func="main",file="a.c",fullname="/a.c",line="10",times="0",original-location="a.c:10"}"#;
        match parse_line(line) {
            Some(DebuggerEvent::State(StateEvent::BreakpointAdded { breakpoint })) => {
                assert_eq!(breakpoint.id, 1);
            }
            other => panic!("expected BreakpointAdded, got {other:?}"),
        }
    }

    // D3 (design.md): a watchpoint's `=breakpoint-modified` notify carries no
    // `file`/`fullname` — `parse_breakpoint_field`'s existing `?`-bail on a
    // missing file must drop it, so no ghost breakpoint row is created for a
    // watchpoint. Pinned here as a coupling constraint for future changes to
    // that function (e.g. a catchpoints change making it more tolerant).
    #[test]
    fn test_watchpoint_breakpoint_modified_notify_yields_none() {
        let line = r#"=breakpoint-modified,bkpt={number="2",type="hw watchpoint",disp="keep",enabled="y",what="x",times="0"}"#;
        assert!(parse_line(line).is_none());
    }

    // `*stopped,reason="watchpoint-trigger"` real MI capture shape: old/new
    // are blocks, not bare fields — a naive extract_str(fields, "value")
    // would match `old`'s value for both, so this pins the block-scoped
    // extraction.
    #[test]
    fn test_watchpoint_trigger_stop_reason() {
        let line = r#"*stopped,reason="watchpoint-trigger",wpt={number="2",exp="x"},old={value="0"},new={value="3"},frame={func="main",args=[],file="recursive2.c",fullname="/home/foo/bar/recursive2.c",line="6"},thread-id="1""#;
        match parse_line(line) {
            Some(DebuggerEvent::State(StateEvent::ProgramPaused { pause })) => {
                match pause.stop_reason {
                    StopReason::WatchpointTriggered { id, expr, old, new } => {
                        assert_eq!(id, 2);
                        assert_eq!(expr, "x");
                        assert_eq!(old, "0");
                        assert_eq!(new, "3");
                    }
                    other => panic!("expected WatchpointTriggered, got {other:?}"),
                }
                assert_eq!(pause.frame.function, "main");
            }
            other => panic!("expected ProgramPaused, got {other:?}"),
        }
    }

    // `*stopped,reason="watchpoint-scope"` WITH a frame= field present.
    #[test]
    fn test_watchpoint_scope_stop_reason_with_frame() {
        let line = r#"*stopped,reason="watchpoint-scope",wpnum="2",frame={func="callee4",args=[],file="basics.c",fullname="/home/foo/bar/basics.c",line="18"},thread-id="1""#;
        match parse_line(line) {
            Some(DebuggerEvent::State(StateEvent::ProgramPaused { pause })) => {
                match pause.stop_reason {
                    StopReason::WatchpointScope(id) => assert_eq!(id, 2),
                    other => panic!("expected WatchpointScope, got {other:?}"),
                }
                assert_eq!(pause.frame.function, "callee4");
            }
            other => panic!("expected ProgramPaused, got {other:?}"),
        }
    }

    // D5 (design.md): `*stopped,reason="watchpoint-scope"` WITHOUT a
    // frame= field must still parse into a ProgramPaused (synthetic frame
    // fallback) rather than being silently dropped by `?` — otherwise
    // auto-removal never runs and a stale row survives.
    #[test]
    fn test_watchpoint_scope_stop_reason_without_frame_falls_back() {
        let line = r#"*stopped,reason="watchpoint-scope",wpnum="7",thread-id="1""#;
        match parse_line(line) {
            Some(DebuggerEvent::State(StateEvent::ProgramPaused { pause })) => {
                match pause.stop_reason {
                    StopReason::WatchpointScope(id) => assert_eq!(id, 7),
                    other => panic!("expected WatchpointScope, got {other:?}"),
                }
                // Synthetic fallback frame — no real frame was available.
                assert_eq!(pause.frame.function, "??");
                assert_eq!(pause.frame.file, None);
            }
            other => panic!("expected ProgramPaused, got {other:?}"),
        }
    }

    // Triangulation: an ordinary breakpoint-hit stop must still bail on a
    // missing frame= exactly as before — the D5 fallback is scoped to
    // watchpoint-scope only.
    #[test]
    fn test_breakpoint_hit_without_frame_is_still_dropped() {
        let line = r#"*stopped,reason="breakpoint-hit",bkptno="1",thread-id="1""#;
        assert!(parse_line(line).is_none());
    }

    // Integration: a real GDB MI transcript (create → trigger → scope) run
    // through the actual `parse_line` + `DebuggerState::apply` pipeline, not
    // just the individual unit boundaries tested above (spec/tasks: "real
    // GDB MI transcripts (add → trigger → scope → delete) through
    // parse_line + apply").
    #[test]
    fn test_watchpoint_full_lifecycle_via_parse_line_and_apply() {
        use crate::state::DebuggerState;

        let mut state = DebuggerState::new();

        // 1. `-break-watch x` reply: self-describing creation.
        match parse_line(r#"5^done,wpt={number="2",exp="x"}"#) {
            Some(DebuggerEvent::State(s)) => state.apply(s),
            other => panic!("expected a state event for the create reply, got {other:?}"),
        }
        assert_eq!(state.persistent.watchpoints.len(), 1);
        assert_eq!(state.persistent.watchpoints[0].expr, "x");

        // 2. `x` changes value: watchpoint-trigger pauses and carries old/new.
        let trigger_line = r#"*stopped,reason="watchpoint-trigger",wpt={number="2",exp="x"},old={value="0"},new={value="3"},frame={func="main",args=[],file="a.c",fullname="/a.c",line="6"},thread-id="1""#;
        match parse_line(trigger_line) {
            Some(DebuggerEvent::State(s)) => state.apply(s),
            other => panic!("expected a state event for the trigger, got {other:?}"),
        }
        assert!(state.is_paused());
        assert_eq!(
            state.persistent.watchpoints.len(),
            1,
            "a trigger must not remove the row — only scope exit does"
        );

        // 3. `x` goes out of scope: silent auto-removal (D6), no separate
        // toast/event — the row disappears as a direct consequence of apply.
        let scope_line = r#"*stopped,reason="watchpoint-scope",wpnum="2",thread-id="1""#;
        match parse_line(scope_line) {
            Some(DebuggerEvent::State(s)) => state.apply(s),
            other => panic!("expected a state event for the scope exit, got {other:?}"),
        }
        assert!(state.persistent.watchpoints.is_empty());
    }

    // ─── Catchpoints (Phase 2a, design addendum #70) ──────────────────────────

    // D3/A6 PINNED REGRESSION: a catchpoint's `bkpt={...}` record must never
    // be parsed by `parse_breakpoint_field` — it always lacks `file`/
    // `fullname`, so the existing `?`-bail already guarantees this, but this
    // test pins it loudly against a future change to that function (e.g. a
    // future contributor making it more tolerant) rather than letting it
    // silently start producing ghost breakpoint rows for catchpoints.
    #[test]
    fn pinned_parse_breakpoint_field_rejects_catchpoint_record() {
        let fields = r#"bkpt={number="1",type="catchpoint",disp="keep",enabled="y",catch-type="fork",thread-groups=["i1"],times="0"}"#;
        assert!(parse_breakpoint_field(fields, "bkpt").is_none());
    }

    // ── parse_catchpoint_field: table-driven, one row per kind ───────────────

    #[test]
    fn parse_catchpoint_field_fork_vfork_exec_have_no_args() {
        for (catch_type, kind) in [
            ("fork", CatchpointKind::Fork),
            ("vfork", CatchpointKind::Vfork),
            ("exec", CatchpointKind::Exec),
        ] {
            let block = format!(
                r#"number="1",type="catchpoint",disp="keep",enabled="y",catch-type="{catch_type}",thread-groups=["i1"],times="0""#
            );
            let cp = parse_catchpoint_field(&block)
                .unwrap_or_else(|| panic!("expected Some for catch-type={catch_type}"));
            assert_eq!(cp.id, 1);
            assert_eq!(cp.kind, kind);
            assert!(cp.args.is_empty());
            assert!(cp.enabled);
        }
    }

    #[test]
    fn parse_catchpoint_field_signal_takes_bare_what_as_arg() {
        let block = r#"number="2",type="catchpoint",disp="keep",enabled="y",what="SIGINT",catch-type="signal",times="0""#;
        let cp = parse_catchpoint_field(block).expect("expected Some");
        assert_eq!(cp.kind, CatchpointKind::Signal);
        assert_eq!(cp.args, vec!["SIGINT".to_string()]);
    }

    #[test]
    fn parse_catchpoint_field_load_extracts_arg_from_matching_prose() {
        let block = r#"number="3",type="catchpoint",disp="keep",enabled="y",what="load of library matching libc",catch-type="load",times="0""#;
        let cp = parse_catchpoint_field(block).expect("expected Some");
        assert_eq!(cp.kind, CatchpointKind::Load);
        assert_eq!(cp.args, vec!["libc".to_string()]);
    }

    #[test]
    fn parse_catchpoint_field_unload_no_arg_bare_prose_yields_empty_args() {
        let block = r#"number="4",type="catchpoint",disp="keep",enabled="y",what="unload of library",catch-type="unload",times="0""#;
        let cp = parse_catchpoint_field(block).expect("expected Some");
        assert_eq!(cp.kind, CatchpointKind::Unload);
        assert!(cp.args.is_empty());
    }

    // A4 defensive fallback: an unrecognized `what=` prose shape (e.g. a
    // future GDB version's differently worded text) must not panic — it
    // falls back to the raw text as a single arg.
    #[test]
    fn parse_catchpoint_field_load_unrecognized_prose_falls_back_to_raw_what() {
        let block = r#"number="5",type="catchpoint",disp="keep",enabled="y",what="some future wording",catch-type="load",times="0""#;
        let cp = parse_catchpoint_field(block).expect("expected Some");
        assert_eq!(cp.args, vec!["some future wording".to_string()]);
    }

    #[test]
    fn parse_catchpoint_field_unknown_catch_type_yields_none() {
        let block =
            r#"number="6",type="catchpoint",disp="keep",enabled="y",catch-type="bogus-type",times="0""#;
        assert!(parse_catchpoint_field(block).is_none());
    }

    // Phase 2b task 1.3: catch-type="syscall" maps to CatchpointKind::Syscall.
    #[test]
    fn parse_catchpoint_kind_syscall_maps_to_syscall_variant() {
        assert_eq!(
            parse_catchpoint_kind("syscall"),
            Some(CatchpointKind::Syscall)
        );
    }

    // Phase 2b task 1.4 (D4): Syscall's `what=` field carries the raw
    // name/number/group token verbatim — no GDB prose to strip, unlike
    // Load/Unload's `"... matching X"` shape.
    #[test]
    fn catchpoint_args_from_what_syscall_absent_is_empty() {
        assert!(catchpoint_args_from_what(CatchpointKind::Syscall, None).is_empty());
    }

    #[test]
    fn catchpoint_args_from_what_syscall_bare_empty_string_is_empty() {
        assert!(catchpoint_args_from_what(CatchpointKind::Syscall, Some("")).is_empty());
    }

    #[test]
    fn catchpoint_args_from_what_syscall_name_extracts_single_arg() {
        assert_eq!(
            catchpoint_args_from_what(CatchpointKind::Syscall, Some("open")),
            vec!["open".to_string()]
        );
    }

    #[test]
    fn catchpoint_args_from_what_syscall_number_extracts_single_arg() {
        assert_eq!(
            catchpoint_args_from_what(CatchpointKind::Syscall, Some("2")),
            vec!["2".to_string()]
        );
    }

    #[test]
    fn catchpoint_args_from_what_syscall_group_selector_extracts_single_arg() {
        assert_eq!(
            catchpoint_args_from_what(CatchpointKind::Syscall, Some("group:process_vm")),
            vec!["group:process_vm".to_string()]
        );
    }

    // Defensive fallback (design D4 open question): if GDB's `what=` ever
    // carries the `print_one_catch_syscall`-style prose (`syscall "open"` /
    // `syscalls "open close"`) instead of a bare token, extraction must
    // still recover the clean name(s) rather than storing the quoted prose
    // verbatim — otherwise `should_add_catchpoint`'s duplicate check would
    // never match a user-entered `["open"]` (D4 regression, mirrors Load's
    // `" matching "` stripping rationale).
    #[test]
    fn catchpoint_args_from_what_syscall_strips_verified_gdb_prose_if_present() {
        assert_eq!(
            catchpoint_args_from_what(CatchpointKind::Syscall, Some(r#"syscall "open""#)),
            vec!["open".to_string()]
        );
        assert_eq!(
            catchpoint_args_from_what(CatchpointKind::Syscall, Some(r#"syscalls "open close""#)),
            vec!["open".to_string(), "close".to_string()]
        );
        assert!(catchpoint_args_from_what(CatchpointKind::Syscall, Some("syscall")).is_empty());
    }

    // ── Phase 2c: Throw/Rethrow/Catch (C++ exceptions) ───────────────────────
    //
    // Phase 0 spike (live GDB 17.2, C++ throw/rethrow/catch fixture)
    // confirmed A1 (verbs exist), A2 (hit is breakpoint-hit+bkptno), and A3
    // (creation reply is tokened `^done,bkpt={...}`). It ALSO found the
    // reply/notify carries a dedicated `regexp="..."` field (present only
    // when `-r` was given, absent — not empty — otherwise), separate from
    // the fixed `what="exception throw"` prose. This is richer than design
    // D4/D5 assumed (echo-and-merge from process.rs): the regexp is
    // self-describing on every ingress path exactly like every other
    // catchpoint kind, so no client-side echo/correlation is needed — see
    // apply-progress deviation note.

    #[test]
    fn parse_catchpoint_kind_throw_rethrow_catch_map_to_variants() {
        assert_eq!(parse_catchpoint_kind("throw"), Some(CatchpointKind::Throw));
        assert_eq!(
            parse_catchpoint_kind("rethrow"),
            Some(CatchpointKind::Rethrow)
        );
        assert_eq!(parse_catchpoint_kind("catch"), Some(CatchpointKind::Catch));
    }

    #[test]
    fn parse_catchpoint_field_throw_bare_has_no_args() {
        let block = r#"number="1",type="catchpoint",disp="keep",enabled="y",what="exception throw",catch-type="throw",thread-groups=["i1"],times="0""#;
        let cp = parse_catchpoint_field(block).expect("expected Some");
        assert_eq!(cp.kind, CatchpointKind::Throw);
        assert!(cp.args.is_empty());
    }

    #[test]
    fn parse_catchpoint_field_rethrow_bare_has_no_args() {
        let block = r#"number="2",type="catchpoint",disp="keep",enabled="y",what="exception rethrow",catch-type="rethrow",thread-groups=["i1"],times="0""#;
        let cp = parse_catchpoint_field(block).expect("expected Some");
        assert_eq!(cp.kind, CatchpointKind::Rethrow);
        assert!(cp.args.is_empty());
    }

    #[test]
    fn parse_catchpoint_field_catch_bare_has_no_args() {
        let block = r#"number="3",type="catchpoint",disp="keep",enabled="y",what="exception catch",catch-type="catch",thread-groups=["i1"],times="0""#;
        let cp = parse_catchpoint_field(block).expect("expected Some");
        assert_eq!(cp.kind, CatchpointKind::Catch);
        assert!(cp.args.is_empty());
    }

    // Live-verified shape (Phase 0 spike): a `-catch-throw -r <regexp>`
    // reply carries the filter in a dedicated `regexp=` field, never in
    // `what=` (which stays the fixed "exception throw" prose regardless).
    #[test]
    fn parse_catchpoint_field_throw_with_regexp_extracts_from_dedicated_field() {
        let block = r#"number="4",type="catchpoint",disp="keep",enabled="y",what="exception throw",catch-type="throw",thread-groups=["i1"],regexp="std::runtime_error",times="0""#;
        let cp = parse_catchpoint_field(block).expect("expected Some");
        assert_eq!(cp.kind, CatchpointKind::Throw);
        assert_eq!(cp.args, vec!["std::runtime_error".to_string()]);
    }

    #[test]
    fn parse_catchpoint_field_rethrow_with_regexp_extracts_from_dedicated_field() {
        let block = r#"number="5",type="catchpoint",disp="keep",enabled="y",what="exception rethrow",catch-type="rethrow",thread-groups=["i1"],regexp="std::logic_error",times="0""#;
        let cp = parse_catchpoint_field(block).expect("expected Some");
        assert_eq!(cp.args, vec!["std::logic_error".to_string()]);
    }

    #[test]
    fn parse_catchpoint_field_catch_with_regexp_extracts_from_dedicated_field() {
        let block = r#"number="6",type="catchpoint",disp="keep",enabled="y",what="exception catch",catch-type="catch",thread-groups=["i1"],regexp="MyException",times="0""#;
        let cp = parse_catchpoint_field(block).expect("expected Some");
        assert_eq!(cp.args, vec!["MyException".to_string()]);
    }

    // Threat matrix — untrusted ingress: a malformed/missing `regexp=` on an
    // exception kind must degrade to empty args, never panic.
    #[test]
    fn parse_catchpoint_field_throw_malformed_regexp_field_degrades_to_empty_args() {
        // Unterminated quote — extract_str's block scan must not panic.
        let block = r#"number="7",type="catchpoint",disp="keep",enabled="y",what="exception throw",catch-type="throw",thread-groups=["i1"],regexp=unterminated,times="0""#;
        let cp = parse_catchpoint_field(block).expect("expected Some");
        assert!(cp.args.is_empty());
    }

    #[test]
    fn parse_catchpoint_field_throw_missing_number_yields_none_not_panic() {
        let block =
            r#"type="catchpoint",disp="keep",enabled="y",what="exception throw",catch-type="throw",times="0""#;
        assert!(parse_catchpoint_field(block).is_none());
    }

    #[test]
    fn parse_catchpoint_field_disabled_reads_enabled_false() {
        let block =
            r#"number="7",type="catchpoint",disp="keep",enabled="n",catch-type="fork",times="0""#;
        let cp = parse_catchpoint_field(block).expect("expected Some");
        assert!(!cp.enabled);
    }

    // ── Ingress path 1: parse_notify_async (untokened, all 6 kinds) ─────────
    // Real GDB 17.2 capture shapes (apply-blocker #68).

    #[test]
    fn notify_async_breakpoint_created_fork_yields_catchpoint_added() {
        let line = r#"=breakpoint-created,bkpt={number="1",type="catchpoint",disp="keep",enabled="y",catch-type="fork",thread-groups=["i1"],times="0"}"#;
        match parse_line(line) {
            Some(DebuggerEvent::State(StateEvent::CatchpointAdded { catchpoint })) => {
                assert_eq!(catchpoint.id, 1);
                assert_eq!(catchpoint.kind, CatchpointKind::Fork);
            }
            other => panic!("expected CatchpointAdded, got {other:?}"),
        }
    }

    #[test]
    fn notify_async_breakpoint_created_signal_yields_catchpoint_added() {
        let line = r#"=breakpoint-created,bkpt={number="2",type="catchpoint",disp="keep",enabled="y",what="SIGINT",catch-type="signal",thread-groups=["i1"],times="0"}"#;
        match parse_line(line) {
            Some(DebuggerEvent::State(StateEvent::CatchpointAdded { catchpoint })) => {
                assert_eq!(catchpoint.kind, CatchpointKind::Signal);
                assert_eq!(catchpoint.args, vec!["SIGINT".to_string()]);
            }
            other => panic!("expected CatchpointAdded, got {other:?}"),
        }
    }

    // breakpoint-modified for a catchpoint must re-emit CatchpointAdded too
    // (idempotent replace-by-id in apply) — not silently dropped.
    #[test]
    fn notify_async_breakpoint_modified_catchpoint_yields_catchpoint_added() {
        let line = r#"=breakpoint-modified,bkpt={number="1",type="catchpoint",disp="keep",enabled="n",catch-type="fork",thread-groups=["i1"],times="0"}"#;
        match parse_line(line) {
            Some(DebuggerEvent::State(StateEvent::CatchpointAdded { catchpoint })) => {
                assert_eq!(catchpoint.id, 1);
                assert!(!catchpoint.enabled);
            }
            other => panic!("expected CatchpointAdded, got {other:?}"),
        }
    }

    // A6/D3 disjointness: an ordinary breakpoint's created/modified notify
    // (no catch-type=) must be completely unaffected by this change.
    #[test]
    fn notify_async_breakpoint_created_without_catch_type_still_yields_breakpoint_added() {
        let line = r#"=breakpoint-created,bkpt={number="1",type="breakpoint",disp="keep",enabled="y",addr="0x1149",func="main",file="a.c",fullname="/a.c",line="10",times="0",original-location="a.c:10"}"#;
        match parse_line(line) {
            Some(DebuggerEvent::State(StateEvent::BreakpointAdded { breakpoint })) => {
                assert_eq!(breakpoint.id, 1);
            }
            other => panic!("expected BreakpointAdded, got {other:?}"),
        }
    }

    // ── Ingress path 2: parse_result's done arm (tokened, Load/Unload only) ──

    #[test]
    fn result_done_catch_load_yields_catchpoint_added() {
        let line = r#"3^done,bkpt={number="3",type="catchpoint",disp="keep",enabled="y",what="load of library matching libc",catch-type="load",times="0"}"#;
        match parse_line(line) {
            Some(DebuggerEvent::State(StateEvent::CatchpointAdded { catchpoint })) => {
                assert_eq!(catchpoint.kind, CatchpointKind::Load);
                assert_eq!(catchpoint.args, vec!["libc".to_string()]);
            }
            other => panic!("expected CatchpointAdded, got {other:?}"),
        }
    }

    // D3 disjointness (both directions): a plain -break-insert ^done must
    // never be mistaken for a catchpoint reply, and a catchpoint ^done must
    // never fall into the plain-bkpt branch.
    #[test]
    fn result_done_catch_type_branch_is_disjoint_from_plain_bkpt_branch() {
        let plain = r#"8^done,bkpt={number="1",type="breakpoint",disp="keep",enabled="y",addr="0x1149",func="main",file="a.c",fullname="/a.c",line="10",times="0",original-location="a.c:10"}"#;
        match parse_line(plain) {
            Some(DebuggerEvent::State(StateEvent::BreakpointAdded { breakpoint })) => {
                assert_eq!(breakpoint.id, 1);
            }
            other => panic!("expected BreakpointAdded, got {other:?}"),
        }
    }

    // ── Stop reasons: fork/vforked/exec/solib-event (D4) ─────────────────────

    #[test]
    fn stop_reason_fork_with_newpid() {
        let line = r#"*stopped,reason="fork",newpid="12345",frame={func="main",args=[],file="a.c",fullname="/a.c",line="10"},thread-id="1""#;
        match parse_line(line) {
            Some(DebuggerEvent::State(StateEvent::ProgramPaused { pause })) => {
                match pause.stop_reason {
                    StopReason::Forked { newpid } => assert_eq!(newpid, Some(12345)),
                    other => panic!("expected Forked, got {other:?}"),
                }
            }
            other => panic!("expected ProgramPaused, got {other:?}"),
        }
    }

    // D5: fork without a frame= must still parse via the synthetic-frame
    // fallback, not be silently dropped.
    #[test]
    fn stop_reason_fork_without_frame_falls_back_to_synthetic_frame() {
        let line = r#"*stopped,reason="fork",newpid="12345",thread-id="1""#;
        match parse_line(line) {
            Some(DebuggerEvent::State(StateEvent::ProgramPaused { pause })) => {
                match pause.stop_reason {
                    StopReason::Forked { newpid } => assert_eq!(newpid, Some(12345)),
                    other => panic!("expected Forked, got {other:?}"),
                }
                assert_eq!(pause.frame.function, "??");
            }
            other => panic!("expected ProgramPaused, got {other:?}"),
        }
    }

    #[test]
    fn stop_reason_fork_without_newpid_is_none() {
        let line = r#"*stopped,reason="fork",frame={func="main",args=[],file="a.c",fullname="/a.c",line="10"},thread-id="1""#;
        match parse_line(line) {
            Some(DebuggerEvent::State(StateEvent::ProgramPaused { pause })) => {
                match pause.stop_reason {
                    StopReason::Forked { newpid } => assert_eq!(newpid, None),
                    other => panic!("expected Forked, got {other:?}"),
                }
            }
            other => panic!("expected ProgramPaused, got {other:?}"),
        }
    }

    #[test]
    fn stop_reason_vforked_with_newpid() {
        let line = r#"*stopped,reason="vforked",newpid="999",frame={func="main",args=[],file="a.c",fullname="/a.c",line="10"},thread-id="1""#;
        match parse_line(line) {
            Some(DebuggerEvent::State(StateEvent::ProgramPaused { pause })) => {
                match pause.stop_reason {
                    StopReason::Vforked { newpid } => assert_eq!(newpid, Some(999)),
                    other => panic!("expected Vforked, got {other:?}"),
                }
            }
            other => panic!("expected ProgramPaused, got {other:?}"),
        }
    }

    #[test]
    fn stop_reason_vforked_without_frame_falls_back_to_synthetic_frame() {
        let line = r#"*stopped,reason="vforked",newpid="999",thread-id="1""#;
        match parse_line(line) {
            Some(DebuggerEvent::State(StateEvent::ProgramPaused { pause })) => {
                assert_eq!(pause.frame.function, "??");
            }
            other => panic!("expected ProgramPaused, got {other:?}"),
        }
    }

    #[test]
    fn stop_reason_exec_without_frame_falls_back_to_synthetic_frame() {
        let line = r#"*stopped,reason="exec",thread-id="1""#;
        match parse_line(line) {
            Some(DebuggerEvent::State(StateEvent::ProgramPaused { pause })) => {
                match pause.stop_reason {
                    StopReason::Execd { .. } => {}
                    other => panic!("expected Execd, got {other:?}"),
                }
                assert_eq!(pause.frame.function, "??");
            }
            other => panic!("expected ProgramPaused, got {other:?}"),
        }
    }

    #[test]
    fn stop_reason_solib_event_with_frame() {
        // Real GDB 17.2 capture shape (apply-blocker #68): `added=[library=...]`.
        let line = r#"*stopped,reason="solib-event",frame={func="_dl_open",args=[],file="a.c",fullname="/a.c",line="10"},thread-id="1""#;
        match parse_line(line) {
            Some(DebuggerEvent::State(StateEvent::ProgramPaused { pause })) => {
                assert!(matches!(pause.stop_reason, StopReason::SolibEvent { .. }));
            }
            other => panic!("expected ProgramPaused, got {other:?}"),
        }
    }

    #[test]
    fn stop_reason_solib_event_extracts_library_name() {
        let line = r#"*stopped,reason="solib-event",added=[library="/lib/libfoo.so"],frame={func="_dl_open",args=[],file="a.c",fullname="/a.c",line="10"},thread-id="1""#;
        match parse_line(line) {
            Some(DebuggerEvent::State(StateEvent::ProgramPaused { pause })) => {
                match pause.stop_reason {
                    StopReason::SolibEvent { library } => {
                        assert_eq!(library, Some("/lib/libfoo.so".to_string()))
                    }
                    other => panic!("expected SolibEvent, got {other:?}"),
                }
            }
            other => panic!("expected ProgramPaused, got {other:?}"),
        }
    }

    #[test]
    fn stop_reason_solib_event_without_library_field_is_none() {
        let line = r#"*stopped,reason="solib-event",frame={func="_dl_open",args=[],file="a.c",fullname="/a.c",line="10"},thread-id="1""#;
        match parse_line(line) {
            Some(DebuggerEvent::State(StateEvent::ProgramPaused { pause })) => {
                match pause.stop_reason {
                    StopReason::SolibEvent { library } => assert_eq!(library, None),
                    other => panic!("expected SolibEvent, got {other:?}"),
                }
            }
            other => panic!("expected ProgramPaused, got {other:?}"),
        }
    }

    #[test]
    fn stop_reason_solib_event_without_frame_falls_back_to_synthetic_frame() {
        let line = r#"*stopped,reason="solib-event",thread-id="1""#;
        match parse_line(line) {
            Some(DebuggerEvent::State(StateEvent::ProgramPaused { pause })) => {
                assert!(matches!(pause.stop_reason, StopReason::SolibEvent { .. }));
                assert_eq!(pause.frame.function, "??");
            }
            other => panic!("expected ProgramPaused, got {other:?}"),
        }
    }

    // ── Stop reasons: syscall-entry / syscall-return (D3, Phase 2b task 1.5) ──
    // Exhaustive matrix: both phases × {both fields, number-only, name-only,
    // neither field}.

    #[test]
    fn stop_reason_syscall_entry_full() {
        let line = r#"*stopped,reason="syscall-entry",syscall-number="2",syscall-name="open",frame={func="open",args=[],file="a.c",fullname="/a.c",line="10"},thread-id="1""#;
        match parse_line(line) {
            Some(DebuggerEvent::State(StateEvent::ProgramPaused { pause })) => {
                match pause.stop_reason {
                    StopReason::SyscallTriggered {
                        phase,
                        number,
                        name,
                    } => {
                        assert_eq!(phase, SyscallPhase::Entry);
                        assert_eq!(number, Some("2".to_string()));
                        assert_eq!(name, Some("open".to_string()));
                    }
                    other => panic!("expected SyscallTriggered, got {other:?}"),
                }
            }
            other => panic!("expected ProgramPaused, got {other:?}"),
        }
    }

    #[test]
    fn stop_reason_syscall_entry_number_only() {
        let line = r#"*stopped,reason="syscall-entry",syscall-number="2",frame={func="open",args=[],file="a.c",fullname="/a.c",line="10"},thread-id="1""#;
        match parse_line(line) {
            Some(DebuggerEvent::State(StateEvent::ProgramPaused { pause })) => {
                match pause.stop_reason {
                    StopReason::SyscallTriggered {
                        phase,
                        number,
                        name,
                    } => {
                        assert_eq!(phase, SyscallPhase::Entry);
                        assert_eq!(number, Some("2".to_string()));
                        assert_eq!(name, None);
                    }
                    other => panic!("expected SyscallTriggered, got {other:?}"),
                }
            }
            other => panic!("expected ProgramPaused, got {other:?}"),
        }
    }

    #[test]
    fn stop_reason_syscall_entry_name_only() {
        let line = r#"*stopped,reason="syscall-entry",syscall-name="open",frame={func="open",args=[],file="a.c",fullname="/a.c",line="10"},thread-id="1""#;
        match parse_line(line) {
            Some(DebuggerEvent::State(StateEvent::ProgramPaused { pause })) => {
                match pause.stop_reason {
                    StopReason::SyscallTriggered {
                        phase,
                        number,
                        name,
                    } => {
                        assert_eq!(phase, SyscallPhase::Entry);
                        assert_eq!(number, None);
                        assert_eq!(name, Some("open".to_string()));
                    }
                    other => panic!("expected SyscallTriggered, got {other:?}"),
                }
            }
            other => panic!("expected ProgramPaused, got {other:?}"),
        }
    }

    #[test]
    fn stop_reason_syscall_entry_neither_field() {
        let line = r#"*stopped,reason="syscall-entry",frame={func="open",args=[],file="a.c",fullname="/a.c",line="10"},thread-id="1""#;
        match parse_line(line) {
            Some(DebuggerEvent::State(StateEvent::ProgramPaused { pause })) => {
                match pause.stop_reason {
                    StopReason::SyscallTriggered {
                        phase,
                        number,
                        name,
                    } => {
                        assert_eq!(phase, SyscallPhase::Entry);
                        assert_eq!(number, None);
                        assert_eq!(name, None);
                    }
                    other => panic!("expected SyscallTriggered, got {other:?}"),
                }
            }
            other => panic!("expected ProgramPaused, got {other:?}"),
        }
    }

    #[test]
    fn stop_reason_syscall_return_full() {
        let line = r#"*stopped,reason="syscall-return",syscall-number="2",syscall-name="open",frame={func="open",args=[],file="a.c",fullname="/a.c",line="10"},thread-id="1""#;
        match parse_line(line) {
            Some(DebuggerEvent::State(StateEvent::ProgramPaused { pause })) => {
                match pause.stop_reason {
                    StopReason::SyscallTriggered {
                        phase,
                        number,
                        name,
                    } => {
                        assert_eq!(phase, SyscallPhase::Return);
                        assert_eq!(number, Some("2".to_string()));
                        assert_eq!(name, Some("open".to_string()));
                    }
                    other => panic!("expected SyscallTriggered, got {other:?}"),
                }
            }
            other => panic!("expected ProgramPaused, got {other:?}"),
        }
    }

    #[test]
    fn stop_reason_syscall_return_number_only() {
        let line = r#"*stopped,reason="syscall-return",syscall-number="2",frame={func="open",args=[],file="a.c",fullname="/a.c",line="10"},thread-id="1""#;
        match parse_line(line) {
            Some(DebuggerEvent::State(StateEvent::ProgramPaused { pause })) => {
                match pause.stop_reason {
                    StopReason::SyscallTriggered { phase, number, .. } => {
                        assert_eq!(phase, SyscallPhase::Return);
                        assert_eq!(number, Some("2".to_string()));
                    }
                    other => panic!("expected SyscallTriggered, got {other:?}"),
                }
            }
            other => panic!("expected ProgramPaused, got {other:?}"),
        }
    }

    #[test]
    fn stop_reason_syscall_return_name_only() {
        let line = r#"*stopped,reason="syscall-return",syscall-name="open",frame={func="open",args=[],file="a.c",fullname="/a.c",line="10"},thread-id="1""#;
        match parse_line(line) {
            Some(DebuggerEvent::State(StateEvent::ProgramPaused { pause })) => {
                match pause.stop_reason {
                    StopReason::SyscallTriggered { phase, name, .. } => {
                        assert_eq!(phase, SyscallPhase::Return);
                        assert_eq!(name, Some("open".to_string()));
                    }
                    other => panic!("expected SyscallTriggered, got {other:?}"),
                }
            }
            other => panic!("expected ProgramPaused, got {other:?}"),
        }
    }

    #[test]
    fn stop_reason_syscall_return_neither_field() {
        let line = r#"*stopped,reason="syscall-return",frame={func="open",args=[],file="a.c",fullname="/a.c",line="10"},thread-id="1""#;
        match parse_line(line) {
            Some(DebuggerEvent::State(StateEvent::ProgramPaused { pause })) => {
                match pause.stop_reason {
                    StopReason::SyscallTriggered {
                        phase,
                        number,
                        name,
                    } => {
                        assert_eq!(phase, SyscallPhase::Return);
                        assert_eq!(number, None);
                        assert_eq!(name, None);
                    }
                    other => panic!("expected SyscallTriggered, got {other:?}"),
                }
            }
            other => panic!("expected ProgramPaused, got {other:?}"),
        }
    }

    // ── D5 frame-fallback: Syscall extends the synthetic-frame set (task 1.6) ─

    #[test]
    fn stop_reason_syscall_entry_without_frame_falls_back_to_synthetic_frame() {
        let line = r#"*stopped,reason="syscall-entry",syscall-number="2",syscall-name="open",thread-id="1""#;
        match parse_line(line) {
            Some(DebuggerEvent::State(StateEvent::ProgramPaused { pause })) => {
                assert!(matches!(
                    pause.stop_reason,
                    StopReason::SyscallTriggered { .. }
                ));
                assert_eq!(pause.frame.function, "??");
            }
            other => panic!("expected ProgramPaused, got {other:?}"),
        }
    }

    #[test]
    fn stop_reason_syscall_return_without_frame_falls_back_to_synthetic_frame() {
        let line = r#"*stopped,reason="syscall-return",syscall-number="2",thread-id="1""#;
        match parse_line(line) {
            Some(DebuggerEvent::State(StateEvent::ProgramPaused { pause })) => {
                assert!(matches!(
                    pause.stop_reason,
                    StopReason::SyscallTriggered { .. }
                ));
                assert_eq!(pause.frame.function, "??");
            }
            other => panic!("expected ProgramPaused, got {other:?}"),
        }
    }

    // Triangulation: an ordinary breakpoint-hit without a frame must still
    // be dropped — the D5 fallback stays scoped to the six listed reasons.
    #[test]
    fn breakpoint_hit_without_frame_still_dropped_after_catchpoint_extension() {
        let line = r#"*stopped,reason="breakpoint-hit",bkptno="1",thread-id="1""#;
        assert!(parse_line(line).is_none());
    }

    // ── Integration: real MI transcripts through parse_line + apply ─────────

    // Fork: console-add -> untokened =breakpoint-created -> *stopped,reason="fork"
    // -> -break-delete removal (RemoveCatchpoint is optimistic, not tested
    // here — this integration test covers the parse+apply side only).
    #[test]
    fn catchpoint_fork_add_stop_via_parse_line_and_apply() {
        use crate::state::DebuggerState;
        let mut state = DebuggerState::new();

        match parse_line(
            r#"=breakpoint-created,bkpt={number="1",type="catchpoint",disp="keep",enabled="y",catch-type="fork",thread-groups=["i1"],times="0"}"#,
        ) {
            Some(DebuggerEvent::State(s)) => state.apply(s),
            other => panic!("expected a state event for the create notify, got {other:?}"),
        }
        assert_eq!(state.persistent.catchpoints.len(), 1);
        assert_eq!(state.persistent.catchpoints[0].kind, CatchpointKind::Fork);

        match parse_line(
            r#"*stopped,reason="fork",newpid="4242",frame={func="main",args=[],file="a.c",fullname="/a.c",line="10"},thread-id="1""#,
        ) {
            Some(DebuggerEvent::State(s)) => state.apply(s),
            other => panic!("expected a state event for the fork stop, got {other:?}"),
        }
        assert!(state.is_paused());
        match &state.pause.as_ref().unwrap().stop_reason {
            StopReason::Forked { newpid } => assert_eq!(*newpid, Some(4242)),
            other => panic!("expected Forked, got {other:?}"),
        }

        state.apply(StateEvent::CatchpointRemoved { id: 1 });
        assert!(state.persistent.catchpoints.is_empty());
    }

    // Signal: console-add -> untokened =breakpoint-created -> signal-received.
    #[test]
    fn catchpoint_signal_add_then_signal_received_via_parse_line_and_apply() {
        use crate::state::DebuggerState;
        let mut state = DebuggerState::new();

        match parse_line(
            r#"=breakpoint-created,bkpt={number="2",type="catchpoint",disp="keep",enabled="y",what="SIGINT",catch-type="signal",thread-groups=["i1"],times="0"}"#,
        ) {
            Some(DebuggerEvent::State(s)) => state.apply(s),
            other => panic!("expected a state event for the create notify, got {other:?}"),
        }
        assert_eq!(
            state.persistent.catchpoints[0].args,
            vec!["SIGINT".to_string()]
        );

        match parse_line(
            r#"*stopped,reason="signal-received",signal-name="SIGINT",frame={func="main",args=[],file="a.c",fullname="/a.c",line="10"},thread-id="1""#,
        ) {
            Some(DebuggerEvent::State(s)) => state.apply(s),
            other => panic!("expected a state event for the signal stop, got {other:?}"),
        }
        match &state.pause.as_ref().unwrap().stop_reason {
            StopReason::Signal(name) => assert_eq!(name, "SIGINT"),
            other => panic!("expected Signal, got {other:?}"),
        }
    }

    // Load: native tokened ^done with a regex arg embedded in `what=` prose.
    #[test]
    fn catchpoint_load_add_with_regexp_via_parse_line_and_apply() {
        use crate::state::DebuggerState;
        let mut state = DebuggerState::new();

        match parse_line(
            r#"5^done,bkpt={number="3",type="catchpoint",disp="keep",enabled="y",what="load of library matching libfoo",catch-type="load",times="0"}"#,
        ) {
            Some(DebuggerEvent::State(s)) => state.apply(s),
            other => panic!("expected a state event for the load create reply, got {other:?}"),
        }
        assert_eq!(state.persistent.catchpoints.len(), 1);
        assert_eq!(state.persistent.catchpoints[0].kind, CatchpointKind::Load);
        assert_eq!(
            state.persistent.catchpoints[0].args,
            vec!["libfoo".to_string()]
        );
    }

    // Duplicate ^done + notify race for the same load catchpoint: both paths
    // must converge on exactly one row (replace-by-id idempotency, A6).
    #[test]
    fn catchpoint_load_duplicate_done_and_notify_still_one_row() {
        use crate::state::DebuggerState;
        let mut state = DebuggerState::new();

        match parse_line(
            r#"5^done,bkpt={number="3",type="catchpoint",disp="keep",enabled="y",what="load of library matching libc",catch-type="load",times="0"}"#,
        ) {
            Some(DebuggerEvent::State(s)) => state.apply(s),
            other => panic!("expected a state event, got {other:?}"),
        }
        match parse_line(
            r#"=breakpoint-created,bkpt={number="3",type="catchpoint",disp="keep",enabled="y",what="load of library matching libc",catch-type="load",thread-groups=["i1"],times="0"}"#,
        ) {
            Some(DebuggerEvent::State(s)) => state.apply(s),
            other => panic!("expected a state event, got {other:?}"),
        }

        assert_eq!(state.persistent.catchpoints.len(), 1);
    }

    // Phase 2b task 4.3: Syscall full flow — create -> row, *stopped -> a
    // correctly-shaped `SyscallTriggered`. Mirrors the Fork/Signal
    // integration tests above. The remaining hop (`StopReason` ->
    // `stop_event_banner_text`'s rendered string) is covered by
    // `ui::panels::catchpoints`'s own test module (Phase 2b task 3.3) using
    // the identical field shape asserted below — `stop_event_banner_text`
    // is `pub(crate)` to `ui` only, so it cannot be called cross-module from
    // here without loosening `mod panels`/`mod parser` visibility, which is
    // out of this change's scope.
    #[test]
    fn syscall_catchpoint_full_flow_create_then_entry_stop() {
        use crate::state::DebuggerState;
        let mut state = DebuggerState::new();

        match parse_line(
            r#"=breakpoint-created,bkpt={number="2",type="catchpoint",disp="keep",enabled="y",what="open",catch-type="syscall",thread-groups=["i1"],times="0"}"#,
        ) {
            Some(DebuggerEvent::State(s)) => state.apply(s),
            other => panic!("expected a state event for the create notify, got {other:?}"),
        }
        assert_eq!(state.persistent.catchpoints.len(), 1);
        assert_eq!(state.persistent.catchpoints[0].kind, CatchpointKind::Syscall);
        assert_eq!(
            state.persistent.catchpoints[0].args,
            vec!["open".to_string()]
        );

        match parse_line(
            r#"*stopped,reason="syscall-entry",syscall-number="2",syscall-name="open",frame={func="open",args=[],file="a.c",fullname="/a.c",line="10"},thread-id="1""#,
        ) {
            Some(DebuggerEvent::State(s)) => state.apply(s),
            other => panic!("expected a state event for the syscall stop, got {other:?}"),
        }
        assert!(state.is_paused());
        match &state.pause.as_ref().unwrap().stop_reason {
            StopReason::SyscallTriggered {
                phase,
                number,
                name,
            } => {
                assert_eq!(*phase, SyscallPhase::Entry);
                assert_eq!(number, &Some("2".to_string()));
                assert_eq!(name, &Some("open".to_string()));
                // `stop_event_banner_text_syscall_entry_full`
                // (ui::panels::catchpoints) asserts this exact shape
                // renders `Some("Syscall: 2 (open)")`.
            }
            other => panic!("expected SyscallTriggered, got {other:?}"),
        }
    }

    // Phase 2c: Throw full flow — native tokened ^done with a `regexp=`
    // field -> row with the echoed regexp -> *stopped,reason=
    // "breakpoint-hit" (A2, no dedicated StopReason). The rendered banner
    // hop (`exception_banner_text`) is covered by
    // `ui::panels::catchpoints`'s own test module, same disjointness
    // reasoning as the Syscall precedent above.
    #[test]
    fn catchpoint_throw_add_with_regexp_then_breakpoint_hit_via_parse_line_and_apply() {
        use crate::state::DebuggerState;
        let mut state = DebuggerState::new();

        match parse_line(
            r#"4^done,bkpt={number="4",type="catchpoint",disp="keep",enabled="y",what="exception throw",catch-type="throw",thread-groups=["i1"],regexp="std::runtime_error",times="0"}"#,
        ) {
            Some(DebuggerEvent::State(s)) => state.apply(s),
            other => panic!("expected a state event for the throw create reply, got {other:?}"),
        }
        assert_eq!(state.persistent.catchpoints.len(), 1);
        assert_eq!(state.persistent.catchpoints[0].kind, CatchpointKind::Throw);
        assert_eq!(
            state.persistent.catchpoints[0].args,
            vec!["std::runtime_error".to_string()]
        );

        match parse_line(
            r#"*stopped,bkptno="4",reason="breakpoint-hit",frame={func="__cxa_throw",args=[],file="a.c",fullname="/a.c",line="10"},thread-id="1""#,
        ) {
            Some(DebuggerEvent::State(s)) => state.apply(s),
            other => panic!("expected a state event for the throw stop, got {other:?}"),
        }
        assert!(state.is_paused());
        match &state.pause.as_ref().unwrap().stop_reason {
            StopReason::BreakpointHit(id) => assert_eq!(*id, 4),
            other => panic!("expected BreakpointHit, got {other:?}"),
        }
        // `exception_banner_text_throw_hit_shows_exception_thrown`
        // (ui::panels::catchpoints) asserts this exact shape renders
        // `Some("Exception thrown")`.
    }

    // Duplicate ^done + a later re-emit (e.g. the `=breakpoint-modified`
    // burst the spike observed at `-exec-run`) for the same throw
    // catchpoint must converge on exactly one row with the regexp intact —
    // no D5 merge needed (see debugger_state.rs deviation note): the
    // `regexp=` field is self-describing on every ingress path.
    #[test]
    fn catchpoint_throw_duplicate_done_and_modified_still_one_row_with_regexp_intact() {
        use crate::state::DebuggerState;
        let mut state = DebuggerState::new();

        match parse_line(
            r#"4^done,bkpt={number="4",type="catchpoint",disp="keep",enabled="y",what="exception throw",catch-type="throw",thread-groups=["i1"],regexp="std::runtime_error",times="0"}"#,
        ) {
            Some(DebuggerEvent::State(s)) => state.apply(s),
            other => panic!("expected a state event, got {other:?}"),
        }
        match parse_line(
            r#"=breakpoint-modified,bkpt={number="4",type="catchpoint",disp="keep",enabled="y",what="exception throw",catch-type="throw",thread-groups=["i1"],regexp="std::runtime_error",times="0"}"#,
        ) {
            Some(DebuggerEvent::State(s)) => state.apply(s),
            other => panic!("expected a state event, got {other:?}"),
        }

        assert_eq!(state.persistent.catchpoints.len(), 1);
        assert_eq!(
            state.persistent.catchpoints[0].args,
            vec!["std::runtime_error".to_string()]
        );
    }
}
