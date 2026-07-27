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
}
