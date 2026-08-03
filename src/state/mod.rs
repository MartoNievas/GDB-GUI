mod debugger_state;
mod persistence;
mod settings;

pub use debugger_state::{
    AsmLine,
    Breakpoint,
    Catchpoint,
    CatchpointKind,
    // Events
    DebuggerEvent,
    // Core state
    DebuggerState,
    // Types
    EditTarget,
    Frame,
    MemoryBlock,
    PauseState,

    ProgramState,
    Register,
    StateEvent,
    StopReason,
    SyscallPhase,
    ThreadInfo,

    UiEvent,
    Variable,
    Watchpoint,
    WatchpointKind,
};

pub use persistence::{
    BreakpointDTO,
    CatchpointDTO,
    LoadOutcome,
    ProjectFile,
    SCHEMA_VERSION,
    Store,
    WatchpointDTO,
};

pub use settings::{Settings, SettingsStore, persistence_enabled};
