use crate::state::{EditTarget, WatchpointKind};

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

    // Program
    LoadExecutable(String),

    RequestLocals,
    RequestStack,
    RequestRegisterNames,
    RequestRegisters,
    RequestDisasm,
    RequestThreads,
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
}
