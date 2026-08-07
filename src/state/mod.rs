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

pub(crate) use debugger_state::same_file;
// Slice 2 (Phase 5.2): re-exported so `ui::panels::remote` can call the
// Slice-1 implementation instead of duplicating it — see that function's
// doc comment (SLICE-1 DEVIATION) for why it lives in `debugger_state.rs`
// rather than `ui/panels/remote.rs` as design.md's Interfaces block states.
pub(crate) use debugger_state::parse_remote_target;

pub use persistence::{
    BreakpointDTO,
    CatchpointDTO,
    LoadOutcome,
    ProjectFile,
    SCHEMA_VERSION,
    Store,
    WatchpointDTO,
    mutates_tracepoints,
};

pub use settings::{Settings, SettingsStore, persistence_enabled};
