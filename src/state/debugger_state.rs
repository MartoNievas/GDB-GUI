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

// ─── Disassembly ──────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct AsmLine {
    pub addr: u64,
    pub inst: String,
}

// ─── Stop reason ─────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub enum StopReason {
    BreakpointHit(u32),
    EndStepping,
    Signal(String),
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
            },
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
            }

            StateEvent::ProgramPaused { pause } => {
                self.program = ProgramState::Paused;
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
}
