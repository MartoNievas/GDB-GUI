#[derive(Clone, Debug)]
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
    AddBreakpoint { file: String, line: u32 },
    RemoveBreakpoint(u32),
    ToggleBreakpoint { id: u32, enable: bool },

    // Program
    LoadExecutable(String),

    RequestLocals,
    RequestStack,
    RequestRegisterNames,
    RequestRegisters,
    RequestDisasm,
    Evaluate(String),

    Raw(String),
}
