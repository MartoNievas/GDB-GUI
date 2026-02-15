#[derive(Clone, Debug)]
pub enum Command {
    // Control de ejecución
    Run,
    Continue,
    Step,        // step-in
    Next,        // step-over
    Finish,      // step-out
    Interrupt,   // Ctrl-C
    Restart,

    // Breakpoints
    AddBreakpoint    { file: String, line: u32 },
    RemoveBreakpoint(u32),                        // por id
    ToggleBreakpoint { id: u32, enable: bool },

    // Carga de programa
    LoadExecutable(String),

    // Consultas (la respuesta llega como DebuggerEvent)
    RequestLocals,
    RequestStack,
    Evaluate(String),

    // Consola libre
    Raw(String),
}
