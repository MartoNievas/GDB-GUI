use crate::state::{CatchpointKind, EditTarget, WatchpointKind};

#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    // Execution
    Run,
    Continue,
    Step,
    Next,
    Finish,
    Interrupt,
    Restart,

    // Breakpoints
    AddBreakpoint {
        file: String,
        line: u32,
        condition: Option<String>,
    },
    RemoveBreakpoint(u32),
    ToggleBreakpoint {
        id: u32,
        enable: bool,
    },
    /// Sets (non-empty) or clears (empty string) the condition on an
    /// existing breakpoint via `-break-condition <id> [<cond>]`.
    SetBreakpointCondition {
        id: u32,
        condition: String,
    },

    // Watchpoints
    /// Creates an expression watchpoint via `-break-watch [-r|-a] <expr>`
    /// depending on `kind`. Fire-and-forget: the reply is self-describing
    /// (`wpt=`/`hw-rwpt=`/`hw-awpt=`), so no `pending_*` map is needed for a
    /// successful creation — only `^error` is correlated back to `expr`.
    AddWatchpoint {
        expr: String,
        kind: WatchpointKind,
    },
    /// Deletes a watchpoint via `-break-delete <id>` — reuses the shared
    /// breakpoint/watchpoint/catchpoint id space and lifecycle verb.
    RemoveWatchpoint(u32),
    /// Toggles a watchpoint's active state via `-break-enable`/
    /// `-break-disable <id>`.
    ToggleWatchpoint {
        id: u32,
        enable: bool,
    },
    /// One-shot probe: `-break-insert -t main`, sent right after
    /// `ProgramLoaded` so the source view has something to show before the
    /// user hits Run. Its reply is intercepted and correlated by MI token
    /// in `process.rs` (`pending_probe`) — it never becomes a `Breakpoint`
    /// row and never reaches the UI as a `Command`/state event of its own.
    ProbeMainSource,

    // Catchpoints
    /// Creates an event catchpoint. Dispatched to one of two MI transports
    /// depending on `kind` (design addendum A1 — supersedes design #65's D7,
    /// which assumed a uniform native `-catch-<kind>` verb that GDB 17.2
    /// does not implement for Fork/Vfork/Exec/Signal): those four go through
    /// `-interpreter-exec console "catch ..."`; Load/Unload keep the native
    /// `-catch-load`/`-catch-unload` verbs. Fire-and-forget with NO
    /// send-time optimistic event (addendum A2, overrides the brief): the
    /// GDB id is unknown until the asynchronous, untokened
    /// `=breakpoint-created` notify arrives and is parsed into
    /// `CatchpointAdded` — only a rejected attempt (`^error`) is correlated
    /// back to this request, via `pending_catch` (addendum A3).
    AddCatchpoint {
        kind: CatchpointKind,
        args: Vec<String>,
    },
    /// Deletes a catchpoint via `-break-delete <id>` — dedicated variant,
    /// NOT `RemoveBreakpoint`/`RemoveWatchpoint` reused (design decision D2):
    /// `process.rs`'s optimistic dispatch keys off the `Command` variant, so
    /// reusing another variant would emit the wrong `StateEvent` and never
    /// clear this row. Reuses the shared breakpoint/watchpoint/catchpoint id
    /// space and lifecycle verb.
    RemoveCatchpoint(u32),
    /// Toggles a catchpoint's active state via `-break-enable`/
    /// `-break-disable <id>` — dedicated variant for the same D2 reason as
    /// `RemoveCatchpoint`.
    ToggleCatchpoint {
        id: u32,
        enable: bool,
    },

    // Program
    LoadExecutable(String),
    /// Attaches to an already-running process via `-target-attach {pid}`.
    /// Success is signalled optimistically at dispatch time (design D1) as
    /// `StateEvent::ProcessAttached{pid}`, next to the `pending.attach`
    /// insert in `handle_commands` — the eventual `*stopped` reply carries
    /// no `reason=`/pid, so it cannot be the source of the success event.
    /// Only `^error` is correlated back (error-only, mirrors
    /// `AddCatchpoint`/`AddWatchpoint`'s pattern).
    AttachToProcess(u32),
    /// Detaches from the attached inferior via `-target-detach` (no
    /// arguments). Named for its single caller (GUI shutdown, design "Detach
    /// on Exit") — there is no interactive/manual detach action in this
    /// change. Acked via `pending.detach` (token-only, mirrors
    /// `Command::ProbeMainSource`'s `pending.probe` shape) on both `^done`
    /// and `^error`, emitting `StateEvent::DetachFinished{error}`.
    DetachForShutdown,
    /// Connects to a remote `gdbserver`/GDB remote stub via
    /// `-target-select extended-remote <host:port>` (design D5: `target` is
    /// the canonical `"{host}:{port}"` rebuilt by the panel from a validated
    /// host + `u16` port, never raw user text). Success is correlated off
    /// `^connected` (never optimistic, unlike `AttachToProcess`) via
    /// `process.rs`'s `pending.remote_connect` map; `^error` is correlated
    /// back to `target` the same way.
    ConnectRemote {
        target: String,
    },
    /// Disconnects from a remote target via `-target-disconnect` (no
    /// arguments). Named for its single caller (GUI shutdown, design D7),
    /// mirroring `DetachForShutdown`'s naming — there is no interactive/
    /// manual disconnect action. Acked via `pending.remote_disconnect`
    /// (token-only, mirrors `pending.detach`'s shape) on both `^done` and
    /// `^error`, emitting `StateEvent::RemoteDisconnected{error}`.
    DisconnectForShutdown,

    RequestLocals,
    RequestStack,
    RequestRegisterNames,
    RequestRegisters,
    RequestDisasm,
    RequestThreads,
    /// Reads target memory via `-data-read-memory-bytes <addr> <count>`.
    /// The reply (`^done,memory=[...]`) is self-describing and parsed
    /// unconditionally, keyed on `memory=[` — no `pending_*` token needed
    /// for success. Only `^error` is correlated back to `address` via
    /// `pending_memory` (mirrors `AddWatchpoint`'s D2 error-only pattern).
    RequestMemory {
        address: String,
        count: u32,
    },
    /// Switches GDB's current thread via `-thread-select <id>`. Clickable
    /// only from a paused thread-panel row (gated UI-side).
    SelectThread(u32),
    /// Evaluates a single expression via `-data-evaluate-expression`. The
    /// struct panel is this variant's sole producer — no separate
    /// `EvaluateStruct` variant exists because replies are correlated by MI
    /// token (`pending_struct` in `process.rs`), not by command identity.
    Evaluate(String),

    // Globals
    RequestGlobalNames,
    EvaluateGlobal(String),

    /// Writes a new value to a local/global variable or a register via
    /// `-gdb-set`. `target` survives unchanged into `pending_edit`, the
    /// resulting `StateEvent::ValueEdit{Succeeded,Failed}`, and the UI's
    /// per-cell error/buffer keys.
    SetValue {
        target: EditTarget,
        value: String,
    },

    Raw(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_breakpoint_carries_optional_condition() {
        let with_cond = Command::AddBreakpoint {
            file: "main.c".into(),
            line: 10,
            condition: Some("i == 10".into()),
        };
        let without_cond = Command::AddBreakpoint {
            file: "main.c".into(),
            line: 10,
            condition: None,
        };
        assert_ne!(with_cond, without_cond);
        assert_eq!(
            with_cond,
            Command::AddBreakpoint {
                file: "main.c".into(),
                line: 10,
                condition: Some("i == 10".into()),
            }
        );
    }

    #[test]
    fn request_threads_command_constructs_and_compares() {
        assert_eq!(Command::RequestThreads, Command::RequestThreads);
        assert_ne!(Command::RequestThreads, Command::RequestStack);
    }

    #[test]
    fn set_value_command_constructs_and_compares() {
        use crate::state::EditTarget;

        let a = Command::SetValue {
            target: EditTarget::Local("x".into()),
            value: "42".into(),
        };
        let b = Command::SetValue {
            target: EditTarget::Local("x".into()),
            value: "42".into(),
        };
        let different_value = Command::SetValue {
            target: EditTarget::Local("x".into()),
            value: "7".into(),
        };
        let different_target = Command::SetValue {
            target: EditTarget::Register("pc".into()),
            value: "42".into(),
        };
        assert_eq!(a, b);
        assert_ne!(a, different_value);
        assert_ne!(a, different_target);
    }

    #[test]
    fn set_breakpoint_condition_constructs_and_compares() {
        let a = Command::SetBreakpointCondition {
            id: 3,
            condition: "count > 3".into(),
        };
        let b = Command::SetBreakpointCondition {
            id: 3,
            condition: "count > 3".into(),
        };
        let clear = Command::SetBreakpointCondition {
            id: 3,
            condition: String::new(),
        };
        assert_eq!(a, b);
        assert_ne!(a, clear);
    }

    #[test]
    fn add_watchpoint_carries_expr_and_kind() {
        let write = Command::AddWatchpoint {
            expr: "x".into(),
            kind: WatchpointKind::Write,
        };
        let read = Command::AddWatchpoint {
            expr: "x".into(),
            kind: WatchpointKind::Read,
        };
        let other_expr = Command::AddWatchpoint {
            expr: "y".into(),
            kind: WatchpointKind::Write,
        };
        assert_eq!(
            write,
            Command::AddWatchpoint {
                expr: "x".into(),
                kind: WatchpointKind::Write,
            }
        );
        assert_ne!(write, read);
        assert_ne!(write, other_expr);
    }

    #[test]
    fn remove_and_toggle_watchpoint_construct_and_compare() {
        assert_eq!(Command::RemoveWatchpoint(3), Command::RemoveWatchpoint(3));
        assert_ne!(Command::RemoveWatchpoint(3), Command::RemoveWatchpoint(4));

        let enable = Command::ToggleWatchpoint {
            id: 3,
            enable: true,
        };
        let disable = Command::ToggleWatchpoint {
            id: 3,
            enable: false,
        };
        assert_eq!(
            enable,
            Command::ToggleWatchpoint {
                id: 3,
                enable: true,
            }
        );
        assert_ne!(enable, disable);
    }

    // ── Catchpoints ──────────────────────────────────────────────────────────

    #[test]
    fn add_catchpoint_carries_kind_and_args() {
        let fork = Command::AddCatchpoint {
            kind: CatchpointKind::Fork,
            args: vec![],
        };
        let signal = Command::AddCatchpoint {
            kind: CatchpointKind::Signal,
            args: vec!["SIGINT".into()],
        };
        let other_args = Command::AddCatchpoint {
            kind: CatchpointKind::Signal,
            args: vec!["SIGSEGV".into()],
        };
        assert_eq!(
            fork,
            Command::AddCatchpoint {
                kind: CatchpointKind::Fork,
                args: vec![],
            }
        );
        assert_ne!(fork, signal);
        assert_ne!(signal, other_args);
    }

    #[test]
    fn remove_and_toggle_catchpoint_construct_and_compare() {
        assert_eq!(Command::RemoveCatchpoint(3), Command::RemoveCatchpoint(3));
        assert_ne!(Command::RemoveCatchpoint(3), Command::RemoveCatchpoint(4));

        let enable = Command::ToggleCatchpoint {
            id: 3,
            enable: true,
        };
        let disable = Command::ToggleCatchpoint {
            id: 3,
            enable: false,
        };
        assert_eq!(
            enable,
            Command::ToggleCatchpoint {
                id: 3,
                enable: true,
            }
        );
        assert_ne!(enable, disable);
    }

    #[test]
    fn request_memory_carries_address_and_count() {
        let a = Command::RequestMemory {
            address: "$sp".into(),
            count: 256,
        };
        let b = Command::RequestMemory {
            address: "$sp".into(),
            count: 256,
        };
        let different_address = Command::RequestMemory {
            address: "0x1000".into(),
            count: 256,
        };
        let different_count = Command::RequestMemory {
            address: "$sp".into(),
            count: 16,
        };
        assert_eq!(a, b);
        assert_ne!(a, different_address);
        assert_ne!(a, different_count);
        assert_ne!(
            Command::RequestMemory {
                address: "$sp".into(),
                count: 256,
            },
            Command::RequestStack
        );
    }

    // D2 regression guard: AddCatchpoint/RemoveCatchpoint/ToggleCatchpoint
    // must be distinct variants from their breakpoint/watchpoint
    // counterparts (never reused), matching design decision D2.
    #[test]
    fn catchpoint_commands_are_distinct_from_watchpoint_commands() {
        assert_ne!(Command::RemoveCatchpoint(3), Command::RemoveWatchpoint(3));
        assert_ne!(
            Command::ToggleCatchpoint {
                id: 3,
                enable: true
            },
            Command::ToggleWatchpoint {
                id: 3,
                enable: true
            }
        );
    }
}
