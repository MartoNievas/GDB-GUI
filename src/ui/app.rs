use eframe::egui::{self, FontId, Margin, ScrollArea, Sense, Stroke, Vec2};
use std::sync::mpsc::{Receiver, Sender};

use super::command::Command;
use super::panels;
use super::theme::*;
use super::widgets::*;
use crate::state::{DebuggerEvent, DebuggerState, UiEvent};
use bitflags::bitflags;

// ─── Source line for rendering ─────────────────────────────────────────────────

struct SourceLine {
    number: u32,
    text: String,
}

// ─── Panel Flags for rendering ─────────────────────────────────────────────────

bitflags! {
    #[derive(Default)]
    pub struct PanelState: u32 {
        const BREAKPOINTS = 1 << 0;
        const WATCHPOINTS = 1 << 1;
        const CATCHPOINTS = 1 << 2;
        const COMMANDS    = 1 << 3;
        const STACK       = 1 << 4;
        const FILES       = 1 << 5;
        const THREAD      = 1 << 6;
        const STRUCT      = 1 << 7;
        const ATTACH      = 1 << 8;
        const REMOTE      = 1 << 9;
    }
}

// ─── App ──────────────────────────────────────────────────────────────────────

#[derive(Default, Debug, PartialEq, Clone, Copy)]
pub(crate) enum WatchTab {
    #[default]
    Watch,
    Registers,
    Data,
    Memory,
}

pub struct App {
    pub state: DebuggerState,
    event_rx: Receiver<DebuggerEvent>,
    cmd_tx: Sender<Command>,

    // UI state
    pub(crate) console_input: String,
    pub(crate) console_log: Vec<String>,
    pub(crate) watch_tab: WatchTab,

    // Collapsible sections
    pub(crate) panels: PanelState,

    source_lines: Vec<SourceLine>,
    source_file: Option<String>,

    // Condition column edit buffer, keyed by breakpoint id. Lazily seeded
    // from `bp.condition` the first time each row is rendered.
    pub(crate) bp_cond_buffer: std::collections::HashMap<u32, String>,

    // Struct panel expression edit buffer.
    pub(crate) struct_input: String,

    // Memory tab address edit buffer (D4) — the commit-on-blur draft;
    // `state.memory_addr` is the committed truth `refresh_thread_scoped_views`
    // reads.
    pub(crate) memory_input: String,

    // Value-cell edit buffer (Watch locals/globals + Registers), keyed by
    // EditTarget so the same key survives Command -> pending_edit ->
    // StateEvent -> edit_errors -> this buffer with no re-derivation.
    pub(crate) value_edit_buffer: std::collections::HashMap<crate::state::EditTarget, String>,

    // Watchpoints panel input row buffers.
    pub(crate) wp_expr_buffer: String,
    pub(crate) wp_kind_buffer: crate::state::WatchpointKind,

    // Catchpoints panel input row buffers.
    pub(crate) cp_kind_buffer: crate::state::CatchpointKind,
    pub(crate) cp_args_buffer: String,

    // Attach panel PID input buffer (design D1-D5).
    pub(crate) attach_pid_buffer: String,

    // Remote-connect panel host:port input buffer (design D1-D5, Slice 2).
    pub(crate) remote_target_buffer: String,

    // ── Session persistence (design decision D4/D6) ─────────────────────
    /// Resolved once at construction via `Store::from_env()`; `None` when
    /// the platform has no resolvable config directory (persistence
    /// disabled for the session rather than crashing). `pub(crate)` so
    /// tests can inject a temp-dir `Store` directly, mirroring
    /// `persistence.rs`'s own `Store { root: dir }` test pattern.
    pub(crate) store: Option<crate::state::Store>,
    /// Suppresses the save hook while a restore replay (Phase 3) is
    /// in-flight (design decision D6): state mid-replay holds only the
    /// entries restored so far, so saving then would truncate the file.
    /// Set `true` by `start_restore` and cleared by `finalize_restore`.
    pub(crate) restore_pending: bool,
    /// The in-flight restore replay (Phase 3, design D2/D6/D9), if any.
    /// `pub(crate)` for direct test injection/inspection.
    pub(crate) restore_session: Option<RestoreSession>,
    /// Failures from the most recently finalized restore, shown by
    /// `panels::restore_report`'s modal until the user picks `Keep` or
    /// `Remove N failed` (design decision D8 — never auto-dropped).
    /// `None`/empty means no report is pending.
    pub(crate) restore_report: Option<Vec<RestoreFailure>>,
    /// `true` for the rest of this session once `Store::load` reports the
    /// project file's `schema_version` is newer than this build understands
    /// (design decision D7: read-only quarantine) — the save hook checks
    /// this so a newer version's file is never clobbered.
    pub(crate) persistence_quarantined: bool,
    /// App-wide settings (currently only the persistence enable/disable
    /// switch), loaded once at construction and mutated by the topbar's
    /// "Persist" checkbox (spec: "Persistence Enable/Disable Setting").
    pub(crate) settings: crate::state::Settings,
    /// Resolved once at construction via `SettingsStore::from_env()`;
    /// `None` when the platform has no resolvable config directory —
    /// `settings` then stays in-memory only for the session (never
    /// persisted, mirroring `store`'s degrade-gracefully contract).
    pub(crate) settings_store: Option<crate::state::SettingsStore>,
}

// ─── Restore replay (Phase 3: design decisions D2, D6, D8, D9) ────────────────

/// One entry from a persisted `ProjectFile` still awaiting its replay
/// outcome. Carries just enough identity to match the `*Added` (success) or
/// `*Failed`/`*Error` (failure) `StateEvent` it produced back to the request
/// that produced it, and whether it should be toggled off once a fresh id
/// exists (design decision D9 — never toggled before the row is added).
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum RestoreEntry {
    /// Keyed by `same_file` + the requested line (design: "key by
    /// `same_file` + (`line`|`requested_line`)") — matches whichever of the
    /// two fields the reply carries a match on.
    Breakpoint { file: String, line: u32, enabled: bool },
    /// Keyed by exact `expr` (design: "key by exact `expr`").
    Watchpoint { expr: String, enabled: bool },
    /// Keyed by exact `(kind, args)` (design: "key by `(kind, args)`").
    Catchpoint {
        kind: crate::state::CatchpointKind,
        args: Vec<String>,
        enabled: bool,
    },
}

/// One failure to show in the restore report modal (design decision D8:
/// failures are reported, never silently dropped from the project file).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RestoreFailure {
    pub(crate) label: String,
    pub(crate) message: String,
}

/// Fallback deadline (design.md "Open Questions": "3 s restore-report
/// deadline is a guess; tune after first real run") — a hung/crashed GDB
/// reply must not suppress the save hook forever.
const RESTORE_DEADLINE: std::time::Duration = std::time::Duration::from_secs(3);

/// One in-flight restore replay, started by `start_restore` on
/// `StateEvent::ProgramLoaded` (design decision D2). `App.restore_pending`
/// is suppressing the save hook for as long as this exists.
#[derive(Debug)]
pub(crate) struct RestoreSession {
    /// Entries still awaiting a success/failure `StateEvent`.
    pub(crate) outstanding: Vec<RestoreEntry>,
    /// Failures observed so far.
    pub(crate) failures: Vec<RestoreFailure>,
    /// Wall-clock fallback: finalize even if some entries never reply.
    pub(crate) deadline: std::time::Instant,
}

/// The restore-relevant fact carried by one `StateEvent`, extracted BEFORE
/// `DebuggerState::apply` consumes it (mirrors every other flag captured in
/// `apply_state_event`, e.g. `was_paused`). `None` for every event a restore
/// never needs to correlate.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum RestoreSignal {
    BreakpointSuccess { file: String, line: u32, requested_line: Option<u32>, id: u32 },
    BreakpointFailure { file: String, line: u32, message: String },
    WatchpointSuccess { expr: String, id: u32 },
    WatchpointFailure { expr: String, message: String },
    CatchpointSuccess { kind: crate::state::CatchpointKind, args: Vec<String>, id: u32 },
    CatchpointFailure { key: String, message: String },
}

fn restore_signal_from_event(event: &crate::state::StateEvent) -> Option<RestoreSignal> {
    use crate::state::StateEvent;
    match event {
        StateEvent::BreakpointAdded { breakpoint } => Some(RestoreSignal::BreakpointSuccess {
            file: breakpoint.file.clone(),
            line: breakpoint.line,
            requested_line: breakpoint.requested_line,
            id: breakpoint.id,
        }),
        StateEvent::BreakpointInsertFailed { file, line, message } => {
            Some(RestoreSignal::BreakpointFailure {
                file: file.clone(),
                line: *line,
                message: message.clone(),
            })
        }
        StateEvent::WatchpointAdded { watchpoint } => Some(RestoreSignal::WatchpointSuccess {
            expr: watchpoint.expr.clone(),
            id: watchpoint.id,
        }),
        StateEvent::WatchpointError { expr, message } => Some(RestoreSignal::WatchpointFailure {
            expr: expr.clone(),
            message: message.clone(),
        }),
        StateEvent::CatchpointAdded { catchpoint } => Some(RestoreSignal::CatchpointSuccess {
            kind: catchpoint.kind,
            args: catchpoint.args.clone(),
            id: catchpoint.id,
        }),
        StateEvent::CatchpointError { key, message } => Some(RestoreSignal::CatchpointFailure {
            key: key.clone(),
            message: message.clone(),
        }),
        _ => None,
    }
}

/// Whether `entry` is the outstanding request that produced `signal`
/// (design: breakpoint keys by `same_file` + (`line`|`requested_line`),
/// watchpoint by exact `expr`, catchpoint by exact `(kind, args)`).
fn restore_entry_matches(entry: &RestoreEntry, signal: &RestoreSignal) -> bool {
    match (entry, signal) {
        (
            RestoreEntry::Breakpoint { file, line, .. },
            RestoreSignal::BreakpointSuccess {
                file: got_file,
                line: got_line,
                requested_line,
                ..
            },
        ) => {
            crate::state::same_file(file, got_file)
                && (*line == *got_line || Some(*line) == *requested_line)
        }
        (
            RestoreEntry::Breakpoint { file, line, .. },
            RestoreSignal::BreakpointFailure {
                file: got_file,
                line: got_line,
                ..
            },
        ) => crate::state::same_file(file, got_file) && *line == *got_line,
        (RestoreEntry::Watchpoint { expr, .. }, RestoreSignal::WatchpointSuccess { expr: got, .. }) => {
            expr == got
        }
        (RestoreEntry::Watchpoint { expr, .. }, RestoreSignal::WatchpointFailure { expr: got, .. }) => {
            expr == got
        }
        (
            RestoreEntry::Catchpoint { kind, args, .. },
            RestoreSignal::CatchpointSuccess {
                kind: got_kind,
                args: got_args,
                ..
            },
        ) => kind == got_kind && args == got_args,
        (RestoreEntry::Catchpoint { kind, args, .. }, RestoreSignal::CatchpointFailure { key, .. }) => {
            format!("{kind}:{}", args.join(",")) == *key
        }
        _ => false,
    }
}

/// Maps a persisted `BreakpointDTO` to the `Command` that replays it
/// (design decisions D2/D3): GDB assigns a fresh id on `-break-insert`, so
/// no id ever round-trips through the project file, and the DTO's `line`
/// is already the *requested* one (design D3 — set at save time from
/// `requested_line.unwrap_or(line)`).
pub(crate) fn breakpoint_dto_to_command(dto: &crate::state::BreakpointDTO) -> Command {
    Command::AddBreakpoint {
        file: dto.file.clone(),
        line: dto.line,
        condition: dto.condition.clone(),
    }
}

/// Maps a persisted `WatchpointDTO` to its replay `Command`. `None` for an
/// unrecognized `kind` wire literal (a project file from a future schema
/// version) — the entry is skipped rather than guessed at.
pub(crate) fn watchpoint_dto_to_command(dto: &crate::state::WatchpointDTO) -> Option<Command> {
    let kind = crate::state::WatchpointKind::from_wire(&dto.kind)?;
    Some(Command::AddWatchpoint {
        expr: dto.expr.clone(),
        kind,
    })
}

/// Maps a persisted `CatchpointDTO` to its replay `Command`. `None` for an
/// unrecognized `kind` wire literal, mirroring `watchpoint_dto_to_command`.
pub(crate) fn catchpoint_dto_to_command(dto: &crate::state::CatchpointDTO) -> Option<Command> {
    let kind = crate::state::CatchpointKind::from_wire(&dto.kind)?;
    Some(Command::AddCatchpoint {
        kind,
        args: dto.args.clone(),
    })
}

impl App {
    pub fn new(
        state: DebuggerState,
        event_rx: Receiver<DebuggerEvent>,
        cmd_tx: Sender<Command>,
    ) -> Self {
        let mut panels = PanelState::empty();
        panels.insert(PanelState::BREAKPOINTS);
        panels.insert(PanelState::STACK);

        Self {
            state,
            event_rx,
            cmd_tx,
            console_input: String::new(),
            console_log: Vec::new(),
            watch_tab: WatchTab::Watch,
            panels,
            source_lines: Vec::new(),
            source_file: None,
            bp_cond_buffer: std::collections::HashMap::new(),
            struct_input: String::new(),
            memory_input: String::new(),
            value_edit_buffer: std::collections::HashMap::new(),
            wp_expr_buffer: String::new(),
            wp_kind_buffer: crate::state::WatchpointKind::Write,
            cp_kind_buffer: crate::state::CatchpointKind::Fork,
            cp_args_buffer: String::new(),
            attach_pid_buffer: String::new(),
            remote_target_buffer: String::new(),
            store: crate::state::Store::from_env(),
            restore_pending: false,
            restore_session: None,
            restore_report: None,
            persistence_quarantined: false,
            settings: crate::state::SettingsStore::from_env()
                .map(|s| s.load())
                .unwrap_or_default(),
            settings_store: crate::state::SettingsStore::from_env(),
        }
    }

    pub(crate) fn send(&self, cmd: Command) {
        let _ = self.cmd_tx.send(cmd);
    }

    /// Refreshes every thread-scoped view: the roster itself plus locals,
    /// stack, registers, disasm, globals, and the struct expression — none
    /// of those commands are thread-qualified, so a thread switch (or a new
    /// pause) invalidates all of them equally. Called identically from both
    /// the pause path and the post-switch path (see design.md Data Flow) so
    /// the two call sites cannot drift apart.
    pub(crate) fn refresh_thread_scoped_views(&self) {
        self.send(Command::RequestThreads);
        self.send(Command::RequestLocals);
        self.send(Command::RequestStack);
        self.send(Command::RequestRegisters);
        self.send(Command::RequestDisasm);
        for name in self.state.global_names.clone() {
            self.send(Command::EvaluateGlobal(name));
        }
        if !self.state.struct_expr.is_empty() {
            self.send(Command::Evaluate(self.state.struct_expr.clone()));
        }
        // R8: a previously committed memory address is auto-re-fetched on
        // every pause/thread-switch, mirroring the struct_expr guard above.
        if !self.state.memory_addr.is_empty() {
            self.send(Command::RequestMemory {
                address: self.state.memory_addr.clone(),
                count: 256,
            });
        }
    }

    /// Applies one `StateEvent` to `self.state` and dispatches whatever
    /// follow-up commands that transition requires. Extracted out of
    /// `update()`'s event pump so it can be driven directly in tests without
    /// an `egui::Context`/`eframe::Frame` (mirrors the `was_paused` idiom:
    /// flags are captured from the event BEFORE `apply()` consumes it, then
    /// used to decide what to send AFTER state reflects the new reality).
    pub(crate) fn apply_state_event(&mut self, s: crate::state::StateEvent) {
        // GDB can create a second breakpoint on the same resolved line
        // (e.g. requesting line 11 and line 12, both landing on 12). We
        // discard the redundant one and remove it from GDB to avoid duplicates.
        if let crate::state::StateEvent::BreakpointAdded { breakpoint } = &s {
            if self.state.is_duplicate_breakpoint(breakpoint) {
                self.send(Command::RemoveBreakpoint(breakpoint.id));
                return;
            }
        }

        let was_paused = matches!(s, crate::state::StateEvent::ProgramPaused { .. });
        let was_loaded = matches!(s, crate::state::StateEvent::ProgramLoaded { .. });
        // Design D1/D4/D5: a distinct flag from `was_loaded` — `was_loaded`
        // matches `StateEvent::ProgramLoaded` only, so `try_start_restore`
        // and `Command::ProbeMainSource` below stay structurally
        // unreachable from `ProcessAttached` (D4's restore guard is
        // unbypassable by construction, not by an added check here).
        let was_attached = matches!(s, crate::state::StateEvent::ProcessAttached { .. });
        // Design D1/D3/D4 (Slice 2, task 7.2): mirrors `was_attached`'s
        // precedent for a distinct, structurally-unreachable-from-restore
        // flag — no `*stopped` is guaranteed after `^connected`, so this
        // branch must explicitly send both the names requests AND
        // `refresh_thread_scoped_views()` (unlike `was_attached`, which
        // relies on the following `*stopped` for the latter).
        let connected_remote = matches!(s, crate::state::StateEvent::RemoteConnected { .. });
        // Captured for the restore hook (Phase 3, design D2): the executable
        // this `ProgramLoaded` names, so `try_start_restore` can load its
        // project file once `s` has been consumed into `self.state`.
        let loaded_executable = if let crate::state::StateEvent::ProgramLoaded { executable } = &s {
            Some(executable.clone())
        } else {
            None
        };
        // Captured for restore correlation (Phase 3, design D2/D6/D9):
        // whether this event is the success/failure reply to an outstanding
        // replayed entry. `None` when no restore is relevant to this event
        // shape at all (cheap to compute regardless of whether a restore is
        // actually in flight — `handle_restore_signal` is the no-op guard).
        let restore_signal = restore_signal_from_event(&s);
        let thread_selected = matches!(s, crate::state::StateEvent::ThreadSelected { .. });
        let new_global_names = if let crate::state::StateEvent::GlobalNamesReceived { names } = &s {
            Some(names.clone())
        } else {
            None
        };
        // -gdb-set's ^done carries no value: the re-fetch below is the sole
        // source of truth for the refreshed cell (spec "Explicit Refresh
        // After Successful Write"). Captured before apply() clears the
        // matching edit_errors entry, mirroring was_paused/thread_selected.
        let succeeded_target = if let crate::state::StateEvent::ValueEditSucceeded { target } = &s {
            Some(target.clone())
        } else {
            None
        };
        // Captured BEFORE apply() consumes `s`, mirroring every other flag
        // above (was_paused, was_loaded, ...) — the decision to save is
        // about the mutation this event represents, not about the state
        // apply() leaves behind. Gated on three independent reasons to skip
        // (Phase 3): an in-flight restore (D6), a quarantined newer-schema
        // session (D7), and the user's persistence toggle (spec "Persistence
        // Enable/Disable Setting").
        let should_save = !self.restore_pending
            && !self.persistence_quarantined
            && self.persistence_effectively_enabled()
            && crate::state::mutates_tracepoints(&s);

        self.state.apply(s);
        self.load_source_if_needed();
        if should_save {
            self.save_persistent_state();
        }
        if let Some(signal) = restore_signal {
            self.handle_restore_signal(signal);
        }
        if was_loaded {
            self.send(Command::RequestRegisterNames);
            self.send(Command::RequestGlobalNames);
            // One-shot probe so the source view has something to show
            // before the user hits Run — see design "Preload main's source
            // at load time". Its reply never crosses into a `Command`,
            // `Breakpoint`, or panel row (process.rs intercepts it by MI
            // token before parse_line).
            self.send(Command::ProbeMainSource);
            // Phase 3 (design D2): a new executable resets last session's
            // schema-version quarantine (D7) — a fresh load deserves its own
            // determination, not the previous executable's stale one.
            self.persistence_quarantined = false;
            if let Some(exe) = loaded_executable {
                if self.persistence_effectively_enabled() {
                    self.try_start_restore(&exe);
                }
            }
        }
        if was_attached {
            // D5: only the register/global-names requests — no
            // `ProbeMainSource` (targets a not-yet-started program, which
            // attach never is) and no `try_start_restore` (D4: structurally
            // unreachable, since this branch is not `if was_loaded`).
            self.send(Command::RequestRegisterNames);
            self.send(Command::RequestGlobalNames);
        }
        if connected_remote {
            // D3/D4: no `ProbeMainSource` (mirrors `was_attached` — the
            // remote target is never a not-yet-started program) and no
            // `try_start_restore` (structurally unreachable: this branch is
            // not `if was_loaded`).
            self.send(Command::RequestRegisterNames);
            self.send(Command::RequestGlobalNames);
            self.refresh_thread_scoped_views();
        }
        if was_paused {
            self.refresh_thread_scoped_views();
        }
        if thread_selected {
            self.refresh_thread_scoped_views();
        }
        if let Some(names) = new_global_names {
            for name in names {
                self.send(Command::EvaluateGlobal(name));
            }
        }
        if let Some(target) = succeeded_target {
            match target {
                crate::state::EditTarget::Local(_) => self.send(Command::RequestLocals),
                crate::state::EditTarget::Global(name) => self.send(Command::EvaluateGlobal(name)),
                crate::state::EditTarget::Register(_) => self.send(Command::RequestRegisters),
            }
        }
    }

    /// Writes the current persisted breakpoints/watchpoints/catchpoints to
    /// disk via `self.store`, if one is available. A no-op when no
    /// executable is loaded yet — `ProjectFile::from_persistent` would
    /// otherwise serialize an empty `executable` string, which
    /// `Store::save`/`project_path` would hash into a bogus, unnamed
    /// project file. IO failures are logged to the console (mirroring
    /// `load_source_if_needed`'s degrade-gracefully pattern): a failed save
    /// must never panic or block the UI.
    fn save_persistent_state(&mut self) {
        let Some(store) = &self.store else { return };
        if self.state.persistent.executable.is_none() {
            return;
        }
        let project_file = crate::state::ProjectFile::from_persistent(&self.state.persistent);
        if let Err(e) = store.save(&project_file) {
            self.console_log
                .push(format!("[UI] ✗ Failed to save project file: {e}"));
        }
    }

    /// Whether persistence is effectively enabled right now (spec
    /// "Persistence Enable/Disable Setting"): the `GDB_GUI_NO_PERSIST`
    /// environment variable's mere presence always wins, regardless of the
    /// persisted setting.
    fn persistence_effectively_enabled(&self) -> bool {
        let env = std::env::var("GDB_GUI_NO_PERSIST").ok();
        crate::state::persistence_enabled(&self.settings, env.as_deref())
    }

    /// Flips the persistence toggle (topbar "Persist" checkbox) and
    /// persists the new setting immediately, mirroring
    /// `save_persistent_state`'s degrade-gracefully IO-error handling.
    pub(crate) fn set_persistence_enabled(&mut self, enabled: bool) {
        self.settings.persistence_enabled = enabled;
        if let Some(store) = &self.settings_store {
            if let Err(e) = store.save(&self.settings) {
                self.console_log
                    .push(format!("[UI] ✗ Failed to save settings: {e}"));
            }
        }
    }

    /// Loads the project file for `exe` (design decision D2) and starts a
    /// replay on a well-formed match. Every other `LoadOutcome` degrades
    /// silently (spec "Missing or Corrupt File Handling" / design D7):
    /// `Absent` = nothing to restore; `Corrupt` = warn and start clean
    /// (still allows a future save to overwrite it); `TooNew` = warn and
    /// quarantine this session's saves so a newer version's file is never
    /// clobbered.
    fn try_start_restore(&mut self, exe: &str) {
        let Some(store) = &self.store else { return };
        match store.load(exe) {
            crate::state::LoadOutcome::Loaded(project_file) => {
                self.start_restore(&project_file);
            }
            crate::state::LoadOutcome::Absent => {}
            crate::state::LoadOutcome::Corrupt(msg) => {
                self.console_log
                    .push(format!("[UI] ⚠ Project file corrupt, starting clean: {msg}"));
            }
            crate::state::LoadOutcome::TooNew(version) => {
                self.console_log.push(format!(
                    "[UI] ⚠ Project file schema v{version} is newer than this build supports — persistence disabled for this session"
                ));
                self.persistence_quarantined = true;
            }
        }
    }

    /// Starts a restore replay for `project_file` (design decision D2):
    /// dispatches an Add command for every entry regardless of its
    /// persisted `enabled` flag — GDB assigns a fresh id on insert, so a
    /// disabled entry must exist before it can be toggled off (design
    /// decision D9) — and arms `restore_pending`/`restore_session` so the
    /// save hook stays suppressed until every entry's outcome is known.
    fn start_restore(&mut self, project_file: &crate::state::ProjectFile) {
        let mut outstanding = Vec::new();

        for bp in &project_file.breakpoints {
            self.send(breakpoint_dto_to_command(bp));
            outstanding.push(RestoreEntry::Breakpoint {
                file: bp.file.clone(),
                line: bp.line,
                enabled: bp.enabled,
            });
        }
        for wp in &project_file.watchpoints {
            match watchpoint_dto_to_command(wp) {
                Some(cmd) => {
                    self.send(cmd);
                    outstanding.push(RestoreEntry::Watchpoint {
                        expr: wp.expr.clone(),
                        enabled: wp.enabled,
                    });
                }
                None => self.console_log.push(format!(
                    "[UI] ⚠ Skipped restoring watchpoint '{}': unrecognized kind '{}'",
                    wp.expr, wp.kind
                )),
            }
        }
        for cp in &project_file.catchpoints {
            match catchpoint_dto_to_command(cp) {
                Some(cmd) => {
                    self.send(cmd);
                    outstanding.push(RestoreEntry::Catchpoint {
                        // `catchpoint_dto_to_command` returning `Some` already
                        // proves `cp.kind` parsed — reusing it here (instead
                        // of destructuring `cmd`) keeps this a plain data
                        // build, not a pattern match on the Command shape.
                        kind: crate::state::CatchpointKind::from_wire(&cp.kind)
                            .expect("catchpoint_dto_to_command returned Some"),
                        args: cp.args.clone(),
                        enabled: cp.enabled,
                    });
                }
                None => self.console_log.push(format!(
                    "[UI] ⚠ Skipped restoring catchpoint: unrecognized kind '{}'",
                    cp.kind
                )),
            }
        }

        if outstanding.is_empty() {
            // Nothing to replay (an empty project file) — no in-flight
            // entries, so there is nothing for the save hook to wait on.
            return;
        }

        self.restore_pending = true;
        self.restore_session = Some(RestoreSession {
            outstanding,
            failures: Vec::new(),
            deadline: std::time::Instant::now() + RESTORE_DEADLINE,
        });
    }

    /// Correlates one restore-relevant `StateEvent` (already turned into a
    /// `RestoreSignal` before `apply()` consumed it) against the in-flight
    /// session's outstanding entries. A no-op when no restore is in flight,
    /// or when `signal` doesn't match anything still outstanding (e.g. a
    /// normal, non-restore add/error that merely shares the same event
    /// shape).
    fn handle_restore_signal(&mut self, signal: RestoreSignal) {
        let Some(session) = self.restore_session.as_mut() else {
            return;
        };
        let Some(idx) = session
            .outstanding
            .iter()
            .position(|entry| restore_entry_matches(entry, &signal))
        else {
            return;
        };
        let entry = session.outstanding.remove(idx);

        let mut toggle_off: Option<Command> = None;
        match &signal {
            RestoreSignal::BreakpointSuccess { id, .. } => {
                if matches!(entry, RestoreEntry::Breakpoint { enabled: false, .. }) {
                    toggle_off = Some(Command::ToggleBreakpoint {
                        id: *id,
                        enable: false,
                    });
                }
            }
            RestoreSignal::WatchpointSuccess { id, .. } => {
                if matches!(entry, RestoreEntry::Watchpoint { enabled: false, .. }) {
                    toggle_off = Some(Command::ToggleWatchpoint {
                        id: *id,
                        enable: false,
                    });
                }
            }
            RestoreSignal::CatchpointSuccess { id, .. } => {
                if matches!(entry, RestoreEntry::Catchpoint { enabled: false, .. }) {
                    toggle_off = Some(Command::ToggleCatchpoint {
                        id: *id,
                        enable: false,
                    });
                }
            }
            RestoreSignal::BreakpointFailure { file, line, message } => {
                session.failures.push(RestoreFailure {
                    label: format!("Breakpoint {file}:{line}"),
                    message: message.clone(),
                });
            }
            RestoreSignal::WatchpointFailure { expr, message } => {
                session.failures.push(RestoreFailure {
                    label: format!("Watchpoint {expr}"),
                    message: message.clone(),
                });
            }
            RestoreSignal::CatchpointFailure { key, message } => {
                session.failures.push(RestoreFailure {
                    label: format!("Catchpoint {key}"),
                    message: message.clone(),
                });
            }
        }

        if let Some(cmd) = toggle_off {
            self.send(cmd);
        }
        self.finalize_restore_if_done();
    }

    /// Finalizes the in-flight restore if every entry has replied, or if the
    /// fallback deadline has elapsed (design.md: "3 s restore-report
    /// deadline ... checked in `update()`" — also checked reactively here so
    /// a fully-resolved session doesn't wait for the next frame). A no-op
    /// when no restore is in flight or it isn't done yet.
    fn finalize_restore_if_done(&mut self) {
        let Some(session) = &self.restore_session else {
            return;
        };
        let done = session.outstanding.is_empty() || std::time::Instant::now() >= session.deadline;
        if done {
            self.finalize_restore();
        }
    }

    /// Clears the in-flight restore session, resumes the save hook, and — if
    /// any entries failed — arms the restore report modal (design decision
    /// D8: failures are surfaced, never silently dropped from the project
    /// file).
    fn finalize_restore(&mut self) {
        let Some(session) = self.restore_session.take() else {
            return;
        };
        self.restore_pending = false;
        if !session.failures.is_empty() {
            self.restore_report = Some(session.failures);
        }
    }

    /// "Keep" button (design decision D8): dismiss the report without
    /// touching the project file — the failed entries stay on disk for a
    /// retry on the next launch.
    pub(crate) fn dismiss_restore_report(&mut self) {
        self.restore_report = None;
    }

    /// "Remove N failed" button (design decision D8): explicitly rewrite the
    /// project file from the current in-memory state. Failed entries were
    /// never added to `self.state.persistent` in the first place, so this
    /// naturally drops them from the file without any separate filtering
    /// step.
    pub(crate) fn remove_failed_restore_entries(&mut self) {
        self.save_persistent_state();
        self.restore_report = None;
    }

    fn load_source_if_needed(&mut self) {
        let target_file = match self.state.source_view_file() {
            Some(f) => f.to_owned(),
            None => {
                self.source_lines.clear();
                self.source_file = None;
                return;
            }
        };

        if self.source_file.as_deref() == Some(&target_file) {
            return;
        }

        let content = self.try_load_source(&target_file);

        match content {
            Some(text) => {
                self.source_lines = text
                    .lines()
                    .enumerate()
                    .map(|(i, line)| SourceLine {
                        number: (i + 1) as u32,
                        text: line.to_owned(),
                    })
                    .collect();
                self.source_file = Some(target_file.clone());
                self.console_log.push(format!(
                    "[UI] ✓ Loaded {} ({} lines)",
                    target_file,
                    self.source_lines.len()
                ));
            }
            None => {
                self.console_log
                    .push(format!("[UI] ✗ Could not find source file: {target_file}"));
                self.console_log.push("[UI] Tried:".into());
                self.console_log.push(format!("  1. {target_file}"));
                if let Some(filename) = std::path::Path::new(&target_file).file_name() {
                    self.console_log
                        .push(format!("  2. {}", filename.to_string_lossy()));
                    self.console_log
                        .push(format!("  3. src/{}", filename.to_string_lossy()));
                }
                self.source_lines.clear();
                self.source_file = None;
            }
        }
    }

    fn try_load_source(&self, path: &str) -> Option<String> {
        // 1. Try the path as-is (absolute or relative from CWD)
        if let Ok(content) = std::fs::read_to_string(path) {
            return Some(content);
        }

        if let Some(filename) = std::path::Path::new(path).file_name() {
            if let Ok(content) = std::fs::read_to_string(filename) {
                return Some(content);
            }
        }

        if let Some(filename) = std::path::Path::new(path).file_name() {
            let src_path = format!("src/{}", filename.to_string_lossy());
            if let Ok(content) = std::fs::read_to_string(&src_path) {
                return Some(content);
            }
        }

        None
    }
}

// ─── Shutdown Release (design D6, generalizing "Detach on Exit") ──────────

/// Bound on how long `on_exit` waits for GDB's `-target-detach`/
/// `-target-disconnect` `^done`/`^error` before giving up — a hung or
/// already-dead GDB must not hang GUI shutdown indefinitely; on timeout the
/// pre-existing kill-on-exit behavior in `process.rs::run_loop` is the
/// fallback, not a new risk.
const DETACH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

/// What `on_exit` must release before the GUI closes (design D6) — a local
/// attach or an active remote connection, never both (mutually exclusive by
/// construction: `attach_enabled`/`remote_connect_enabled` each require the
/// other to be absent).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ShutdownRelease {
    /// A process is attached locally — `on_exit` must send
    /// `Command::DetachForShutdown`.
    Detach { pid: u32 },
    /// A remote target is connected — `on_exit` must send
    /// `Command::DisconnectForShutdown`.
    Disconnect { target: String },
}

/// Outcome of `wait_for_release_ack`: whether the release
/// (`-target-detach`/`-target-disconnect`) was acknowledged before the
/// deadline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReleaseAck {
    /// `StateEvent::DetachFinished` or `StateEvent::RemoteDisconnected`
    /// arrived before the deadline.
    Finished,
    /// The deadline elapsed with no matching event observed.
    TimedOut,
    /// `event_rx`'s sender was dropped (the GDB thread died) before a
    /// matching event arrived — keeps meaning *channel* disconnect (design
    /// D6), distinct from `StateEvent::RemoteDisconnected`.
    Disconnected,
}

/// Blocks, bounded by `timeout`, until `StateEvent::DetachFinished` or
/// `StateEvent::RemoteDisconnected` arrives on `rx` (design D6: shared by
/// both shutdown branches) — discarding every other event along the way
/// (`on_exit` has no UI left to route them to at this point anyway). Loops
/// `recv_timeout` against a single deadline rather than re-arming a fresh
/// `timeout` per iteration, so a stream of unrelated events cannot extend
/// the wait past `timeout`. Never blocks unbounded: a hung/dead GDB
/// resolves via `TimedOut`/`Disconnected`.
pub(crate) fn wait_for_release_ack(
    rx: &Receiver<DebuggerEvent>,
    timeout: std::time::Duration,
) -> ReleaseAck {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            return ReleaseAck::TimedOut;
        }
        match rx.recv_timeout(remaining) {
            Ok(DebuggerEvent::State(crate::state::StateEvent::DetachFinished { .. }))
            | Ok(DebuggerEvent::State(crate::state::StateEvent::RemoteDisconnected { .. })) => {
                return ReleaseAck::Finished;
            }
            Ok(_) => continue,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => return ReleaseAck::TimedOut,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                return ReleaseAck::Disconnected;
            }
        }
    }
}

/// What `on_exit` must release before the GUI closes — `None` unless a
/// process is attached locally or a remote target is connected (design D6,
/// generalizing "Detach on Exit" step 1). A local attach takes precedence
/// if somehow both were set (structurally shouldn't happen: `attach_enabled`
/// requires `remote_target.is_none()` and vice versa).
pub(crate) fn shutdown_release(state: &DebuggerState) -> Option<ShutdownRelease> {
    if let Some(pid) = state.attached_pid {
        Some(ShutdownRelease::Detach { pid })
    } else {
        state
            .remote_target
            .clone()
            .map(|target| ShutdownRelease::Disconnect { target })
    }
}

// ─── eframe::App ──────────────────────────────────────────────────────────────

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        apply_theme(ctx);

        while let Ok(event) = self.event_rx.try_recv() {
            match event {
                DebuggerEvent::State(s) => {
                    self.apply_state_event(s);
                }
                DebuggerEvent::Ui(UiEvent::ConsoleOutput(text)) => {
                    self.console_log.push(text);
                }
                DebuggerEvent::Ui(UiEvent::GdbError(err)) => {
                    self.console_log.push(format!("[ERROR] {err}"));
                }
            }
        }

        // Phase 3 (design.md: "3 s restore-report deadline ... checked in
        // `update()`"): a restore with no more incoming GDB replies (e.g. a
        // crashed/hung child) must still finalize eventually instead of
        // suppressing the save hook forever.
        self.finalize_restore_if_done();

        ctx.request_repaint();

        // ── TOP BAR ───────────────────────────────────────────────────────────
        egui::TopBottomPanel::top("top_bar")
            .frame(flat(BG_TOPBAR).inner_margin(Margin {
                left: 8,
                right: 8,
                top: 4,
                bottom: 4,
            }))
            .show(ctx, |ui| {
                panels::topbar::render(self, ui);
            });

        // ── RESTORE REPORT (modal, Phase 3 task 3.10) ───────────────────────────
        panels::restore_report::render(self, ctx);

        // ── CONSOLE (bottom) ──────────────────────────────────────────────────
        egui::TopBottomPanel::bottom("console")
            .resizable(true)
            .min_height(100.0)
            .default_height(300.0)
            .frame(flat(BG_CONSOLE))
            .show(ctx, |ui| {
                panels::console::render(self, ui, ctx);
            });

        // ── RIGHT PANEL ───────────────────────────────────────────────────────
        egui::SidePanel::right("right_panel")
            .resizable(true)
            .min_width(500.0)
            .default_width(500.0)
            .frame(flat(BG_PANEL))
            .show(ctx, |ui| {
                // Upper collapsible sections — drag the divider below to resize
                egui::TopBottomPanel::top("right_upper_panel")
                    .resizable(true)
                    .default_height(ui.available_height() * 0.52)
                    .frame(flat(BG_PANEL))
                    .show_inside(ui, |ui| {
                        ScrollArea::vertical()
                            .id_salt("right_upper")
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                ui.set_min_width(ui.available_width());

                                // BREAKPOINTS ──────────────────────────────────────────
                                panels::breakpoints::render(self, ui);

                                // WATCHPOINTS ──────────────────────────────────────────
                                panels::watchpoints::render(self, ui);

                                // CATCHPOINTS ──────────────────────────────────────────
                                panels::catchpoints::render(self, ui);

                                // ATTACH ────────────────────────────────────────────────
                                panels::attach::render(self, ui);

                                // REMOTE ────────────────────────────────────────────────
                                panels::remote::render(self, ui);

                                // COMMANDS ──────────────────────────────────────────────
                                panels::commands::render(self, ui);

                                // STRUCT ────────────────────────────────────────────────
                                panels::struct_panel::render(self, ui);

                                // STACK ─────────────────────────────────────────────────
                                panels::stack::render(self, ui);

                                // FILES ─────────────────────────────────────────────────
                                panels::files::render(self, ui);

                                // THREAD ────────────────────────────────────────────────
                                panels::thread::render(self, ui);
                            });
                    });

                // Watch / Registers / Data tabs — fill the remaining space
                egui::CentralPanel::default()
                    .frame(flat(BG_PANEL))
                    .show_inside(ui, |ui| {
                        panels::watch::render(self, ui);
                    });
            });

        // ── SOURCE VIEW (central) ─────────────────────────────────────────────
        egui::CentralPanel::default()
            .frame(flat(BG_APP))
            .show(ctx, |ui| {
                ScrollArea::both()
                    .id_salt("source")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        if self.source_lines.is_empty() {
                            ui.centered_and_justified(|ui| {
                                ui.label(m("No source file loaded", 13.0, TXT_DIM).italics());
                            });
                            return;
                        }

                        let current_line = self.state.current_line();

                        for line in &self.source_lines {
                            let is_current = Some(line.number) == current_line;
                            let file_ref = self.source_file.as_deref().unwrap_or("");
                            // The marker is only drawn on the actual line where GDB stops;
                            // the toggle also accepts the requested line (in case GDB relocated it).
                            let has_bp = self.state.has_breakpoint_marker(file_ref, line.number);
                            let bp_id = self
                                .state
                                .breakpoint_at(file_ref, line.number)
                                .map(|b| b.id);

                            let response =
                                source_row(ui, line.number, &line.text, is_current, has_bp);
                            if response.clicked() {
                                if let Some(id) = bp_id {
                                    self.send(Command::RemoveBreakpoint(id));
                                } else if let Some(file) = self.source_file.clone() {
                                    self.send(Command::AddBreakpoint {
                                        file,
                                        line: line.number,
                                        condition: None,
                                    });
                                }
                            }
                        }
                    });
            });
    }

    /// Shutdown release (design D6, generalizing "Detach on Exit"): closing
    /// the GUI while attached or remote-connected releases the inferior/
    /// target instead of leaving it stopped-and-traced, or killed outright
    /// once `event_rx` drops and `process.rs::run_loop`'s kill path becomes
    /// reachable. Kept a thin delegation to `shutdown_release`/
    /// `wait_for_release_ack` so the untestable surface — this lifecycle
    /// hook actually firing — stays minimal; the decision logic itself is
    /// unit-tested without a real `eframe::App`. `eframe` 0.33.3's default
    /// `glow` feature is what fixes this exact signature
    /// (`Option<&glow::Context>`); the no-`glow` alternative takes no
    /// parameter at all.
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        let Some(release) = shutdown_release(&self.state) else {
            return;
        };
        // GDB in synchronous MI mode does not read stdin while the inferior
        // is running, so a piped -target-detach/-target-disconnect is never
        // consumed without interrupting first.
        if self.state.is_running() {
            self.send(Command::Interrupt);
        }
        let label = match &release {
            ShutdownRelease::Detach { pid } => {
                self.send(Command::DetachForShutdown);
                format!("pid {pid}")
            }
            ShutdownRelease::Disconnect { target } => {
                self.send(Command::DisconnectForShutdown);
                format!("target {target}")
            }
        };
        match wait_for_release_ack(&self.event_rx, DETACH_TIMEOUT) {
            ReleaseAck::Finished => {}
            // The console panel is already gone by now, so a failed
            // release is reported loudly to stderr instead of silently —
            // the pre-existing kill-on-exit behavior is the fallback here,
            // not a new regression.
            ReleaseAck::TimedOut => {
                eprintln!("[gdb-gui] release timed out waiting for GDB to release {label}");
            }
            ReleaseAck::Disconnected => {
                eprintln!(
                    "[gdb-gui] release failed: GDB thread disconnected before releasing {label}"
                );
            }
        }
    }
}

// ─── Source row ───────────────────────────────────────────────────────────────

fn source_row(
    ui: &mut egui::Ui,
    line_no: u32,
    code: &str,
    is_current: bool,
    has_bp: bool,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(
        Vec2::new(f32::max(ui.available_width(), 900.0), 18.0),
        Sense::click(),
    );

    if response.hovered() {
        ui.ctx()
            .output_mut(|o| o.cursor_icon = egui::CursorIcon::PointingHand);
    }

    let p = ui.painter();
    let cy = rect.center().y;

    if is_current {
        p.rect_filled(rect, 0.0, BG_LINE_HL);
        p.line_segment(
            [rect.left_top(), rect.left_bottom()],
            Stroke::new(2.0_f32, ACCENT),
        );
    }

    if has_bp {
        p.circle_filled(egui::pos2(rect.left() + 9.0, cy), 5.0, RED);
    }

    // Line number – right-aligned in a 56 px gutter
    p.text(
        egui::pos2(rect.left() + 56.0, cy),
        egui::Align2::RIGHT_CENTER,
        format!("{line_no}"),
        FontId::monospace(12.0),
        if has_bp { RED } else { TXT_DIM },
    );

    // Code
    p.text(
        egui::pos2(rect.left() + 66.0, cy),
        egui::Align2::LEFT_CENTER,
        code,
        FontId::monospace(12.5),
        if is_current { TXT_HL } else { TXT },
    );

    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::DebuggerState;
    use std::sync::mpsc;

    fn test_app() -> (App, Receiver<Command>) {
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (_event_tx, event_rx) = mpsc::channel();
        let mut app = App::new(DebuggerState::new(), event_rx, cmd_tx);
        // App::new() reads real settings via SettingsStore::from_env(), so a
        // developer machine's actual ~/.config/gdb-gui/settings.toml (e.g.
        // persistence_enabled = false, left over from manual runs) would
        // otherwise leak into every test. Tests that inject a temp-dir
        // `store` already assume persistence is on by default; tests that
        // need it off set `app.settings.persistence_enabled = false`
        // explicitly afterward.
        app.settings.persistence_enabled = true;
        (app, cmd_rx)
    }

    // ── Persistence save hook (Phase 2: Wiring & Sanitization) ──────────────

    fn unique_temp_dir(tag: &str) -> std::path::PathBuf {
        let mut dir = std::env::temp_dir();
        dir.push(format!(
            "gdb-gui-app-persistence-test-{tag}-{}-{:?}",
            std::process::id(),
            std::time::Instant::now()
        ));
        dir
    }

    #[test]
    fn app_new_defaults_restore_pending_to_false() {
        let (app, _cmd_rx) = test_app();
        assert!(!app.restore_pending);
    }

    // Spec "Save Timing and Atomicity": the project file must be rewritten
    // after a mutating tracepoint event. `store` is injected directly
    // (bypassing `Store::from_env()`, mirroring `persistence.rs`'s own
    // `Store { root: dir }` test pattern) so this stays a fast, hermetic
    // unit test over a temp dir.
    #[test]
    fn apply_state_event_saves_project_file_on_breakpoint_added() {
        let (mut app, _cmd_rx) = test_app();
        let dir = unique_temp_dir("save-on-add");
        app.store = Some(crate::state::Store { root: dir.clone() });

        app.apply_state_event(crate::state::StateEvent::ProgramLoaded {
            executable: "/tmp/persist-test.out".into(),
        });
        app.apply_state_event(crate::state::StateEvent::BreakpointAdded {
            breakpoint: crate::state::Breakpoint {
                id: 1,
                file: "/tmp/main.c".into(),
                line: 5,
                requested_line: None,
                enabled: true,
                condition: None,
                condition_error: None,
            },
        });

        let store = app.store.as_ref().unwrap();
        match store.load("/tmp/persist-test.out") {
            crate::state::LoadOutcome::Loaded(project_file) => {
                assert_eq!(project_file.breakpoints.len(), 1);
                assert_eq!(project_file.breakpoints[0].file, "/tmp/main.c");
            }
            other => panic!("expected Loaded, got {other:?}"),
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    // D6: while `restore_pending` is true, state mid-replay holds only the
    // entries restored so far — saving then would truncate the file.
    #[test]
    fn apply_state_event_does_not_save_while_restore_pending() {
        let (mut app, _cmd_rx) = test_app();
        let dir = unique_temp_dir("no-save-during-restore");
        app.store = Some(crate::state::Store { root: dir.clone() });

        app.apply_state_event(crate::state::StateEvent::ProgramLoaded {
            executable: "/tmp/restore-pending-test.out".into(),
        });
        app.restore_pending = true;
        app.apply_state_event(crate::state::StateEvent::BreakpointAdded {
            breakpoint: crate::state::Breakpoint {
                id: 1,
                file: "/tmp/main.c".into(),
                line: 5,
                requested_line: None,
                enabled: true,
                condition: None,
                condition_error: None,
            },
        });

        let store = app.store.as_ref().unwrap();
        assert!(
            matches!(
                store.load("/tmp/restore-pending-test.out"),
                crate::state::LoadOutcome::Absent
            ),
            "no file must be written while restore_pending is true"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    // Triangulation: a read-only refresh event must not trigger a save
    // either, mirroring `mutates_tracepoints_false_for_non_mutating_events`.
    #[test]
    fn apply_state_event_does_not_save_for_non_mutating_event() {
        let (mut app, _cmd_rx) = test_app();
        let dir = unique_temp_dir("no-save-read-only");
        app.store = Some(crate::state::Store { root: dir.clone() });

        app.apply_state_event(crate::state::StateEvent::ProgramLoaded {
            executable: "/tmp/read-only-test.out".into(),
        });
        app.apply_state_event(crate::state::StateEvent::LocalsUpdated { vars: vec![] });

        let store = app.store.as_ref().unwrap();
        assert!(matches!(
            store.load("/tmp/read-only-test.out"),
            crate::state::LoadOutcome::Absent
        ));

        let _ = std::fs::remove_dir_all(&dir);
    }

    // Guard: without a loaded executable, `ProjectFile::from_persistent`
    // would serialize an empty `executable` string — saving here would
    // silently create a bogus, unnamed project file instead of skipping.
    #[test]
    fn apply_state_event_does_not_save_when_no_executable_loaded() {
        let (mut app, _cmd_rx) = test_app();
        let dir = unique_temp_dir("no-save-no-exe");
        app.store = Some(crate::state::Store { root: dir.clone() });

        app.apply_state_event(crate::state::StateEvent::BreakpointAdded {
            breakpoint: crate::state::Breakpoint {
                id: 1,
                file: "/tmp/main.c".into(),
                line: 5,
                requested_line: None,
                enabled: true,
                condition: None,
                condition_error: None,
            },
        });

        let store = app.store.as_ref().unwrap();
        assert!(matches!(store.load(""), crate::state::LoadOutcome::Absent));

        let _ = std::fs::remove_dir_all(&dir);
    }

    // Design "Re-fetch site": ValueEditSucceeded triggers an explicit
    // follow-up request matching the edited item's kind — here, a local
    // variable's write success must re-fetch the whole locals list (GDB's
    // ^done carries no value; the follow-up is the source of truth).
    #[test]
    fn value_edit_succeeded_local_triggers_request_locals_followup() {
        let (mut app, cmd_rx) = test_app();

        app.apply_state_event(crate::state::StateEvent::ValueEditSucceeded {
            target: crate::state::EditTarget::Local("x".into()),
        });

        let sent: Vec<Command> = cmd_rx.try_iter().collect();
        assert!(
            sent.contains(&Command::RequestLocals),
            "expected a RequestLocals follow-up, got {sent:?}"
        );
    }

    // Triangulates the follow-up match: a global write must re-evaluate
    // that specific global, not the whole locals list.
    #[test]
    fn value_edit_succeeded_global_triggers_evaluate_global_followup() {
        let (mut app, cmd_rx) = test_app();

        app.apply_state_event(crate::state::StateEvent::ValueEditSucceeded {
            target: crate::state::EditTarget::Global("g_counter".into()),
        });

        let sent: Vec<Command> = cmd_rx.try_iter().collect();
        assert!(
            sent.contains(&Command::EvaluateGlobal("g_counter".into())),
            "expected an EvaluateGlobal(\"g_counter\") follow-up, got {sent:?}"
        );
        assert!(!sent.contains(&Command::RequestLocals));
    }

    // Triangulates the follow-up match: a register write must re-fetch the
    // whole register set (no single-register MI request exists).
    #[test]
    fn value_edit_succeeded_register_triggers_request_registers_followup() {
        let (mut app, cmd_rx) = test_app();

        app.apply_state_event(crate::state::StateEvent::ValueEditSucceeded {
            target: crate::state::EditTarget::Register("pc".into()),
        });

        let sent: Vec<Command> = cmd_rx.try_iter().collect();
        assert!(
            sent.contains(&Command::RequestRegisters),
            "expected a RequestRegisters follow-up, got {sent:?}"
        );
        assert!(!sent.contains(&Command::RequestLocals));
    }

    // Pins the spec MUST at specs/thread-selection/spec.md:53: both the
    // pause and post-switch paths call this single shared function, so it
    // alone is responsible for the roster re-fetch — no drift possible
    // between two copies.
    #[test]
    fn refresh_thread_scoped_views_sends_request_threads_alongside_existing_refreshes() {
        let (app, cmd_rx) = test_app();
        app.refresh_thread_scoped_views();
        let sent: Vec<Command> = cmd_rx.try_iter().collect();

        assert!(sent.contains(&Command::RequestThreads));
        assert!(sent.contains(&Command::RequestLocals));
        assert!(sent.contains(&Command::RequestStack));
        assert!(sent.contains(&Command::RequestRegisters));
        assert!(sent.contains(&Command::RequestDisasm));
    }

    // Mirrors the existing pause-refresh behavior: no struct expression has
    // ever been committed → no Evaluate command is sent.
    #[test]
    fn refresh_thread_scoped_views_sends_no_evaluate_when_struct_expr_empty() {
        let (app, cmd_rx) = test_app();
        app.refresh_thread_scoped_views();
        let sent: Vec<Command> = cmd_rx.try_iter().collect();

        assert!(!sent.iter().any(|c| matches!(c, Command::Evaluate(_))));
    }

    // ── Memory tab (D3/D4/R8) ────────────────────────────────────────────────

    #[test]
    fn app_new_starts_with_empty_memory_input_and_watch_tab_default() {
        let (app, _cmd_rx) = test_app();
        assert_eq!(app.memory_input, "");
        assert_eq!(app.watch_tab, WatchTab::Watch);
    }

    #[test]
    fn watch_tab_memory_variant_exists_and_compares() {
        assert_eq!(WatchTab::Memory, WatchTab::Memory);
        assert_ne!(WatchTab::Memory, WatchTab::Data);
    }

    // R8: a committed address is re-sent on every pause/thread-switch
    // refresh, mirroring the struct_expr guard.
    #[test]
    fn refresh_thread_scoped_views_resends_request_memory_when_addr_committed() {
        let (mut app, cmd_rx) = test_app();
        app.state.memory_addr = "$sp".into();

        app.refresh_thread_scoped_views();
        let sent: Vec<Command> = cmd_rx.try_iter().collect();

        assert!(
            sent.contains(&Command::RequestMemory {
                address: "$sp".into(),
                count: 256,
            }),
            "expected a RequestMemory(\"$sp\", 256) follow-up, got {sent:?}"
        );
    }

    #[test]
    fn refresh_thread_scoped_views_sends_no_request_memory_when_addr_empty() {
        let (app, cmd_rx) = test_app();
        app.refresh_thread_scoped_views();
        let sent: Vec<Command> = cmd_rx.try_iter().collect();

        assert!(!sent.iter().any(|c| matches!(c, Command::RequestMemory { .. })));
    }

    #[test]
    fn refresh_thread_scoped_views_sends_exactly_one_request_memory_on_address_change() {
        let (mut app, cmd_rx) = test_app();
        app.state.memory_addr = "0x1000".into();

        app.refresh_thread_scoped_views();
        let sent: Vec<Command> = cmd_rx.try_iter().collect();

        let count = sent
            .iter()
            .filter(|c| matches!(c, Command::RequestMemory { .. }))
            .count();
        assert_eq!(count, 1, "expected exactly one RequestMemory, got {sent:?}");
    }

    // Preload-source: ProgramLoaded must trigger the one-shot probe
    // alongside the existing register-names/global-names requests, and the
    // probe itself must never surface as a breakpoint row — process.rs
    // intercepts and consumes its reply by MI token before it ever becomes
    // a StateEvent::BreakpointAdded, so no probe-originated event ever
    // reaches apply_state_event in the first place.
    #[test]
    fn program_loaded_sends_probe_main_source_and_creates_no_breakpoint_row() {
        let (mut app, cmd_rx) = test_app();

        app.apply_state_event(crate::state::StateEvent::ProgramLoaded {
            executable: "a.out".into(),
        });

        let sent: Vec<Command> = cmd_rx.try_iter().collect();
        assert!(
            sent.contains(&Command::ProbeMainSource),
            "expected a ProbeMainSource follow-up, got {sent:?}"
        );
        assert!(
            app.state.persistent.breakpoints.is_empty(),
            "ProgramLoaded alone must not create a breakpoint row"
        );
    }

    // ── Watchpoint event-loop routing ────────────────────────────────────────

    #[test]
    fn apply_state_event_watchpoint_added_updates_state() {
        let (mut app, _cmd_rx) = test_app();

        app.apply_state_event(crate::state::StateEvent::WatchpointAdded {
            watchpoint: crate::state::Watchpoint {
                id: 2,
                expr: "x".into(),
                kind: crate::state::WatchpointKind::Write,
                enabled: true,
            },
        });

        assert_eq!(app.state.persistent.watchpoints.len(), 1);
        assert_eq!(app.state.persistent.watchpoints[0].expr, "x");
    }

    #[test]
    fn apply_state_event_watchpoint_error_populates_watchpoint_errors() {
        let (mut app, _cmd_rx) = test_app();

        app.apply_state_event(crate::state::StateEvent::WatchpointError {
            expr: "nosuchvar".into(),
            message: "No symbol \"nosuchvar\" in current context.".into(),
        });

        assert_eq!(
            app.state.watchpoint_error("nosuchvar"),
            Some("No symbol \"nosuchvar\" in current context.")
        );
    }

    // Triangulation: Removed/Toggled must each apply through the same
    // generic routing, not just Added/Error.
    #[test]
    fn apply_state_event_watchpoint_removed_and_toggled_update_state() {
        let (mut app, _cmd_rx) = test_app();
        app.state
            .persistent
            .watchpoints
            .push(crate::state::Watchpoint {
                id: 3,
                expr: "y".into(),
                kind: crate::state::WatchpointKind::Read,
                enabled: true,
            });

        app.apply_state_event(crate::state::StateEvent::WatchpointToggled {
            id: 3,
            enabled: false,
        });
        assert!(!app.state.persistent.watchpoints[0].enabled);

        app.apply_state_event(crate::state::StateEvent::WatchpointRemoved { id: 3 });
        assert!(app.state.persistent.watchpoints.is_empty());
    }

    // ── Catchpoint event-loop routing (Task 5) ───────────────────────────────

    #[test]
    fn apply_state_event_catchpoint_added_updates_state() {
        let (mut app, _cmd_rx) = test_app();

        app.apply_state_event(crate::state::StateEvent::CatchpointAdded {
            catchpoint: crate::state::Catchpoint {
                id: 2,
                kind: crate::state::CatchpointKind::Fork,
                args: vec![],
                enabled: true,
            },
        });

        assert_eq!(app.state.persistent.catchpoints.len(), 1);
        assert_eq!(
            app.state.persistent.catchpoints[0].kind,
            crate::state::CatchpointKind::Fork
        );
    }

    #[test]
    fn apply_state_event_catchpoint_error_populates_catchpoint_errors() {
        let (mut app, _cmd_rx) = test_app();

        app.apply_state_event(crate::state::StateEvent::CatchpointError {
            key: "signal:BOGUS".into(),
            message: "Undefined signal name BOGUS.".into(),
        });

        assert_eq!(
            app.state.catchpoint_error("signal:BOGUS"),
            Some("Undefined signal name BOGUS.")
        );
    }

    // Triangulation: Removed/Toggled must each apply through the same
    // generic routing, not just Added/Error — mirrors the watchpoint test
    // above.
    #[test]
    fn apply_state_event_catchpoint_removed_and_toggled_update_state() {
        let (mut app, _cmd_rx) = test_app();
        app.state
            .persistent
            .catchpoints
            .push(crate::state::Catchpoint {
                id: 3,
                kind: crate::state::CatchpointKind::Exec,
                args: vec![],
                enabled: true,
            });

        app.apply_state_event(crate::state::StateEvent::CatchpointToggled {
            id: 3,
            enabled: false,
        });
        assert!(!app.state.persistent.catchpoints[0].enabled);

        app.apply_state_event(crate::state::StateEvent::CatchpointRemoved { id: 3 });
        assert!(app.state.persistent.catchpoints.is_empty());
    }

    // ── Phase 3 (persistence-serialization): restore replay ─────────────────
    //
    // RED: DTO -> Command mapping is the first replay building block (spec
    // "Load and Restore via Command Replay"; design D2/D3 — GDB assigns a
    // fresh id, so the DTO never round-trips one).

    #[test]
    fn breakpoint_dto_to_command_maps_file_line_and_condition() {
        let dto = crate::state::BreakpointDTO {
            file: "/tmp/main.c".into(),
            line: 12,
            enabled: true,
            condition: Some("i > 3".into()),
        };
        assert_eq!(
            breakpoint_dto_to_command(&dto),
            Command::AddBreakpoint {
                file: "/tmp/main.c".into(),
                line: 12,
                condition: Some("i > 3".into()),
            }
        );
    }

    #[test]
    fn breakpoint_dto_to_command_carries_no_condition_when_none() {
        let dto = crate::state::BreakpointDTO {
            file: "/tmp/a.c".into(),
            line: 3,
            enabled: false,
            condition: None,
        };
        assert_eq!(
            breakpoint_dto_to_command(&dto),
            Command::AddBreakpoint {
                file: "/tmp/a.c".into(),
                line: 3,
                condition: None,
            }
        );
    }

    #[test]
    fn watchpoint_dto_to_command_maps_expr_and_parses_kind() {
        let dto = crate::state::WatchpointDTO {
            expr: "counter".into(),
            kind: "write".into(),
            enabled: true,
        };
        assert_eq!(
            watchpoint_dto_to_command(&dto),
            Some(Command::AddWatchpoint {
                expr: "counter".into(),
                kind: crate::state::WatchpointKind::Write,
            })
        );
    }

    #[test]
    fn watchpoint_dto_to_command_none_for_unknown_kind_literal() {
        let dto = crate::state::WatchpointDTO {
            expr: "x".into(),
            kind: "bogus".into(),
            enabled: true,
        };
        assert_eq!(watchpoint_dto_to_command(&dto), None);
    }

    #[test]
    fn catchpoint_dto_to_command_maps_kind_and_args() {
        let dto = crate::state::CatchpointDTO {
            kind: "syscall".into(),
            args: vec!["open".into()],
            enabled: true,
        };
        assert_eq!(
            catchpoint_dto_to_command(&dto),
            Some(Command::AddCatchpoint {
                kind: crate::state::CatchpointKind::Syscall,
                args: vec!["open".into()],
            })
        );
    }

    #[test]
    fn catchpoint_dto_to_command_none_for_unknown_kind_literal() {
        let dto = crate::state::CatchpointDTO {
            kind: "bogus".into(),
            args: vec![],
            enabled: true,
        };
        assert_eq!(catchpoint_dto_to_command(&dto), None);
    }

    // Stop-reason dispatch (Task 5): a ProgramPaused carrying any of the 4
    // new catchpoint stop reasons must trigger the same thread-scoped
    // refresh as every other pause (no special-casing needed — the routing
    // is by StateEvent variant, not by stop_reason content).
    #[test]
    fn apply_state_event_catchpoint_stop_reasons_trigger_thread_scoped_refresh() {
        for reason in [
            crate::state::StopReason::Forked { newpid: Some(123) },
            crate::state::StopReason::Vforked { newpid: None },
            crate::state::StopReason::Execd { path: None },
            crate::state::StopReason::SolibEvent { library: None },
            // Phase 2b task 4.2 (verify-only): routing is by StateEvent
            // variant, not stop_reason content, so Syscall needs no new
            // production code here either.
            crate::state::StopReason::SyscallTriggered {
                phase: crate::state::SyscallPhase::Entry,
                number: Some("2".into()),
                name: Some("open".into()),
            },
        ] {
            let (mut app, cmd_rx) = test_app();
            app.apply_state_event(crate::state::StateEvent::ProgramPaused {
                pause: crate::state::PauseState {
                    thread_id: 1,
                    frame: crate::state::Frame {
                        addr: 0,
                        function: "??".into(),
                        file: None,
                        line: None,
                    },
                    stack: vec![],
                    stop_reason: reason.clone(),
                },
            });
            let sent: Vec<Command> = cmd_rx.try_iter().collect();
            assert!(
                sent.contains(&Command::RequestThreads),
                "expected RequestThreads for stop reason {reason:?}, got {sent:?}"
            );
        }
    }

    // ── Phase 3 (persistence-serialization): restore replay integration ─────

    fn seeded_project_file(exe: &str) -> crate::state::ProjectFile {
        crate::state::ProjectFile {
            schema_version: crate::state::SCHEMA_VERSION,
            executable: exe.into(),
            breakpoints: vec![crate::state::BreakpointDTO {
                file: "/tmp/main.c".into(),
                line: 5,
                enabled: true,
                condition: None,
            }],
            watchpoints: vec![crate::state::WatchpointDTO {
                expr: "counter".into(),
                kind: "write".into(),
                enabled: true,
            }],
            catchpoints: vec![crate::state::CatchpointDTO {
                kind: "fork".into(),
                args: vec![],
                enabled: true,
            }],
        }
    }

    fn bp_row(id: u32, file: &str, line: u32) -> crate::state::Breakpoint {
        crate::state::Breakpoint {
            id,
            file: file.into(),
            line,
            requested_line: Some(line),
            enabled: true,
            condition: None,
            condition_error: None,
        }
    }

    // Task 3.1/3.2: `ProgramLoaded` for an executable with a saved project
    // file loads it and replays every entry via `Command::Add*` — zero
    // direct `persistent.*` writes (spec "Load and Restore via Command
    // Replay").
    #[test]
    fn program_loaded_starts_restore_and_dispatches_add_commands_for_each_entry() {
        let (mut app, cmd_rx) = test_app();
        let dir = unique_temp_dir("restore-start");
        let exe = "/tmp/restore-start-exe.out";
        app.store = Some(crate::state::Store { root: dir.clone() });
        app.store
            .as_ref()
            .unwrap()
            .save(&seeded_project_file(exe))
            .expect("seed project file");

        app.apply_state_event(crate::state::StateEvent::ProgramLoaded {
            executable: exe.into(),
        });

        let sent: Vec<Command> = cmd_rx.try_iter().collect();
        assert!(sent.contains(&Command::AddBreakpoint {
            file: "/tmp/main.c".into(),
            line: 5,
            condition: None,
        }));
        assert!(sent.contains(&Command::AddWatchpoint {
            expr: "counter".into(),
            kind: crate::state::WatchpointKind::Write,
        }));
        assert!(sent.contains(&Command::AddCatchpoint {
            kind: crate::state::CatchpointKind::Fork,
            args: vec![],
        }));
        assert!(app.restore_pending, "restore must suppress the save hook");
        assert!(app.restore_session.is_some());
        assert_eq!(
            app.restore_session.as_ref().unwrap().outstanding.len(),
            3,
            "all 3 entries must be outstanding until their outcomes arrive"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    // Task 3.9/D6: once every replayed entry's success reply has arrived,
    // the session finalizes, `restore_pending` clears, and no failures were
    // reported.
    #[test]
    fn restore_finalizes_and_resumes_save_hook_once_all_entries_succeed() {
        let (mut app, _cmd_rx) = test_app();
        let dir = unique_temp_dir("restore-all-success");
        let exe = "/tmp/restore-success-exe.out";
        app.store = Some(crate::state::Store { root: dir.clone() });
        app.store
            .as_ref()
            .unwrap()
            .save(&seeded_project_file(exe))
            .expect("seed project file");

        app.apply_state_event(crate::state::StateEvent::ProgramLoaded {
            executable: exe.into(),
        });
        assert!(app.restore_pending);

        app.apply_state_event(crate::state::StateEvent::BreakpointAdded {
            breakpoint: bp_row(101, "/tmp/main.c", 5),
        });
        app.apply_state_event(crate::state::StateEvent::WatchpointAdded {
            watchpoint: crate::state::Watchpoint {
                id: 102,
                expr: "counter".into(),
                kind: crate::state::WatchpointKind::Write,
                enabled: true,
            },
        });
        app.apply_state_event(crate::state::StateEvent::CatchpointAdded {
            catchpoint: crate::state::Catchpoint {
                id: 103,
                kind: crate::state::CatchpointKind::Fork,
                args: vec![],
                enabled: true,
            },
        });

        assert!(
            !app.restore_pending,
            "restore must finalize once every entry has replied"
        );
        assert!(app.restore_session.is_none());
        assert!(
            app.restore_report.is_none(),
            "an all-success restore must not arm the report modal"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    // Task 3.6/3.7 (design D9): a disabled persisted breakpoint is added
    // like every other, but only toggled off AFTER its own `BreakpointAdded`
    // confirms a fresh id — never before.
    #[test]
    fn disabled_breakpoint_restore_toggles_off_only_after_added_event() {
        let (mut app, cmd_rx) = test_app();
        let dir = unique_temp_dir("restore-disabled");
        let exe = "/tmp/restore-disabled-exe.out";
        let mut project_file = seeded_project_file(exe);
        project_file.breakpoints[0].enabled = false;
        project_file.watchpoints.clear();
        project_file.catchpoints.clear();
        app.store = Some(crate::state::Store { root: dir.clone() });
        app.store.as_ref().unwrap().save(&project_file).expect("seed project file");

        app.apply_state_event(crate::state::StateEvent::ProgramLoaded {
            executable: exe.into(),
        });

        let before: Vec<Command> = cmd_rx.try_iter().collect();
        assert!(before.contains(&Command::AddBreakpoint {
            file: "/tmp/main.c".into(),
            line: 5,
            condition: None,
        }));
        assert!(
            !before.iter().any(|c| matches!(c, Command::ToggleBreakpoint { .. })),
            "must never toggle before the row exists: {before:?}"
        );

        app.apply_state_event(crate::state::StateEvent::BreakpointAdded {
            breakpoint: bp_row(55, "/tmp/main.c", 5),
        });

        let after: Vec<Command> = cmd_rx.try_iter().collect();
        assert!(
            after.contains(&Command::ToggleBreakpoint {
                id: 55,
                enable: false,
            }),
            "expected a toggle-off for the fresh id, got {after:?}"
        );
        assert!(
            !app.restore_pending,
            "the single outstanding entry resolved, so the session must finalize"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    // Task 3.8/3.9: a partial failure (1 of 2 breakpoints) restores the
    // success, reports the failure, and does NOT auto-drop it from the file
    // (design decision D8 — the user must explicitly prune).
    #[test]
    fn partial_restore_failure_restores_successes_and_reports_the_rest() {
        let (mut app, _cmd_rx) = test_app();
        let dir = unique_temp_dir("restore-partial");
        let exe = "/tmp/restore-partial-exe.out";
        let project_file = crate::state::ProjectFile {
            schema_version: crate::state::SCHEMA_VERSION,
            executable: exe.into(),
            breakpoints: vec![
                crate::state::BreakpointDTO {
                    file: "/tmp/ok.c".into(),
                    line: 10,
                    enabled: true,
                    condition: None,
                },
                crate::state::BreakpointDTO {
                    file: "/tmp/deleted.c".into(),
                    line: 42,
                    enabled: true,
                    condition: None,
                },
            ],
            watchpoints: vec![],
            catchpoints: vec![],
        };
        app.store = Some(crate::state::Store { root: dir.clone() });
        app.store.as_ref().unwrap().save(&project_file).expect("seed project file");

        app.apply_state_event(crate::state::StateEvent::ProgramLoaded {
            executable: exe.into(),
        });

        app.apply_state_event(crate::state::StateEvent::BreakpointAdded {
            breakpoint: bp_row(200, "/tmp/ok.c", 10),
        });
        app.apply_state_event(crate::state::StateEvent::BreakpointInsertFailed {
            file: "/tmp/deleted.c".into(),
            line: 42,
            message: "No such file or directory.".into(),
        });

        assert!(!app.restore_pending, "both entries replied — must finalize");
        assert_eq!(app.state.persistent.breakpoints.len(), 1);
        assert_eq!(app.state.persistent.breakpoints[0].file, "/tmp/ok.c");

        let report = app.restore_report.as_ref().expect("expected a restore report");
        assert_eq!(report.len(), 1);
        assert!(report[0].label.contains("/tmp/deleted.c"));
        assert_eq!(report[0].message, "No such file or directory.");

        let _ = std::fs::remove_dir_all(&dir);
    }

    // Design D8: "Keep" must leave the project file untouched — the failed
    // entry stays available for a retry on the next launch.
    #[test]
    fn dismiss_restore_report_leaves_project_file_untouched() {
        let (mut app, _cmd_rx) = test_app();
        let dir = unique_temp_dir("restore-keep");
        let exe = "/tmp/restore-keep-exe.out";
        let project_file = crate::state::ProjectFile {
            schema_version: crate::state::SCHEMA_VERSION,
            executable: exe.into(),
            breakpoints: vec![crate::state::BreakpointDTO {
                file: "/tmp/deleted.c".into(),
                line: 42,
                enabled: true,
                condition: None,
            }],
            watchpoints: vec![],
            catchpoints: vec![],
        };
        app.store = Some(crate::state::Store { root: dir.clone() });
        app.store.as_ref().unwrap().save(&project_file).expect("seed project file");

        app.apply_state_event(crate::state::StateEvent::ProgramLoaded {
            executable: exe.into(),
        });
        app.apply_state_event(crate::state::StateEvent::BreakpointInsertFailed {
            file: "/tmp/deleted.c".into(),
            line: 42,
            message: "No such file or directory.".into(),
        });
        assert!(app.restore_report.is_some());

        app.dismiss_restore_report();

        assert!(app.restore_report.is_none());
        match app.store.as_ref().unwrap().load(exe) {
            crate::state::LoadOutcome::Loaded(loaded) => {
                assert_eq!(
                    loaded.breakpoints.len(),
                    1,
                    "Keep must not touch the on-disk file"
                );
            }
            other => panic!("expected Loaded, got {other:?}"),
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    // Design D8: "Remove N failed" explicitly rewrites the file from
    // current in-memory state, which naturally drops the never-added
    // failure while keeping the successfully restored entry.
    #[test]
    fn remove_failed_restore_entries_rewrites_file_dropping_only_failures() {
        let (mut app, _cmd_rx) = test_app();
        let dir = unique_temp_dir("restore-remove");
        let exe = "/tmp/restore-remove-exe.out";
        let project_file = crate::state::ProjectFile {
            schema_version: crate::state::SCHEMA_VERSION,
            executable: exe.into(),
            breakpoints: vec![
                crate::state::BreakpointDTO {
                    file: "/tmp/ok.c".into(),
                    line: 10,
                    enabled: true,
                    condition: None,
                },
                crate::state::BreakpointDTO {
                    file: "/tmp/deleted.c".into(),
                    line: 42,
                    enabled: true,
                    condition: None,
                },
            ],
            watchpoints: vec![],
            catchpoints: vec![],
        };
        app.store = Some(crate::state::Store { root: dir.clone() });
        app.store.as_ref().unwrap().save(&project_file).expect("seed project file");

        app.apply_state_event(crate::state::StateEvent::ProgramLoaded {
            executable: exe.into(),
        });
        app.apply_state_event(crate::state::StateEvent::BreakpointAdded {
            breakpoint: bp_row(7, "/tmp/ok.c", 10),
        });
        app.apply_state_event(crate::state::StateEvent::BreakpointInsertFailed {
            file: "/tmp/deleted.c".into(),
            line: 42,
            message: "No such file or directory.".into(),
        });

        app.remove_failed_restore_entries();

        assert!(app.restore_report.is_none());
        match app.store.as_ref().unwrap().load(exe) {
            crate::state::LoadOutcome::Loaded(loaded) => {
                assert_eq!(loaded.breakpoints.len(), 1);
                assert_eq!(loaded.breakpoints[0].file, "/tmp/ok.c");
            }
            other => panic!("expected Loaded, got {other:?}"),
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    // design.md "3 s restore-report deadline ... checked in update()": a
    // hung/crashed reply must not suppress the save hook forever.
    #[test]
    fn finalize_restore_if_done_finalizes_past_deadline_even_with_outstanding_entries() {
        let (mut app, _cmd_rx) = test_app();
        app.restore_pending = true;
        app.restore_session = Some(RestoreSession {
            outstanding: vec![RestoreEntry::Breakpoint {
                file: "/tmp/never-replies.c".into(),
                line: 1,
                enabled: true,
            }],
            failures: vec![],
            deadline: std::time::Instant::now() - std::time::Duration::from_secs(1),
        });

        app.finalize_restore_if_done();

        assert!(!app.restore_pending);
        assert!(app.restore_session.is_none());
    }

    // Task 3.11/3.12 (spec "Persistence Enable/Disable Setting"): the
    // topbar toggle is a full opt-out — disabled means no load/restore at
    // all, even when a project file exists.
    #[test]
    fn program_loaded_skips_restore_when_persistence_disabled() {
        let (mut app, cmd_rx) = test_app();
        let dir = unique_temp_dir("restore-disabled-setting");
        let exe = "/tmp/restore-setting-off-exe.out";
        app.store = Some(crate::state::Store { root: dir.clone() });
        app.store
            .as_ref()
            .unwrap()
            .save(&seeded_project_file(exe))
            .expect("seed project file");
        app.settings.persistence_enabled = false;

        app.apply_state_event(crate::state::StateEvent::ProgramLoaded {
            executable: exe.into(),
        });

        let sent: Vec<Command> = cmd_rx.try_iter().collect();
        assert!(
            !sent.iter().any(|c| matches!(c, Command::AddBreakpoint { .. })),
            "disabled persistence must not replay any entry: {sent:?}"
        );
        assert!(!app.restore_pending);
        assert!(app.restore_session.is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }

    // Task 3.12: disabled persistence must also skip the save hook, mirroring
    // the load-side gate above.
    #[test]
    fn apply_state_event_does_not_save_when_persistence_disabled() {
        let (mut app, _cmd_rx) = test_app();
        let dir = unique_temp_dir("save-disabled-setting");
        app.store = Some(crate::state::Store { root: dir.clone() });
        app.settings.persistence_enabled = false;

        app.apply_state_event(crate::state::StateEvent::ProgramLoaded {
            executable: "/tmp/save-setting-off.out".into(),
        });
        app.apply_state_event(crate::state::StateEvent::BreakpointAdded {
            breakpoint: bp_row(1, "/tmp/main.c", 5),
        });

        let store = app.store.as_ref().unwrap();
        assert!(matches!(
            store.load("/tmp/save-setting-off.out"),
            crate::state::LoadOutcome::Absent
        ));

        let _ = std::fs::remove_dir_all(&dir);
    }

    // Design D7: a project file whose `schema_version` is newer than this
    // build understands must be quarantined read-only for the session — the
    // save hook must not clobber it, and no restore is attempted from it.
    #[test]
    fn too_new_schema_quarantines_session_and_skips_both_restore_and_save() {
        let (mut app, cmd_rx) = test_app();
        let dir = unique_temp_dir("restore-too-new");
        let exe = "/tmp/restore-too-new-exe.out";
        app.store = Some(crate::state::Store { root: dir.clone() });
        let store = app.store.as_ref().unwrap();
        let path = store.project_path(exe);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            format!(
                "schema_version = {}\nexecutable = \"{exe}\"\n",
                crate::state::SCHEMA_VERSION + 1
            ),
        )
        .unwrap();

        app.apply_state_event(crate::state::StateEvent::ProgramLoaded {
            executable: exe.into(),
        });

        assert!(app.persistence_quarantined);
        assert!(!app.restore_pending);
        let sent: Vec<Command> = cmd_rx.try_iter().collect();
        assert!(!sent.iter().any(|c| matches!(c, Command::AddBreakpoint { .. })));

        // A subsequent mutation must not overwrite the newer file.
        app.apply_state_event(crate::state::StateEvent::BreakpointAdded {
            breakpoint: bp_row(1, "/tmp/main.c", 5),
        });
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(
            raw.contains(&(crate::state::SCHEMA_VERSION + 1).to_string()),
            "the quarantined file must remain byte-for-byte the newer version's, got: {raw}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── Process attach (PR2, Phase 4) ────────────────────────────────────────

    // Task 4.1: the attach cascade sends exactly RequestRegisterNames +
    // RequestGlobalNames — ProbeMainSource must be absent (design D5: it
    // targets a not-yet-started program, which attach never is).
    #[test]
    fn apply_state_event_process_attached_sends_register_and_global_names_only() {
        let (mut app, cmd_rx) = test_app();

        app.apply_state_event(crate::state::StateEvent::ProcessAttached { pid: 4242 });

        let sent: Vec<Command> = cmd_rx.try_iter().collect();
        assert!(
            sent.contains(&Command::RequestRegisterNames),
            "expected a RequestRegisterNames follow-up, got {sent:?}"
        );
        assert!(
            sent.contains(&Command::RequestGlobalNames),
            "expected a RequestGlobalNames follow-up, got {sent:?}"
        );
        assert!(
            !sent.contains(&Command::ProbeMainSource),
            "ProbeMainSource must never be sent on attach, got {sent:?}"
        );
        assert_eq!(
            sent.len(),
            2,
            "attach cascade must send exactly two commands, got {sent:?}"
        );
    }

    // Task 4.2 (design D4): a successful attach must never arm a restore
    // replay — `was_loaded` matches `StateEvent::ProgramLoaded` only, and
    // `ProcessAttached` is a distinct variant, so `try_start_restore` is
    // structurally unreachable from this event.
    #[test]
    fn apply_state_event_process_attached_does_not_start_restore() {
        let (mut app, _cmd_rx) = test_app();
        let dir = unique_temp_dir("attach-no-restore");
        app.store = Some(crate::state::Store { root: dir.clone() });

        app.apply_state_event(crate::state::StateEvent::ProcessAttached { pid: 4242 });

        assert!(
            !app.restore_pending,
            "ProcessAttached must never arm a restore replay"
        );
        assert!(app.restore_session.is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }

    // Task 4.2 (design D4): `persistent.executable` stays `None` on attach,
    // so `save_persistent_state`'s own no-executable guard applies —
    // asserted end-to-end via the project-file store, mirroring
    // `apply_state_event_does_not_save_when_no_executable_loaded`.
    #[test]
    fn apply_state_event_process_attached_does_not_save_project_file() {
        let (mut app, _cmd_rx) = test_app();
        let dir = unique_temp_dir("attach-no-save");
        app.store = Some(crate::state::Store { root: dir.clone() });

        app.apply_state_event(crate::state::StateEvent::ProcessAttached { pid: 4242 });

        assert_eq!(app.state.persistent.executable, None);
        let store = app.store.as_ref().unwrap();
        assert!(matches!(store.load(""), crate::state::LoadOutcome::Absent));

        let _ = std::fs::remove_dir_all(&dir);
    }

    // Triangulation: seed a real project file for a PRIOR executable first,
    // so a restore WOULD have fired here if attach accidentally reused the
    // `ProgramLoaded` restore path. The prior file must be left untouched.
    #[test]
    fn apply_state_event_process_attached_leaves_a_prior_executables_project_file_untouched() {
        let (mut app, _cmd_rx) = test_app();
        let dir = unique_temp_dir("attach-leaves-prior-file-untouched");
        app.store = Some(crate::state::Store { root: dir.clone() });

        app.apply_state_event(crate::state::StateEvent::ProgramLoaded {
            executable: "/tmp/prior-exe.out".into(),
        });
        app.apply_state_event(crate::state::StateEvent::BreakpointAdded {
            breakpoint: bp_row(1, "/tmp/main.c", 5),
        });

        app.apply_state_event(crate::state::StateEvent::ProcessAttached { pid: 4242 });

        let store = app.store.as_ref().unwrap();
        match store.load("/tmp/prior-exe.out") {
            crate::state::LoadOutcome::Loaded(project_file) => {
                assert_eq!(
                    project_file.breakpoints.len(),
                    1,
                    "attach must not rewrite a prior executable's project file"
                );
            }
            other => panic!("expected the prior seeded project file to still be Loaded, got {other:?}"),
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── RemoteConnected cascade (Slice 2, task 7.1) ──────────────────────────

    // Task 7.1 (design "connected_remote flag ... sends RequestRegisterNames
    // + RequestGlobalNames and refresh_thread_scoped_views()"): unlike
    // attach, no `*stopped` is guaranteed after `^connected`, so the pause
    // cascade must be sent explicitly here too — not just the two
    // register/global-names requests `was_attached` sends alone.
    #[test]
    fn apply_state_event_remote_connected_sends_names_and_thread_scoped_refresh() {
        let (mut app, cmd_rx) = test_app();

        app.apply_state_event(crate::state::StateEvent::RemoteConnected {
            target: "localhost:1234".into(),
        });

        let sent: Vec<Command> = cmd_rx.try_iter().collect();
        assert!(
            sent.contains(&Command::RequestRegisterNames),
            "expected a RequestRegisterNames follow-up, got {sent:?}"
        );
        assert!(
            sent.contains(&Command::RequestGlobalNames),
            "expected a RequestGlobalNames follow-up, got {sent:?}"
        );
        assert!(
            sent.contains(&Command::RequestThreads),
            "expected refresh_thread_scoped_views's RequestThreads, got {sent:?}"
        );
        assert!(
            sent.contains(&Command::RequestLocals),
            "expected refresh_thread_scoped_views's RequestLocals, got {sent:?}"
        );
        assert!(
            sent.contains(&Command::RequestStack),
            "expected refresh_thread_scoped_views's RequestStack, got {sent:?}"
        );
        assert!(
            sent.contains(&Command::RequestRegisters),
            "expected refresh_thread_scoped_views's RequestRegisters, got {sent:?}"
        );
        assert!(
            sent.contains(&Command::RequestDisasm),
            "expected refresh_thread_scoped_views's RequestDisasm, got {sent:?}"
        );
        assert!(
            !sent.contains(&Command::ProbeMainSource),
            "ProbeMainSource must never be sent on remote connect, got {sent:?}"
        );
    }

    // Task 7.1: a remote connect must never arm a restore replay —
    // `was_loaded` matches `StateEvent::ProgramLoaded` only, and
    // `RemoteConnected` is a distinct variant, mirroring
    // `apply_state_event_process_attached_does_not_start_restore`.
    #[test]
    fn apply_state_event_remote_connected_does_not_start_restore() {
        let (mut app, _cmd_rx) = test_app();
        let dir = unique_temp_dir("remote-connect-no-restore");
        app.store = Some(crate::state::Store { root: dir.clone() });

        app.apply_state_event(crate::state::StateEvent::RemoteConnected {
            target: "localhost:1234".into(),
        });

        assert!(
            !app.restore_pending,
            "RemoteConnected must never arm a restore replay"
        );
        assert!(app.restore_session.is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }

    // Task 7.1: `persistent.executable` stays `None` on remote connect, so
    // `save_persistent_state`'s no-executable guard applies, mirroring
    // `apply_state_event_process_attached_does_not_save_project_file`.
    #[test]
    fn apply_state_event_remote_connected_does_not_save_project_file() {
        let (mut app, _cmd_rx) = test_app();
        let dir = unique_temp_dir("remote-connect-no-save");
        app.store = Some(crate::state::Store { root: dir.clone() });

        app.apply_state_event(crate::state::StateEvent::RemoteConnected {
            target: "localhost:1234".into(),
        });

        assert_eq!(app.state.persistent.executable, None);
        let store = app.store.as_ref().unwrap();
        assert!(matches!(store.load(""), crate::state::LoadOutcome::Absent));

        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── remote_target_buffer / PanelState::REMOTE (Slice 2, task 7.2) ───────

    #[test]
    fn app_new_starts_with_empty_remote_target_buffer() {
        let (app, _cmd_rx) = test_app();
        assert_eq!(app.remote_target_buffer, "");
    }

    #[test]
    fn panel_state_remote_flag_is_distinct_from_attach() {
        assert_ne!(PanelState::REMOTE.bits(), PanelState::ATTACH.bits());
        assert_ne!(PanelState::REMOTE.bits(), 0);
    }

    // Task 4.3 (design D3) + task 6.1 (design D2): gating predicate truth
    // table — enabled iff no program is loaded/attached AND no remote
    // target is connected AND the buffer parses to a non-zero u32.
    #[test]
    fn attach_enabled_truth_table() {
        use crate::state::ProgramState;
        use crate::ui::panels::attach::attach_enabled;

        // Enabled: no program, no attached_pid, no remote_target, valid
        // nonzero pid.
        assert!(attach_enabled(
            &ProgramState::NoProgramLoaded,
            None,
            None,
            "4242"
        ));

        // Disabled: every other ProgramState variant, even with no
        // attached_pid/remote_target and a valid buffer.
        assert!(!attach_enabled(
            &ProgramState::ProgramLoaded,
            None,
            None,
            "4242"
        ));
        assert!(!attach_enabled(&ProgramState::Running, None, None, "4242"));
        assert!(!attach_enabled(&ProgramState::Paused, None, None, "4242"));
        assert!(!attach_enabled(
            &ProgramState::Exited { code: Some(0) },
            None,
            None,
            "4242"
        ));
        assert!(!attach_enabled(
            &ProgramState::Attached { pid: 1 },
            None,
            None,
            "4242"
        ));

        // Disabled: attached_pid already set, even with NoProgramLoaded.
        assert!(!attach_enabled(
            &ProgramState::NoProgramLoaded,
            Some(1),
            None,
            "4242"
        ));

        // Disabled (design D2): a remote target is connected, even with
        // NoProgramLoaded, no attached_pid, and a valid buffer.
        assert!(!attach_enabled(
            &ProgramState::NoProgramLoaded,
            None,
            Some("localhost:1234"),
            "4242"
        ));
        // Disabled: the RemoteConnected ProgramState variant itself, paired
        // with the durable remote_target it implies.
        assert!(!attach_enabled(
            &ProgramState::RemoteConnected {
                target: "localhost:1234".into(),
            },
            None,
            Some("localhost:1234"),
            "4242"
        ));

        // Disabled: invalid buffer content — non-numeric, zero, negative,
        // overflowing u32, or empty.
        assert!(!attach_enabled(
            &ProgramState::NoProgramLoaded,
            None,
            None,
            "abc"
        ));
        assert!(!attach_enabled(
            &ProgramState::NoProgramLoaded,
            None,
            None,
            "0"
        ));
        assert!(!attach_enabled(
            &ProgramState::NoProgramLoaded,
            None,
            None,
            "-1"
        ));
        assert!(!attach_enabled(
            &ProgramState::NoProgramLoaded,
            None,
            None,
            "99999999999"
        ));
        assert!(!attach_enabled(
            &ProgramState::NoProgramLoaded,
            None,
            None,
            ""
        ));

        // Injection row (threat matrix): a trailing-newline buffer (e.g.
        // "12\n-exec-run") must never enable the button — u32::from_str
        // rejects trailing whitespace/newlines outright.
        assert!(!attach_enabled(
            &ProgramState::NoProgramLoaded,
            None,
            None,
            "12\n-exec-run"
        ));
    }

    // ── Shutdown release (PR3 Phase 5, generalized Slice 2 Phase 8 / D6) ────

    // Task 8.1: `shutdown_release` is the pure predicate `on_exit` delegates
    // to (design D6, generalizing "Detach on Exit" step 1) — `None` unless
    // either a process is attached or a remote target is connected.
    #[test]
    fn shutdown_release_is_none_when_never_attached_or_connected() {
        let state = DebuggerState::new();
        assert_eq!(shutdown_release(&state), None);
    }

    #[test]
    fn shutdown_release_is_some_detach_when_attached() {
        let mut state = DebuggerState::new();
        state.apply(crate::state::StateEvent::ProcessAttached { pid: 4242 });
        assert_eq!(shutdown_release(&state), Some(ShutdownRelease::Detach { pid: 4242 }));
    }

    #[test]
    fn shutdown_release_is_some_disconnect_when_remote_connected() {
        let mut state = DebuggerState::new();
        state.apply(crate::state::StateEvent::RemoteConnected {
            target: "localhost:1234".into(),
        });
        assert_eq!(
            shutdown_release(&state),
            Some(ShutdownRelease::Disconnect {
                target: "localhost:1234".into()
            })
        );
    }

    // Task 8.2: rename `wait_for_detach_ack`/`DetachAck` →
    // `wait_for_release_ack`/`ReleaseAck` — the three outcomes `on_exit`
    // reacts to (design "Detach on Exit" step 4, generalized by D6).
    #[test]
    fn wait_for_release_ack_returns_finished_on_detach_finished_event() {
        let (tx, rx) = mpsc::channel();
        tx.send(crate::state::DebuggerEvent::State(
            crate::state::StateEvent::DetachFinished { error: None },
        ))
        .unwrap();

        let ack = wait_for_release_ack(&rx, std::time::Duration::from_millis(200));
        assert_eq!(ack, ReleaseAck::Finished);
    }

    // D6: `ReleaseAck::Finished` must also fire on `RemoteDisconnected` —
    // `wait_for_release_ack` is shared by both the detach and disconnect
    // shutdown branches.
    #[test]
    fn wait_for_release_ack_returns_finished_on_remote_disconnected_event() {
        let (tx, rx) = mpsc::channel();
        tx.send(crate::state::DebuggerEvent::State(
            crate::state::StateEvent::RemoteDisconnected { error: None },
        ))
        .unwrap();

        let ack = wait_for_release_ack(&rx, std::time::Duration::from_millis(200));
        assert_eq!(ack, ReleaseAck::Finished);
    }

    // Discards unrelated events on the way to the real ack (design step 4:
    // "discarding every event that is not State(DetachFinished{..} |
    // RemoteDisconnected{..})").
    #[test]
    fn wait_for_release_ack_discards_unrelated_events_before_finished() {
        let (tx, rx) = mpsc::channel();
        tx.send(crate::state::DebuggerEvent::Ui(
            crate::state::UiEvent::ConsoleOutput("noise".into()),
        ))
        .unwrap();
        tx.send(crate::state::DebuggerEvent::State(
            crate::state::StateEvent::LocalsUpdated { vars: vec![] },
        ))
        .unwrap();
        tx.send(crate::state::DebuggerEvent::State(
            crate::state::StateEvent::DetachFinished { error: None },
        ))
        .unwrap();

        let ack = wait_for_release_ack(&rx, std::time::Duration::from_millis(200));
        assert_eq!(ack, ReleaseAck::Finished);
    }

    #[test]
    fn wait_for_release_ack_times_out_on_silence() {
        let (_tx, rx) = mpsc::channel::<crate::state::DebuggerEvent>();
        let ack = wait_for_release_ack(&rx, std::time::Duration::from_millis(50));
        assert_eq!(ack, ReleaseAck::TimedOut);
    }

    #[test]
    fn wait_for_release_ack_returns_disconnected_when_sender_dropped() {
        let (tx, rx) = mpsc::channel::<crate::state::DebuggerEvent>();
        drop(tx);
        let ack = wait_for_release_ack(&rx, std::time::Duration::from_millis(200));
        assert_eq!(ack, ReleaseAck::Disconnected);
    }
}
