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
    /// Actual line where GDB placed the breakpoint (may differ from the
    /// requested one: when requesting one on a function name's line, GDB
    /// relocates it to the first executable line of the body).
    pub line: u32,
    /// Originally requested line (from `original-location`), if known.
    /// Allows a click on that line to remove the breakpoint even if GDB
    /// moved it elsewhere.
    pub requested_line: Option<u32>,
    pub enabled: bool,
    /// GDB condition expression (`-c "<cond>"` / `-break-condition`), if the
    /// breakpoint is conditional. `None` = unconditional.
    pub condition: Option<String>,
    /// GDB `^error` message after a failed attempt to set/edit the
    /// condition. Cleared (`None`) on any subsequent successful merge.
    pub condition_error: Option<String>,
}

// ─── Variable (locals / watch) ────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct Variable {
    pub name: String,
    pub value: String,

    pub type_: String,
}

// ─── Register ─────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct Register {
    pub number: u32,
    pub name: String,
    pub value: String, // hex: "0x00007fff..."
}

// ─── Edit target (variable / register value writes) ─────────────────────────

/// Identifies the target of a `Command::SetValue` write. Threaded unchanged
/// from `Command` -> `pending_edit` -> `StateEvent` -> `edit_errors` -> the
/// UI's per-cell buffer key, so no row identity is re-derived at any hop.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum EditTarget {
    Local(String),
    Global(String),
    Register(String),
}

// ─── Disassembly ──────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct AsmLine {
    pub addr: u64,
    pub inst: String,
}

// ─── Watchpoint ───────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WatchpointKind {
    Write,
    Read,
    Access,
}

#[derive(Clone, Debug)]
pub struct Watchpoint {
    pub id: u32,
    pub expr: String,
    pub kind: WatchpointKind,
    pub enabled: bool,
}

// ─── Catchpoint ───────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CatchpointKind {
    Fork,
    Vfork,
    Exec,
    Signal,
    Load,
    Unload,
    /// Phase 2b: `catch syscall [name|number|group:foo ...]`. Unit variant
    /// (D1) — `args` already lives in `Catchpoint::args`, so no payload is
    /// needed here.
    Syscall,
    /// Phase 2c: `-catch-throw [-r <regexp>]`. Unit variant (D1) — the
    /// optional regexp filter lives in `Catchpoint::args`, same shape as
    /// Load/Unload.
    Throw,
    /// Phase 2c: `-catch-rethrow [-r <regexp>]`.
    Rethrow,
    /// Phase 2c: `-catch-catch [-r <regexp>]`.
    Catch,
}

impl std::fmt::Display for CatchpointKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            CatchpointKind::Fork => "fork",
            CatchpointKind::Vfork => "vfork",
            CatchpointKind::Exec => "exec",
            CatchpointKind::Signal => "signal",
            CatchpointKind::Load => "load",
            CatchpointKind::Unload => "unload",
            CatchpointKind::Syscall => "syscall",
            CatchpointKind::Throw => "throw",
            CatchpointKind::Rethrow => "rethrow",
            CatchpointKind::Catch => "catch",
        };
        write!(f, "{s}")
    }
}

/// Phase 2b (D3): `catch syscall` stops on both entry and return by
/// default. Distinguishes which phase fired without needing two separate
/// `StopReason` variants.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyscallPhase {
    Entry,
    Return,
}

#[derive(Clone, Debug)]
pub struct Catchpoint {
    pub id: u32,
    pub kind: CatchpointKind,
    pub args: Vec<String>,
    pub enabled: bool,
}

// ─── Stop reason ─────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub enum StopReason {
    BreakpointHit(u32),
    EndStepping,
    Signal(String),
    /// `*stopped,reason="watchpoint-trigger"`: the watched expression's id,
    /// text, and its old/new values (from the `old={value=...}` /
    /// `new={value=...}` blocks).
    WatchpointTriggered {
        id: u32,
        expr: String,
        old: String,
        new: String,
    },
    /// `*stopped,reason="watchpoint-scope"` (the `wpnum` field). Consumed by
    /// `DebuggerState::apply` on `ProgramPaused` to silently remove the row
    /// (design decision D6) — it never becomes a separate `StateEvent`.
    WatchpointScope(u32),
    /// `*stopped,reason="fork"`: a Fork catchpoint fired. `newpid` is the
    /// child process's pid if GDB reported it.
    Forked {
        newpid: Option<u32>,
    },
    /// `*stopped,reason="vforked"`: a Vfork catchpoint fired.
    Vforked {
        newpid: Option<u32>,
    },
    /// `*stopped,reason="exec"`: an Exec catchpoint fired.
    Execd {
        path: Option<String>,
    },
    /// `*stopped,reason="solib-event"`: a Load/Unload catchpoint fired.
    /// `library` is the affected library's name/path if GDB reported one
    /// (verified live shape: `added=[library="..."]`, apply-blocker #68) —
    /// `None` if the field is absent, so the stop-event banner degrades to
    /// "unknown" instead of panicking.
    SolibEvent {
        library: Option<String>,
    },
    /// `*stopped,reason="syscall-entry"|"syscall-return"` (D3): a Syscall
    /// catchpoint fired. `number` and `name` are each independently
    /// optional (GDB may report either, both, or neither); `phase`
    /// distinguishes entry from return so both stops get a labeled banner
    /// instead of one of them falling through to `Unknown`.
    SyscallTriggered {
        phase: SyscallPhase,
        number: Option<String>,
        name: Option<String>,
    },
    Unknown,
}

// ─── Thread ───────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct ThreadInfo {
    pub id: u32,
    pub target_id: String,
    pub state: String,
    pub frame: Option<Frame>,
}

// ─── Pause state ─────────────────────────────────────────────────────────────

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

// ─── Persistent state ────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct PersistentState {
    pub executable: Option<String>,
    pub breakpoints: Vec<Breakpoint>,
    /// Expression watchpoints. In-memory only for the session's lifetime —
    /// never persisted to disk (spec: "No Persistence Across Sessions").
    pub watchpoints: Vec<Watchpoint>,
    /// Event catchpoints (fork/vfork/exec/signal/load/unload). In-memory
    /// only for the session's lifetime, same scope as `watchpoints`.
    pub catchpoints: Vec<Catchpoint>,
}

// ─── Error state ──────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default)]
pub struct ErrorState {
    /// GDB `^error` message from a failed `Command::SetValue`, keyed by the
    /// target that failed. Cleared for a key on that key's next
    /// `ValueEditSucceeded`. Wiped in full on Loaded/Started/Exited, like the
    /// other per-session panels.
    pub edit_errors: std::collections::HashMap<EditTarget, String>,
    /// GDB `^error` message from a failed watchpoint creation/toggle/delete,
    /// keyed by the requested expression (design decision D1 — a rejected
    /// `-break-watch` returns no GDB id, so there is no row to attach an
    /// error to). Cleared on that expression's next successful add. Wiped in
    /// full on Loaded/Started/Exited, like `edit_errors`.
    pub watchpoint_errors: std::collections::HashMap<String, String>,
    /// GDB `^error` message from a failed catchpoint creation/toggle/delete,
    /// keyed by `"{kind}:{args joined}"` (design decision D1 — a rejected
    /// catchpoint attempt returns no GDB id, so there is no row to attach an
    /// error to). Wiped in full on Loaded/Started/Exited, like
    /// `watchpoint_errors`.
    pub catchpoint_errors: std::collections::HashMap<String, String>,
}

// ─── Top-level state ─────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct DebuggerState {
    pub program: ProgramState,
    pub pause: Option<PauseState>,
    pub locals: Vec<Variable>,
    pub register_names: Vec<String>,
    pub registers: Vec<Register>,
    pub disasm: Vec<AsmLine>,
    pub global_names: Vec<String>,
    pub globals: Vec<Variable>,
    pub struct_expr: String,
    pub struct_value: Option<String>,
    pub threads: Vec<ThreadInfo>,
    pub current_thread: Option<u32>,
    pub persistent: PersistentState,
    /// Path resolved by the preload-source probe (`Command::ProbeMainSource`)
    /// while no pause frame exists yet, so the source view has something to
    /// show before the user hits Run. Cleared on `ProgramLoaded` (new
    /// executable, previous resolution stale); deliberately survives
    /// `ProgramStarted`/`ProgramExited` (source stays visible after the
    /// program ends, where today the view goes blank). Read only through
    /// `source_view_file()` — `current_file()` stays pause-only so the
    /// topbar location label keeps meaning "where execution is".
    pub preview_file: Option<String>,
    pub errors: ErrorState,
}

// ─── Events ──────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub enum StateEvent {
    ProgramLoaded {
        executable: String,
    },
    ProgramStarted,
    ProgramPaused {
        pause: PauseState,
    },
    ProgramExited {
        code: Option<i32>,
    },
    StackUpdated {
        frames: Vec<Frame>,
    },
    BreakpointAdded {
        breakpoint: Breakpoint,
    },
    BreakpointRemoved {
        id: u32,
    },
    BreakpointToggled {
        id: u32,
        enabled: bool,
    },
    BreakpointConditionError {
        id: u32,
        message: String,
    },
    LocalsUpdated {
        vars: Vec<Variable>,
    },
    RegisterNamesReceived {
        names: Vec<String>,
    },
    RegistersUpdated {
        registers: Vec<Register>,
    },
    DisasmUpdated {
        lines: Vec<AsmLine>,
    },
    GlobalNamesReceived {
        names: Vec<String>,
    },
    GlobalValueUpdated {
        name: String,
        value: String,
    },
    StructValueUpdated {
        expr: String,
        value: String,
    },
    ThreadsUpdated {
        threads: Vec<ThreadInfo>,
        current: Option<u32>,
    },
    ThreadSelected {
        id: u32,
    },
    /// `-gdb-set` acked with `^error`: the write did not take effect. Stores
    /// GDB's message for the affected cell; the value shown is unchanged.
    ValueEditFailed {
        target: EditTarget,
        message: String,
    },
    /// `-gdb-set` acked with bare `^done`: the write succeeded. Carries no
    /// value — the follow-up re-fetch (locals/global/registers, dispatched
    /// from `app.rs`) is the source of truth for the refreshed cell.
    ValueEditSucceeded {
        target: EditTarget,
    },
    /// The preload-source probe resolved `main`'s file (`^done,bkpt={...}`),
    /// correlated and consumed by MI token in `process.rs` before it ever
    /// reaches `parse_result` — this event is the probe's only visible
    /// trace, and it carries no breakpoint id or row of its own.
    SourcePreviewResolved {
        file: String,
    },
    /// A watchpoint was created (self-describing `wpt=`/`hw-rwpt=`/
    /// `hw-awpt=` reply — no token correlation needed for success).
    WatchpointAdded {
        watchpoint: Watchpoint,
    },
    /// `-break-delete <id>` has no reply the removal can be correlated
    /// from, so `process.rs` emits this optimistically at send time,
    /// mirroring `BreakpointRemoved`.
    WatchpointRemoved {
        id: u32,
    },
    /// `-break-enable`/`-break-disable` reply with a bare `^done`
    /// (design decision D4): emitted optimistically by `process.rs` at send
    /// time, since the `=breakpoint-modified` notify for a watchpoint is
    /// dropped by `parse_breakpoint_field`'s `file`/`fullname` bail (D3).
    WatchpointToggled {
        id: u32,
        enabled: bool,
    },
    /// GDB `^error` for a watchpoint creation/toggle/delete attempt, or a
    /// locally rejected duplicate expression (see `has_watchpoint_expr`).
    WatchpointError {
        expr: String,
        message: String,
    },
    /// A catchpoint was created (self-describing `bkpt={type="catchpoint",
    /// catch-type=...}` reply/notify — no token correlation needed for
    /// success).
    CatchpointAdded {
        catchpoint: Catchpoint,
    },
    /// `-break-delete <id>` has no reply the removal can be correlated
    /// from, so `process.rs` emits this optimistically at send time,
    /// mirroring `WatchpointRemoved`.
    CatchpointRemoved {
        id: u32,
    },
    /// `-break-enable`/`-break-disable` reply with a bare `^done`: emitted
    /// optimistically by `process.rs` at send time, mirroring
    /// `WatchpointToggled`.
    CatchpointToggled {
        id: u32,
        enabled: bool,
    },
    /// GDB `^error` for a catchpoint creation/toggle/delete attempt, or a
    /// locally rejected duplicate (kind, args) pair.
    CatchpointError {
        key: String,
        message: String,
    },
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

// ─── impl ────────────────────────────────────────────────────────────────────

impl DebuggerState {
    pub fn new() -> Self {
        Self {
            program: ProgramState::NoProgramLoaded,
            pause: None,
            locals: vec![],
            register_names: vec![],
            registers: vec![],
            disasm: vec![],
            global_names: vec![],
            globals: vec![],
            struct_expr: String::new(),
            struct_value: None,
            threads: vec![],
            current_thread: None,
            persistent: PersistentState {
                executable: None,
                breakpoints: vec![],
                watchpoints: vec![],
                catchpoints: vec![],
            },
            preview_file: None,
            errors: ErrorState::default(),
        }
    }

    pub fn apply(&mut self, event: StateEvent) {
        match event {
            StateEvent::ProgramLoaded { executable } => {
                self.program = ProgramState::ProgramLoaded;
                self.persistent.executable = Some(executable);
                self.pause = None;
                self.locals = vec![];
                self.register_names = vec![];
                self.registers = vec![];
                self.disasm = vec![];
                self.global_names = vec![];
                self.globals = vec![];
                // New executable: symbols change, so a previously committed
                // expression may no longer resolve — reset both.
                self.struct_expr = String::new();
                self.struct_value = None;
                self.threads = vec![];
                self.current_thread = None;
                // New executable: a previously resolved preview path may no
                // longer apply.
                self.preview_file = None;
                self.errors.edit_errors.clear();
                self.errors.watchpoint_errors.clear();
                self.errors.catchpoint_errors.clear();
            }

            StateEvent::ProgramStarted => {
                self.program = ProgramState::Running;
                self.pause = None;
                self.locals = vec![];
                // register_names are architecture-static — fetched once on load,
                // so we keep them across runs instead of wiping and refetching.
                self.registers = vec![];
                self.disasm = vec![];
                self.globals = vec![];
                // struct_expr survives run/pause/continue cycles so it is
                // auto-re-evaluated on the next pause; only the stale value clears.
                self.struct_value = None;
                self.threads = vec![];
                self.current_thread = None;
                self.errors.edit_errors.clear();
                self.errors.watchpoint_errors.clear();
                self.errors.catchpoint_errors.clear();
            }

            StateEvent::ProgramPaused { pause } => {
                self.program = ProgramState::Paused;
                // D6: watchpoint-scope auto-removal is a state consequence of
                // the pause, not a separate event — no toast, single test
                // site (spec: "Watchpoint-Scope Auto-Removal").
                if let StopReason::WatchpointScope(id) = pause.stop_reason {
                    self.persistent.watchpoints.retain(|w| w.id != id);
                }
                self.pause = Some(pause);
            }

            StateEvent::StackUpdated { frames } => {
                // Arrives right after *stopped (which only carries the top frame);
                // replaces the single-frame stack with the full one.
                if let Some(pause) = &mut self.pause {
                    if let Some(top) = frames.first() {
                        pause.frame = top.clone();
                    }
                    pause.stack = frames;
                }
            }

            StateEvent::ProgramExited { code } => {
                self.program = ProgramState::Exited { code };
                self.pause = None;
                self.locals = vec![];
                // Keep register_names — same executable, same architecture.
                self.registers = vec![];
                self.disasm = vec![];
                self.globals = vec![];
                self.struct_value = None;
                self.threads = vec![];
                self.current_thread = None;
                self.errors.edit_errors.clear();
                self.errors.watchpoint_errors.clear();
                self.errors.catchpoint_errors.clear();
            }

            StateEvent::BreakpointAdded { breakpoint } => {
                if let Some(existing) = self
                    .persistent
                    .breakpoints
                    .iter_mut()
                    .find(|b| b.id == breakpoint.id)
                {
                    *existing = breakpoint;
                } else {
                    self.persistent.breakpoints.push(breakpoint);
                }
            }

            StateEvent::BreakpointRemoved { id } => {
                self.persistent.breakpoints.retain(|b| b.id != id);
            }

            StateEvent::BreakpointToggled { id, enabled } => {
                if let Some(bp) = self.persistent.breakpoints.iter_mut().find(|b| b.id == id) {
                    bp.enabled = enabled;
                }
            }

            StateEvent::BreakpointConditionError { id, message } => {
                if let Some(bp) = self.persistent.breakpoints.iter_mut().find(|b| b.id == id) {
                    bp.condition_error = Some(message);
                }
            }

            StateEvent::LocalsUpdated { vars } => self.locals = vars,
            StateEvent::RegisterNamesReceived { names } => self.register_names = names,
            StateEvent::RegistersUpdated { registers } => self.registers = registers,
            StateEvent::DisasmUpdated { lines } => self.disasm = lines,

            StateEvent::GlobalNamesReceived { names } => self.global_names = names,
            StateEvent::GlobalValueUpdated { name, value } => {
                if let Some(existing) = self.globals.iter_mut().find(|v| v.name == name) {
                    existing.value = value;
                } else {
                    self.globals.push(Variable {
                        name,
                        value,
                        type_: String::new(),
                    });
                }
            }
            StateEvent::StructValueUpdated { expr, value } => {
                if expr == self.struct_expr {
                    self.struct_value = Some(value);
                }
            }
            StateEvent::ThreadsUpdated { threads, current } => {
                self.threads = threads;
                self.current_thread = current;
            }
            // Only updates current_thread — the reply's frame={...} is
            // deliberately ignored (see design.md "Post-switch frame"):
            // -stack-list-frames stays the single writer of pause.frame.
            StateEvent::ThreadSelected { id } => {
                self.current_thread = Some(id);
            }

            StateEvent::ValueEditFailed { target, message } => {
                self.errors.edit_errors.insert(target, message);
            }
            StateEvent::ValueEditSucceeded { target } => {
                self.errors.edit_errors.remove(&target);
            }
            StateEvent::SourcePreviewResolved { file } => {
                self.preview_file = Some(file);
            }

            StateEvent::WatchpointAdded { watchpoint } => {
                if let Some(existing) = self
                    .persistent
                    .watchpoints
                    .iter_mut()
                    .find(|w| w.id == watchpoint.id)
                {
                    *existing = watchpoint;
                } else {
                    self.persistent.watchpoints.push(watchpoint);
                }
            }
            StateEvent::WatchpointRemoved { id } => {
                self.persistent.watchpoints.retain(|w| w.id != id);
            }
            StateEvent::WatchpointToggled { id, enabled } => {
                if let Some(wp) = self.persistent.watchpoints.iter_mut().find(|w| w.id == id) {
                    wp.enabled = enabled;
                }
            }
            StateEvent::WatchpointError { expr, message } => {
                self.errors.watchpoint_errors.insert(expr, message);
            }

            StateEvent::CatchpointAdded { catchpoint } => {
                if let Some(existing) = self
                    .persistent
                    .catchpoints
                    .iter_mut()
                    .find(|c| c.id == catchpoint.id)
                {
                    *existing = catchpoint;
                } else {
                    self.persistent.catchpoints.push(catchpoint);
                }
            }
            StateEvent::CatchpointRemoved { id } => {
                self.persistent.catchpoints.retain(|c| c.id != id);
            }
            StateEvent::CatchpointToggled { id, enabled } => {
                if let Some(cp) = self.persistent.catchpoints.iter_mut().find(|c| c.id == id) {
                    cp.enabled = enabled;
                }
            }
            StateEvent::CatchpointError { key, message } => {
                self.errors.catchpoint_errors.insert(key, message);
            }
        }
    }

    // ── Helpers ───────────────────────────────────────────────────────────────

    pub fn is_paused(&self) -> bool {
        matches!(self.program, ProgramState::Paused)
    }

    pub fn is_running(&self) -> bool {
        matches!(self.program, ProgramState::Running)
    }

    pub fn current_file(&self) -> Option<&str> {
        self.pause.as_ref()?.frame.file.as_deref()
    }

    /// File the source view should display: the pause frame's file while
    /// stopped, else the preload-source probe's resolution (if any). Falls
    /// back automatically once `pause` is set again — `current_file()` and
    /// `preview_file` are never both consulted by anything else, so
    /// precedence lives here alone.
    pub fn source_view_file(&self) -> Option<&str> {
        self.current_file().or(self.preview_file.as_deref())
    }

    pub fn current_line(&self) -> Option<u32> {
        self.pause.as_ref()?.frame.line
    }

    pub fn current_function(&self) -> Option<&str> {
        Some(self.pause.as_ref()?.frame.function.as_str())
    }

    pub fn current_addr(&self) -> Option<u64> {
        Some(self.pause.as_ref()?.frame.addr)
    }

    /// Breakpoint that a click on `line` should toggle. Matches either the
    /// actual line where GDB placed it or the originally requested line, so
    /// it can be removed both from a function name's line and from the
    /// executable line GDB relocated it to.
    pub fn breakpoint_at(&self, file: &str, line: u32) -> Option<&Breakpoint> {
        self.persistent.breakpoints.iter().find(|b| {
            same_file(&b.file, file) && (b.line == line || b.requested_line == Some(line))
        })
    }

    /// Should the breakpoint marker be drawn on this line? Only on the actual
    /// line where GDB halts execution, not the requested one (which GDB may
    /// have relocated), so no phantom dot shows up on two lines.
    pub fn has_breakpoint_marker(&self, file: &str, line: u32) -> bool {
        self.persistent
            .breakpoints
            .iter()
            .any(|b| b.line == line && same_file(&b.file, file))
    }

    /// GDB's `^error` message from the last failed write to `target`, if
    /// any. `None` means no failed edit is pending display for this target.
    pub fn edit_error(&self, target: &EditTarget) -> Option<&str> {
        self.errors.edit_errors.get(target).map(|s| s.as_str())
    }

    /// Whether a watchpoint already exists for `expr` (exact string match).
    /// Used to reject a duplicate creation attempt client-side, with no MI
    /// round trip (spec: "Duplicate Expression Prevention").
    pub fn has_watchpoint_expr(&self, expr: &str) -> bool {
        self.persistent.watchpoints.iter().any(|w| w.expr == expr)
    }

    /// GDB's `^error` message (or a locally rejected duplicate's message)
    /// for `expr`, if any. `None` means no error is pending display for this
    /// expression.
    pub fn watchpoint_error(&self, expr: &str) -> Option<&str> {
        self.errors.watchpoint_errors.get(expr).map(|s| s.as_str())
    }

    /// Whether a catchpoint already exists for this exact `(kind, args)`
    /// pair. Used to reject a duplicate creation attempt client-side, with
    /// no MI round trip.
    pub fn has_catchpoint(&self, kind: CatchpointKind, args: &[String]) -> bool {
        self.persistent
            .catchpoints
            .iter()
            .any(|c| c.kind == kind && c.args == args)
    }

    /// GDB's `^error` message (or a locally rejected duplicate's message)
    /// for `key` (`"{kind}:{args joined}"`), if any.
    pub fn catchpoint_error(&self, key: &str) -> Option<&str> {
        self.errors.catchpoint_errors.get(key).map(|s| s.as_str())
    }

    /// Catchpoint with the given GDB id, if any — used to correlate a
    /// `BreakpointHit(id)` stop against an armed catchpoint.
    pub fn catchpoint_by_id(&self, id: u32) -> Option<&Catchpoint> {
        self.persistent.catchpoints.iter().find(|c| c.id == id)
    }

    /// Checks whether `candidate` is redundant: another breakpoint (with a
    /// different id) already exists at the same *resolved* location (file +
    /// actual line).
    ///
    /// This happens because GDB relocates breakpoints requested on
    /// non-executable lines to the next actual line: e.g. requesting line 11
    /// and line 12 can both end up on line 12 with two different ids. Returns
    /// `true` to discard the duplicate and keep only one.
    pub fn is_duplicate_breakpoint(&self, candidate: &Breakpoint) -> bool {
        self.persistent.breakpoints.iter().any(|b| {
            b.id != candidate.id && b.line == candidate.line && same_file(&b.file, &candidate.file)
        })
    }
}

/// Compares two file paths tolerantly.
///
/// GDB may report a breakpoint's path differently from the current frame's
/// (absolute vs relative, with or without prefixes like `./`). We consider
/// two paths to point to the same file if they are equal or if the shorter
/// one is a component-wise suffix of the longer one. Comparing by components
/// avoids false positives like `foobar.c` vs `bar.c`.
fn same_file(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }

    let a_comps: Vec<_> = std::path::Path::new(a).components().collect();
    let b_comps: Vec<_> = std::path::Path::new(b).components().collect();
    let n = a_comps.len().min(b_comps.len());
    if n == 0 {
        return false;
    }

    a_comps[a_comps.len() - n..] == b_comps[b_comps.len() - n..]
}

impl Default for DebuggerState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edit_error_returns_none_for_absent_key() {
        let state = DebuggerState::new();
        assert_eq!(state.edit_error(&EditTarget::Local("x".into())), None);
    }

    #[test]
    fn edit_target_equality_and_hash_roundtrip() {
        use std::collections::HashMap;

        let a = EditTarget::Local("x".into());
        let b = EditTarget::Local("x".into());
        let c = EditTarget::Global("x".into());
        let d = EditTarget::Register("pc".into());

        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_ne!(a, d);

        let mut map: HashMap<EditTarget, &str> = HashMap::new();
        map.insert(a.clone(), "found via a");
        assert_eq!(map.get(&b), Some(&"found via a"));
        assert_eq!(map.get(&c), None);
    }

    #[test]
    fn value_edit_failed_sets_edit_errors_for_target() {
        let mut state = DebuggerState::new();
        let target = EditTarget::Local("x".into());

        state.apply(StateEvent::ValueEditFailed {
            target: target.clone(),
            message: "No symbol \"x\" in current context.".into(),
        });

        assert_eq!(
            state.edit_error(&target),
            Some("No symbol \"x\" in current context.")
        );
    }

    #[test]
    fn value_edit_succeeded_clears_only_its_own_key() {
        let mut state = DebuggerState::new();
        let target_a = EditTarget::Local("x".into());
        let target_b = EditTarget::Global("g_counter".into());
        state
            .errors
            .edit_errors
            .insert(target_a.clone(), "stale error a".into());
        state
            .errors
            .edit_errors
            .insert(target_b.clone(), "stale error b".into());

        state.apply(StateEvent::ValueEditSucceeded {
            target: target_a.clone(),
        });

        assert_eq!(state.edit_error(&target_a), None);
        assert_eq!(state.edit_error(&target_b), Some("stale error b"));
    }

    #[test]
    fn program_loaded_started_exited_wipe_edit_errors() {
        let target = EditTarget::Register("pc".into());

        let mut state = DebuggerState::new();
        state.errors.edit_errors.insert(target.clone(), "e".into());
        state.apply(StateEvent::ProgramLoaded {
            executable: "a.out".into(),
        });
        assert!(state.errors.edit_errors.is_empty());

        let mut state = DebuggerState::new();
        state.errors.edit_errors.insert(target.clone(), "e".into());
        state.apply(StateEvent::ProgramStarted);
        assert!(state.errors.edit_errors.is_empty());

        let mut state = DebuggerState::new();
        state.errors.edit_errors.insert(target, "e".into());
        state.apply(StateEvent::ProgramExited { code: Some(0) });
        assert!(state.errors.edit_errors.is_empty());
    }

    fn bp(id: u32, file: &str, line: u32) -> Breakpoint {
        Breakpoint {
            id,
            file: file.into(),
            line,
            requested_line: None,
            enabled: true,
            condition: None,
            condition_error: None,
        }
    }

    #[test]
    fn threads_updated_sets_threads_and_current() {
        let mut state = DebuggerState::new();
        let threads = vec![
            ThreadInfo {
                id: 1,
                target_id: "Thread 1".into(),
                state: "stopped".into(),
                frame: None,
            },
            ThreadInfo {
                id: 2,
                target_id: "Thread 2".into(),
                state: "stopped".into(),
                frame: None,
            },
        ];

        state.apply(StateEvent::ThreadsUpdated {
            threads: threads.clone(),
            current: Some(2),
        });

        assert_eq!(state.threads.len(), 2);
        assert_eq!(state.threads[0].id, 1);
        assert_eq!(state.current_thread, Some(2));
    }

    #[test]
    fn program_loaded_started_exited_clear_threads() {
        let thread = ThreadInfo {
            id: 1,
            target_id: "Thread 1".into(),
            state: "stopped".into(),
            frame: None,
        };

        let mut state = DebuggerState::new();
        state.threads = vec![thread.clone()];
        state.current_thread = Some(1);
        state.apply(StateEvent::ProgramLoaded {
            executable: "a.out".into(),
        });
        assert!(state.threads.is_empty());
        assert_eq!(state.current_thread, None);

        let mut state = DebuggerState::new();
        state.threads = vec![thread.clone()];
        state.current_thread = Some(1);
        state.apply(StateEvent::ProgramStarted);
        assert!(state.threads.is_empty());
        assert_eq!(state.current_thread, None);

        let mut state = DebuggerState::new();
        state.threads = vec![thread];
        state.current_thread = Some(1);
        state.apply(StateEvent::ProgramExited { code: Some(0) });
        assert!(state.threads.is_empty());
        assert_eq!(state.current_thread, None);
    }

    #[test]
    fn thread_selected_sets_current_leaves_threads_intact() {
        let mut state = DebuggerState::new();
        state.threads = vec![
            ThreadInfo {
                id: 1,
                target_id: "Thread 1".into(),
                state: "stopped".into(),
                frame: None,
            },
            ThreadInfo {
                id: 3,
                target_id: "Thread 3".into(),
                state: "stopped".into(),
                frame: None,
            },
        ];
        state.current_thread = Some(1);

        state.apply(StateEvent::ThreadSelected { id: 3 });

        assert_eq!(state.current_thread, Some(3));
        assert_eq!(state.threads.len(), 2);
        assert_eq!(state.threads[0].id, 1);
        assert_eq!(state.threads[1].id, 3);
    }

    // Design decision "Post-switch frame": the -stack-list-frames reply is the
    // single authoritative writer of `pause.frame`; ThreadSelected must never
    // touch it (no second writer, no race).
    #[test]
    fn thread_selected_leaves_pause_frame_unchanged() {
        let mut state = DebuggerState::new();
        state.pause = Some(PauseState {
            thread_id: 1,
            frame: Frame {
                addr: 0x1000,
                function: "original_frame".into(),
                file: Some("a.c".into()),
                line: Some(5),
            },
            stack: vec![],
            stop_reason: StopReason::Unknown,
        });

        state.apply(StateEvent::ThreadSelected { id: 3 });

        let frame = &state.pause.as_ref().unwrap().frame;
        assert_eq!(frame.function, "original_frame");
        assert_eq!(frame.line, Some(5));
    }

    #[test]
    fn breakpoint_has_condition_and_condition_error_fields() {
        let b = Breakpoint {
            id: 1,
            file: "a.c".into(),
            line: 5,
            requested_line: None,
            enabled: true,
            condition: Some("x == 1".into()),
            condition_error: Some("No symbol \"x\"".into()),
        };
        assert_eq!(b.condition, Some("x == 1".to_string()));
        assert_eq!(b.condition_error, Some("No symbol \"x\"".to_string()));
    }

    // The `=breakpoint-modified` response re-parses the entire row (replace-by-id):
    // it must update `condition` with the new value and clear any previous
    // `condition_error`, since a successful merge implies GDB accepted it.
    #[test]
    fn breakpoint_added_merge_updates_condition_and_clears_error() {
        let mut state = DebuggerState::new();
        state.persistent.breakpoints.push(Breakpoint {
            id: 1,
            file: "/tmp/example.c".into(),
            line: 7,
            requested_line: None,
            enabled: true,
            condition: None,
            condition_error: Some("previous error".into()),
        });

        state.apply(StateEvent::BreakpointAdded {
            breakpoint: Breakpoint {
                id: 1,
                file: "/tmp/example.c".into(),
                line: 7,
                requested_line: None,
                enabled: true,
                condition: Some("count > 3".into()),
                condition_error: None,
            },
        });

        let bp = state
            .persistent
            .breakpoints
            .iter()
            .find(|b| b.id == 1)
            .expect("breakpoint 1 must still exist");
        assert_eq!(bp.condition, Some("count > 3".to_string()));
        assert_eq!(bp.condition_error, None);
    }

    #[test]
    fn breakpoint_condition_error_sets_error_leaves_condition_untouched() {
        let mut state = DebuggerState::new();
        state.persistent.breakpoints.push(Breakpoint {
            id: 1,
            file: "/tmp/example.c".into(),
            line: 7,
            requested_line: None,
            enabled: true,
            condition: Some("x == 1".into()),
            condition_error: None,
        });

        state.apply(StateEvent::BreakpointConditionError {
            id: 1,
            message: "No symbol \"unknown_symbol_xyz\" in current context.".into(),
        });

        let bp = state
            .persistent
            .breakpoints
            .iter()
            .find(|b| b.id == 1)
            .expect("breakpoint 1 must still exist");
        assert_eq!(bp.condition, Some("x == 1".to_string()));
        assert_eq!(
            bp.condition_error,
            Some("No symbol \"unknown_symbol_xyz\" in current context.".to_string())
        );
    }

    #[test]
    fn same_file_matches_absolute_and_relative() {
        assert!(same_file("/tmp/example.c", "/tmp/example.c"));
        assert!(same_file("/tmp/example.c", "example.c"));
        assert!(same_file("example.c", "/tmp/example.c"));
        assert!(same_file("/home/user/src/example.c", "src/example.c"));
        assert!(same_file("./example.c", "example.c"));
    }

    #[test]
    fn same_file_rejects_different_files() {
        assert!(!same_file("/tmp/foobar.c", "bar.c"));
        assert!(!same_file("/tmp/a/example.c", "/tmp/b/example.c"));
        assert!(!same_file("main.c", "example.c"));
    }

    // ── Preload-source preview ──────────────────────────────────────────────

    #[test]
    fn source_preview_resolved_sets_preview_file() {
        let mut state = DebuggerState::new();

        state.apply(StateEvent::SourcePreviewResolved {
            file: "/tmp/main.c".into(),
        });

        assert_eq!(state.preview_file, Some("/tmp/main.c".to_string()));
    }

    #[test]
    fn source_view_file_prefers_pause_frame_over_preview_once_paused() {
        let mut state = DebuggerState::new();
        state.apply(StateEvent::SourcePreviewResolved {
            file: "/tmp/main.c".into(),
        });
        assert_eq!(state.source_view_file(), Some("/tmp/main.c"));

        state.apply(StateEvent::ProgramPaused {
            pause: PauseState {
                thread_id: 1,
                frame: Frame {
                    addr: 0x1000,
                    function: "step_fn".into(),
                    file: Some("/tmp/step.c".into()),
                    line: Some(3),
                },
                stack: vec![],
                stop_reason: StopReason::Unknown,
            },
        });

        assert_eq!(state.source_view_file(), Some("/tmp/step.c"));
    }

    #[test]
    fn program_loaded_clears_preview_file_survives_started_and_exited() {
        let mut state = DebuggerState::new();
        state.apply(StateEvent::SourcePreviewResolved {
            file: "/tmp/main.c".into(),
        });
        state.apply(StateEvent::ProgramLoaded {
            executable: "a.out".into(),
        });
        assert_eq!(state.preview_file, None);

        let mut state = DebuggerState::new();
        state.apply(StateEvent::SourcePreviewResolved {
            file: "/tmp/main.c".into(),
        });
        state.apply(StateEvent::ProgramStarted);
        assert_eq!(state.preview_file, Some("/tmp/main.c".to_string()));

        let mut state = DebuggerState::new();
        state.apply(StateEvent::SourcePreviewResolved {
            file: "/tmp/main.c".into(),
        });
        state.apply(StateEvent::ProgramExited { code: Some(0) });
        assert_eq!(state.preview_file, Some("/tmp/main.c".to_string()));
    }

    #[test]
    fn struct_value_updated_matching_expr_sets_value() {
        let mut state = DebuggerState::new();
        state.struct_expr = "my_struct.field".into();

        state.apply(StateEvent::StructValueUpdated {
            expr: "my_struct.field".into(),
            value: "{a = 1, b = 2}".into(),
        });

        assert_eq!(state.struct_value, Some("{a = 1, b = 2}".to_string()));
    }

    #[test]
    fn struct_value_updated_stale_expr_is_dropped() {
        let mut state = DebuggerState::new();
        state.struct_expr = "current_expr".into();
        state.struct_value = Some("stale value".into());

        state.apply(StateEvent::StructValueUpdated {
            expr: "old_expr".into(),
            value: "should not apply".into(),
        });

        assert_eq!(state.struct_value, Some("stale value".to_string()));
    }

    #[test]
    fn program_loaded_clears_struct_expr_and_struct_value() {
        let mut state = DebuggerState::new();
        state.struct_expr = "my_struct".into();
        state.struct_value = Some("{a = 1}".into());

        state.apply(StateEvent::ProgramLoaded {
            executable: "a.out".into(),
        });

        assert_eq!(state.struct_expr, "");
        assert_eq!(state.struct_value, None);
    }

    #[test]
    fn program_started_clears_only_struct_value() {
        let mut state = DebuggerState::new();
        state.struct_expr = "my_struct".into();
        state.struct_value = Some("{a = 1}".into());

        state.apply(StateEvent::ProgramStarted);

        assert_eq!(state.struct_expr, "my_struct");
        assert_eq!(state.struct_value, None);
    }

    #[test]
    fn program_exited_clears_only_struct_value() {
        let mut state = DebuggerState::new();
        state.struct_expr = "my_struct".into();
        state.struct_value = Some("{a = 1}".into());

        state.apply(StateEvent::ProgramExited { code: Some(0) });

        assert_eq!(state.struct_expr, "my_struct");
        assert_eq!(state.struct_value, None);
    }

    // Reproduces the bug: GDB stores the breakpoint with an absolute path, but
    // the click queries with the path as known by the UI. The toggle must
    // find the existing breakpoint instead of creating a duplicate.
    #[test]
    fn breakpoint_at_finds_bp_despite_path_form() {
        let mut state = DebuggerState::new();
        state
            .persistent
            .breakpoints
            .push(bp(1, "/tmp/example.c", 8));

        assert!(state.breakpoint_at("/tmp/example.c", 8).is_some());
        assert!(state.breakpoint_at("example.c", 8).is_some());
        assert!(state.breakpoint_at("/tmp/example.c", 9).is_none());
    }

    // GDB relocates a breakpoint requested on the function name's line
    // (line 6) to the first executable line (line 7). A new click on line 6
    // must find it (via requested_line) and remove it, not duplicate it.
    #[test]
    fn breakpoint_at_matches_requested_and_resolved_line() {
        let mut state = DebuggerState::new();
        state.persistent.breakpoints.push(Breakpoint {
            id: 1,
            file: "/tmp/example.c".into(),
            line: 7,
            requested_line: Some(6),
            enabled: true,
            condition: None,
            condition_error: None,
        });

        // Click on the actual line where GDB placed it.
        assert!(state.breakpoint_at("example.c", 7).is_some());
        // Click on the function name's line (the requested one).
        assert!(state.breakpoint_at("example.c", 6).is_some());
        // An unrelated line does not match.
        assert!(state.breakpoint_at("example.c", 5).is_none());
    }

    // The visual marker must only appear on the actual line (7), not the
    // requested one (6), even though the toggle works from both.
    #[test]
    fn marker_only_on_resolved_line() {
        let mut state = DebuggerState::new();
        state.persistent.breakpoints.push(Breakpoint {
            id: 1,
            file: "/tmp/example.c".into(),
            line: 7,
            requested_line: Some(6),
            enabled: true,
            condition: None,
            condition_error: None,
        });

        assert!(state.has_breakpoint_marker("example.c", 7));
        assert!(!state.has_breakpoint_marker("example.c", 6));
    }

    // Requesting line 11 and line 12 can both resolve to line 12 with
    // different ids: the second one is a duplicate that must be discarded.
    #[test]
    fn detects_duplicate_at_resolved_line() {
        let mut state = DebuggerState::new();
        state.persistent.breakpoints.push(Breakpoint {
            id: 1,
            file: "/tmp/example.c".into(),
            line: 12,
            requested_line: Some(11),
            enabled: true,
            condition: None,
            condition_error: None,
        });

        // Same file+resolved line, different id → duplicate.
        let candidate = Breakpoint {
            id: 2,
            file: "/tmp/example.c".into(),
            line: 12,
            requested_line: Some(12),
            enabled: true,
            condition: None,
            condition_error: None,
        };
        assert!(state.is_duplicate_breakpoint(&candidate));

        // Different resolved line → not a duplicate.
        let other = Breakpoint {
            id: 3,
            file: "/tmp/example.c".into(),
            line: 8,
            requested_line: Some(8),
            enabled: true,
            condition: None,
            condition_error: None,
        };
        assert!(!state.is_duplicate_breakpoint(&other));

        // The same id (re-emission of the breakpoint itself) does not count as a duplicate.
        assert!(!state.is_duplicate_breakpoint(&bp(1, "/tmp/example.c", 12)));
    }

    // ── Watchpoints ──────────────────────────────────────────────────────────

    fn wp(id: u32, expr: &str, kind: WatchpointKind) -> Watchpoint {
        Watchpoint {
            id,
            expr: expr.into(),
            kind,
            enabled: true,
        }
    }

    #[test]
    fn watchpoint_added_pushes_new_row() {
        let mut state = DebuggerState::new();
        state.apply(StateEvent::WatchpointAdded {
            watchpoint: wp(5, "x", WatchpointKind::Write),
        });

        assert_eq!(state.persistent.watchpoints.len(), 1);
        let row = &state.persistent.watchpoints[0];
        assert_eq!(row.id, 5);
        assert_eq!(row.expr, "x");
        assert_eq!(row.kind, WatchpointKind::Write);
        assert!(row.enabled);
    }

    #[test]
    fn watchpoint_added_replaces_existing_by_id() {
        let mut state = DebuggerState::new();
        state
            .persistent
            .watchpoints
            .push(wp(5, "x", WatchpointKind::Write));

        state.apply(StateEvent::WatchpointAdded {
            watchpoint: wp(5, "x", WatchpointKind::Read),
        });

        assert_eq!(state.persistent.watchpoints.len(), 1);
        assert_eq!(state.persistent.watchpoints[0].kind, WatchpointKind::Read);
    }

    #[test]
    fn watchpoint_removed_drops_only_matching_id() {
        let mut state = DebuggerState::new();
        state
            .persistent
            .watchpoints
            .push(wp(1, "a", WatchpointKind::Write));
        state
            .persistent
            .watchpoints
            .push(wp(2, "b", WatchpointKind::Write));

        state.apply(StateEvent::WatchpointRemoved { id: 1 });

        assert_eq!(state.persistent.watchpoints.len(), 1);
        assert_eq!(state.persistent.watchpoints[0].id, 2);
    }

    #[test]
    fn watchpoint_toggled_flips_enabled_leaves_others_untouched() {
        let mut state = DebuggerState::new();
        state
            .persistent
            .watchpoints
            .push(wp(3, "x", WatchpointKind::Write));

        state.apply(StateEvent::WatchpointToggled {
            id: 3,
            enabled: false,
        });
        assert!(!state.persistent.watchpoints[0].enabled);

        state.apply(StateEvent::WatchpointToggled {
            id: 3,
            enabled: true,
        });
        assert!(state.persistent.watchpoints[0].enabled);
    }

    #[test]
    fn has_watchpoint_expr_true_for_existing_expr_only() {
        let mut state = DebuggerState::new();
        state
            .persistent
            .watchpoints
            .push(wp(1, "x", WatchpointKind::Write));

        assert!(state.has_watchpoint_expr("x"));
        assert!(!state.has_watchpoint_expr("y"));
    }

    #[test]
    fn watchpoint_error_sets_and_reads_by_expr() {
        let mut state = DebuggerState::new();
        state.apply(StateEvent::WatchpointError {
            expr: "nosuchvar".into(),
            message: "No symbol \"nosuchvar\" in current context.".into(),
        });

        assert_eq!(
            state.watchpoint_error("nosuchvar"),
            Some("No symbol \"nosuchvar\" in current context.")
        );
        assert_eq!(state.watchpoint_error("other"), None);
    }

    #[test]
    fn program_loaded_started_exited_wipe_watchpoint_errors() {
        let mut state = DebuggerState::new();
        state
            .errors
            .watchpoint_errors
            .insert("x".into(), "stale error".into());
        state.apply(StateEvent::ProgramLoaded {
            executable: "a.out".into(),
        });
        assert!(state.errors.watchpoint_errors.is_empty());

        let mut state = DebuggerState::new();
        state
            .errors
            .watchpoint_errors
            .insert("x".into(), "stale error".into());
        state.apply(StateEvent::ProgramStarted);
        assert!(state.errors.watchpoint_errors.is_empty());

        let mut state = DebuggerState::new();
        state
            .errors
            .watchpoint_errors
            .insert("x".into(), "stale error".into());
        state.apply(StateEvent::ProgramExited { code: Some(0) });
        assert!(state.errors.watchpoint_errors.is_empty());
    }

    // Rollback guarantee (design.md): the same lifecycle events must still
    // leave persisted breakpoints/watchpoints untouched — only the *_errors
    // maps are wiped.
    #[test]
    fn program_loaded_does_not_clear_persisted_watchpoints() {
        let mut state = DebuggerState::new();
        state
            .persistent
            .watchpoints
            .push(wp(1, "x", WatchpointKind::Write));

        state.apply(StateEvent::ProgramLoaded {
            executable: "a.out".into(),
        });

        assert_eq!(state.persistent.watchpoints.len(), 1);
    }

    // D6: a *stopped with reason="watchpoint-scope" must silently remove the
    // matching row (no toast, no separate StateEvent) while the pause
    // transition itself still happens.
    #[test]
    fn watchpoint_scope_stop_reason_silently_removes_row_on_pause() {
        let mut state = DebuggerState::new();
        state
            .persistent
            .watchpoints
            .push(wp(7, "local_var", WatchpointKind::Write));

        state.apply(StateEvent::ProgramPaused {
            pause: PauseState {
                thread_id: 1,
                frame: Frame {
                    addr: 0,
                    function: "??".into(),
                    file: None,
                    line: None,
                },
                stack: vec![],
                stop_reason: StopReason::WatchpointScope(7),
            },
        });

        assert!(state.persistent.watchpoints.is_empty());
        assert!(state.is_paused(), "the pause transition must still happen");
    }

    // Triangulation: an unrelated stop reason (e.g. a plain breakpoint hit)
    // must never touch the watchpoints list.
    #[test]
    fn breakpoint_hit_stop_reason_leaves_watchpoints_untouched() {
        let mut state = DebuggerState::new();
        state
            .persistent
            .watchpoints
            .push(wp(7, "local_var", WatchpointKind::Write));

        state.apply(StateEvent::ProgramPaused {
            pause: PauseState {
                thread_id: 1,
                frame: Frame {
                    addr: 0x1000,
                    function: "main".into(),
                    file: Some("a.c".into()),
                    line: Some(5),
                },
                stack: vec![],
                stop_reason: StopReason::BreakpointHit(1),
            },
        });

        assert_eq!(state.persistent.watchpoints.len(), 1);
    }

    // ── Catchpoints ─────────────────────────────────────────────────────────

    fn cp(id: u32, kind: CatchpointKind, args: &[&str]) -> Catchpoint {
        Catchpoint {
            id,
            kind,
            args: args.iter().map(|s| s.to_string()).collect(),
            enabled: true,
        }
    }

    #[test]
    fn catchpoint_kind_display_matches_gdb_catch_type_string() {
        assert_eq!(CatchpointKind::Fork.to_string(), "fork");
        assert_eq!(CatchpointKind::Vfork.to_string(), "vfork");
        assert_eq!(CatchpointKind::Exec.to_string(), "exec");
        assert_eq!(CatchpointKind::Signal.to_string(), "signal");
        assert_eq!(CatchpointKind::Load.to_string(), "load");
        assert_eq!(CatchpointKind::Unload.to_string(), "unload");
    }

    // Phase 2b: Syscall is the 7th CatchpointKind.
    #[test]
    fn syscall_kind_displays_as_syscall() {
        assert_eq!(CatchpointKind::Syscall.to_string(), "syscall");
    }

    // Phase 2c task 1.1: Throw/Rethrow/Catch are the 8th-10th CatchpointKind
    // variants, displaying their GDB `catch-type=` wire literal exactly like
    // every other kind.
    #[test]
    fn throw_rethrow_catch_kinds_display_matches_gdb_catch_type_string() {
        assert_eq!(CatchpointKind::Throw.to_string(), "throw");
        assert_eq!(CatchpointKind::Rethrow.to_string(), "rethrow");
        assert_eq!(CatchpointKind::Catch.to_string(), "catch");
    }

    // Phase 2b (D3): StopReason::SyscallTriggered carries an optional
    // number, optional name, and a phase distinguishing entry from return.
    // Exercises all 4 field-presence permutations plus both phases.
    #[test]
    fn syscall_triggered_entry_with_both_fields_present() {
        let reason = StopReason::SyscallTriggered {
            phase: SyscallPhase::Entry,
            number: Some("2".to_string()),
            name: Some("open".to_string()),
        };
        match reason {
            StopReason::SyscallTriggered {
                phase,
                number,
                name,
            } => {
                assert_eq!(phase, SyscallPhase::Entry);
                assert_eq!(number, Some("2".to_string()));
                assert_eq!(name, Some("open".to_string()));
            }
            _ => panic!("expected SyscallTriggered"),
        }
    }

    #[test]
    fn syscall_triggered_return_with_number_only() {
        let reason = StopReason::SyscallTriggered {
            phase: SyscallPhase::Return,
            number: Some("2".to_string()),
            name: None,
        };
        match reason {
            StopReason::SyscallTriggered {
                phase,
                number,
                name,
            } => {
                assert_eq!(phase, SyscallPhase::Return);
                assert_eq!(number, Some("2".to_string()));
                assert_eq!(name, None);
            }
            _ => panic!("expected SyscallTriggered"),
        }
    }

    #[test]
    fn syscall_triggered_entry_with_neither_field_present() {
        let reason = StopReason::SyscallTriggered {
            phase: SyscallPhase::Entry,
            number: None,
            name: None,
        };
        match reason {
            StopReason::SyscallTriggered {
                phase,
                number,
                name,
            } => {
                assert_eq!(phase, SyscallPhase::Entry);
                assert_eq!(number, None);
                assert_eq!(name, None);
            }
            _ => panic!("expected SyscallTriggered"),
        }
    }

    #[test]
    fn catchpoint_constructs_with_kind_args_and_enabled() {
        let c = cp(1, CatchpointKind::Signal, &["SIGINT"]);
        assert_eq!(c.id, 1);
        assert_eq!(c.kind, CatchpointKind::Signal);
        assert_eq!(c.args, vec!["SIGINT".to_string()]);
        assert!(c.enabled);
    }

    #[test]
    fn catchpoint_added_pushes_new_row() {
        let mut state = DebuggerState::new();
        state.apply(StateEvent::CatchpointAdded {
            catchpoint: cp(5, CatchpointKind::Fork, &[]),
        });

        assert_eq!(state.persistent.catchpoints.len(), 1);
        let row = &state.persistent.catchpoints[0];
        assert_eq!(row.id, 5);
        assert_eq!(row.kind, CatchpointKind::Fork);
        assert!(row.enabled);
    }

    // Phase 2b task 4.1 (verify-only): CatchpointAdded{Syscall,..} appends
    // via the existing generic replace-by-id logic — no per-kind branching
    // needed in `apply`.
    #[test]
    fn catchpoint_added_syscall_pushes_new_row() {
        let mut state = DebuggerState::new();
        state.apply(StateEvent::CatchpointAdded {
            catchpoint: cp(6, CatchpointKind::Syscall, &["open"]),
        });

        assert_eq!(state.persistent.catchpoints.len(), 1);
        let row = &state.persistent.catchpoints[0];
        assert_eq!(row.id, 6);
        assert_eq!(row.kind, CatchpointKind::Syscall);
        assert_eq!(row.args, vec!["open".to_string()]);
        assert!(row.enabled);
    }

    #[test]
    fn catchpoint_added_replaces_existing_by_id() {
        let mut state = DebuggerState::new();
        state
            .persistent
            .catchpoints
            .push(cp(5, CatchpointKind::Load, &["libfoo"]));

        state.apply(StateEvent::CatchpointAdded {
            catchpoint: cp(5, CatchpointKind::Load, &["libbar"]),
        });

        assert_eq!(state.persistent.catchpoints.len(), 1);
        assert_eq!(
            state.persistent.catchpoints[0].args,
            vec!["libbar".to_string()]
        );
    }

    // Phase 2c tasks 1.3/1.4 (verify-only — design deviation, see
    // apply-progress): D5's empty-args-preserving merge is unnecessary. The
    // Phase 0 spike found the regexp round-trips through a dedicated
    // `regexp=` field on EVERY ingress path (tokened `^done` and every
    // `=breakpoint-*` notify/re-emit), so `parse_catchpoint_field` always
    // recovers the correct args — there is no empty-args race to guard
    // against. `CatchpointAdded{Throw,..}` appends/replaces via the
    // existing generic replace-by-id logic, exactly like Phase 2b's
    // Syscall precedent — no per-kind branching needed in `apply`.
    #[test]
    fn catchpoint_added_throw_with_regexp_pushes_new_row() {
        let mut state = DebuggerState::new();
        state.apply(StateEvent::CatchpointAdded {
            catchpoint: cp(7, CatchpointKind::Throw, &["std::runtime_error"]),
        });

        assert_eq!(state.persistent.catchpoints.len(), 1);
        let row = &state.persistent.catchpoints[0];
        assert_eq!(row.id, 7);
        assert_eq!(row.kind, CatchpointKind::Throw);
        assert_eq!(row.args, vec!["std::runtime_error".to_string()]);
        assert!(row.enabled);
    }

    // A later re-emit (e.g. a `=breakpoint-modified` fired while the
    // program starts, verified live in the spike) always carries the same
    // `regexp=` field, so a replace-by-id never wipes it — no merge rule
    // needed, unlike the D4/D5 concern the design anticipated before the
    // spike ran.
    #[test]
    fn catchpoint_added_throw_reemit_with_same_regexp_does_not_wipe_it() {
        let mut state = DebuggerState::new();
        state
            .persistent
            .catchpoints
            .push(cp(7, CatchpointKind::Throw, &["std::runtime_error"]));

        state.apply(StateEvent::CatchpointAdded {
            catchpoint: cp(7, CatchpointKind::Throw, &["std::runtime_error"]),
        });

        assert_eq!(state.persistent.catchpoints.len(), 1);
        assert_eq!(
            state.persistent.catchpoints[0].args,
            vec!["std::runtime_error".to_string()]
        );
    }

    #[test]
    fn catchpoint_removed_drops_only_matching_id() {
        let mut state = DebuggerState::new();
        state
            .persistent
            .catchpoints
            .push(cp(1, CatchpointKind::Fork, &[]));
        state
            .persistent
            .catchpoints
            .push(cp(2, CatchpointKind::Vfork, &[]));

        state.apply(StateEvent::CatchpointRemoved { id: 1 });

        assert_eq!(state.persistent.catchpoints.len(), 1);
        assert_eq!(state.persistent.catchpoints[0].id, 2);
    }

    #[test]
    fn catchpoint_toggled_flips_enabled_leaves_others_untouched() {
        let mut state = DebuggerState::new();
        state
            .persistent
            .catchpoints
            .push(cp(3, CatchpointKind::Exec, &[]));

        state.apply(StateEvent::CatchpointToggled {
            id: 3,
            enabled: false,
        });
        assert!(!state.persistent.catchpoints[0].enabled);

        state.apply(StateEvent::CatchpointToggled {
            id: 3,
            enabled: true,
        });
        assert!(state.persistent.catchpoints[0].enabled);
    }

    #[test]
    fn has_catchpoint_true_for_existing_kind_and_args_only() {
        let mut state = DebuggerState::new();
        state
            .persistent
            .catchpoints
            .push(cp(1, CatchpointKind::Signal, &["SIGINT"]));

        assert!(has_catchpoint_helper(
            &state,
            CatchpointKind::Signal,
            &["SIGINT"]
        ));
        assert!(!has_catchpoint_helper(
            &state,
            CatchpointKind::Signal,
            &["SIGSEGV"]
        ));
        assert!(!has_catchpoint_helper(&state, CatchpointKind::Fork, &[]));
    }

    fn has_catchpoint_helper(state: &DebuggerState, kind: CatchpointKind, args: &[&str]) -> bool {
        let owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        state.has_catchpoint(kind, &owned)
    }

    #[test]
    fn catchpoint_error_sets_and_reads_by_key() {
        let mut state = DebuggerState::new();
        state.apply(StateEvent::CatchpointError {
            key: "signal:BOGUS".into(),
            message: "Undefined signal name BOGUS.".into(),
        });

        assert_eq!(
            state.catchpoint_error("signal:BOGUS"),
            Some("Undefined signal name BOGUS.")
        );
        assert_eq!(state.catchpoint_error("fork:"), None);
    }

    #[test]
    fn catchpoint_by_id_finds_only_matching_row() {
        let mut state = DebuggerState::new();
        state
            .persistent
            .catchpoints
            .push(cp(9, CatchpointKind::Load, &["libc"]));

        assert!(state.catchpoint_by_id(9).is_some());
        assert_eq!(
            state.catchpoint_by_id(9).unwrap().kind,
            CatchpointKind::Load
        );
        assert!(state.catchpoint_by_id(10).is_none());
    }

    #[test]
    fn program_loaded_started_exited_wipe_catchpoint_errors() {
        let mut state = DebuggerState::new();
        state
            .errors
            .catchpoint_errors
            .insert("fork:".into(), "stale error".into());
        state.apply(StateEvent::ProgramLoaded {
            executable: "a.out".into(),
        });
        assert!(state.errors.catchpoint_errors.is_empty());

        let mut state = DebuggerState::new();
        state
            .errors
            .catchpoint_errors
            .insert("fork:".into(), "stale error".into());
        state.apply(StateEvent::ProgramStarted);
        assert!(state.errors.catchpoint_errors.is_empty());

        let mut state = DebuggerState::new();
        state
            .errors
            .catchpoint_errors
            .insert("fork:".into(), "stale error".into());
        state.apply(StateEvent::ProgramExited { code: Some(0) });
        assert!(state.errors.catchpoint_errors.is_empty());
    }

    // Rollback guarantee (mirrors watchpoints): lifecycle events must leave
    // persisted catchpoints untouched — only the *_errors map is wiped.
    #[test]
    fn program_loaded_does_not_clear_persisted_catchpoints() {
        let mut state = DebuggerState::new();
        state
            .persistent
            .catchpoints
            .push(cp(1, CatchpointKind::Fork, &[]));

        state.apply(StateEvent::ProgramLoaded {
            executable: "a.out".into(),
        });

        assert_eq!(state.persistent.catchpoints.len(), 1);
    }
}
