// ─── Frame ────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct Frame {
    pub addr: u64,
    pub function: String,
    pub file: Option<String>,
    pub line: Option<u32>,
}

// ─── Breakpoint ───────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct Breakpoint {
    pub id: u32,
    pub file: String,
    pub line: u32,
    pub enabled: bool,
}

// ─── Variable (watch / locals) ────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct Variable {
    pub name: String,
    pub value: String,
    pub type_: String,
}

// ─── Stop reason ─────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub enum StopReason {
    BreakpointHit(u32), // id del breakpoint
    EndStepping,
    Signal(String), // "SIGSEGV", "SIGINT", …
    Unknown,
}

// ─── Pause state  ────────────────

#[derive(Clone, Debug)]
pub struct PauseState {
    pub thread_id: u32,
    pub frame: Frame,
    pub stack: Vec<Frame>,
    pub stop_reason: StopReason,
}

// ─── Program state ────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub enum ProgramState {
    NoProgramLoaded,
    ProgramLoaded,
    Running,
    Paused,
    Exited { code: Option<i32> },
}

// ─── Persistent state (sobrevive entre runs) ──────────────────────────────────

#[derive(Clone, Debug)]
pub struct PersistentState {
    pub executable: Option<String>,
    pub breakpoints: Vec<Breakpoint>,
}

// ─── Top-level state ──────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct DebuggerState {
    pub program: ProgramState,
    pub pause: Option<PauseState>,
    pub locals: Vec<Variable>,
    pub persistent: PersistentState,
}

// ─── Events ───────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub enum StateEvent {
    ProgramLoaded { executable: String },
    ProgramStarted,
    ProgramPaused { pause: PauseState },
    ProgramExited { code: Option<i32> },
    BreakpointAdded { breakpoint: Breakpoint },
    BreakpointRemoved { id: u32 },
    BreakpointToggled { id: u32, enabled: bool },
    LocalsUpdated { vars: Vec<Variable> },
}

#[derive(Clone, Debug)]
pub enum UiEvent {
    ConsoleOutput(String),
    GdbError(String),
}

#[derive(Clone, Debug)]
pub enum DebuggerEvent {
    State(StateEvent),
    Ui(UiEvent),
}

// ─── impl DebuggerState ───────────────────────────────────────────────────────

impl DebuggerState {
    pub fn new() -> Self {
        Self {
            program: ProgramState::NoProgramLoaded,
            pause: None,
            locals: vec![],
            persistent: PersistentState {
                executable: None,
                breakpoints: vec![],
            },
        }
    }

    /// Única puerta de mutación del estado.
    pub fn apply(&mut self, event: StateEvent) {
        match event {
            StateEvent::ProgramLoaded { executable } => {
                self.program = ProgramState::ProgramLoaded;
                self.persistent.executable = Some(executable);
                self.pause = None;
                self.locals = vec![];
            }

            StateEvent::ProgramStarted => {
                self.program = ProgramState::Running;
                self.pause = None;
                self.locals = vec![];
            }

            StateEvent::ProgramPaused { pause } => {
                self.program = ProgramState::Paused;
                self.pause = Some(pause);
            }

            StateEvent::ProgramExited { code } => {
                self.program = ProgramState::Exited { code };
                self.pause = None;
                self.locals = vec![];
            }

            StateEvent::BreakpointAdded { breakpoint } => {
                self.persistent.breakpoints.push(breakpoint);
            }

            StateEvent::BreakpointRemoved { id } => {
                self.persistent.breakpoints.retain(|b| b.id != id);
            }

            StateEvent::BreakpointToggled { id, enabled } => {
                if let Some(bp) = self.persistent.breakpoints.iter_mut().find(|b| b.id == id) {
                    bp.enabled = enabled;
                }
            }

            StateEvent::LocalsUpdated { vars } => {
                self.locals = vars;
            }
        }
    }

    // ── Helpers de consulta (la UI los usa para no acceder a campos raw) ──────

    pub fn is_paused(&self) -> bool {
        matches!(self.program, ProgramState::Paused)
    }

    pub fn is_running(&self) -> bool {
        matches!(self.program, ProgramState::Running)
    }

    pub fn current_file(&self) -> Option<&str> {
        self.pause.as_ref()?.frame.file.as_deref()
    }

    pub fn current_line(&self) -> Option<u32> {
        self.pause.as_ref()?.frame.line
    }

    pub fn current_function(&self) -> Option<&str> {
        Some(self.pause.as_ref()?.frame.function.as_str())
    }

    pub fn breakpoint_at(&self, file: &str, line: u32) -> Option<&Breakpoint> {
        self.persistent
            .breakpoints
            .iter()
            .find(|b| b.file == file && b.line == line)
    }
}

impl Default for DebuggerState {
    fn default() -> Self {
        Self::new()
    }
}
