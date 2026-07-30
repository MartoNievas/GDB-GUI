# Roadmap

Gaps identified in the current implementation that block this project from
being a seriously usable debugger, ordered roughly by priority.

## Done

- **Global-variable correlation race condition (fixed)** — `pending_globals`
  in `src/gdb/process.rs` was a FIFO `VecDeque<String>` matched to replies by
  arrival order; a raced raw console command sharing the same reply shape
  could silently misattribute a value to the wrong variable. Replaced with a
  `HashMap<u32, String>` correlated by MI token, mirroring the existing
  `pending_struct`/`pending_cond` pattern; `^error` now also clears the
  pending entry (previously leaked on failure). See
  `openspec/changes/global-var-correlation/`.
- **Thread selector (implemented)** — `panels/thread.rs` used to only render
  the paused thread's id; there was no `-thread-info` roster and no
  `Command::SelectThread`. Added a live thread list (fetched on pause and
  after every switch), clickable rows (paused-only) dispatching
  `-thread-select`, and a shared `refresh_thread_scoped_views` cascade so
  stack/locals/registers/disassembly/thread-list stay in sync whether the
  program just paused or the user just switched threads. Resolved via
  shape-sniffing in the MI parser rather than the token-correlation pattern
  above, since `-thread-select`'s reply field (`new-thread-id=`) is
  unambiguous. See `openspec/changes/archive/2026-07-27-thread-selector/`.
- **Value editing (implemented)** — `src/ui/command.rs` now defines
  `Command::SetValue` targeting local/global/register values via `EditTarget`
  (`src/state/debugger_state.rs`). The GDB write path uses token-correlated
  `-gdb-set` MI commands in `src/gdb/writer.rs` (`build_gdb_set` and
  `strip_mi_newlines` for MI-injection safety), with pending-map tracking in
  `src/gdb/process.rs` (`pending_edit` and `correlate_pending_edit`, mirroring
  the token-correlation pattern). On success, `src/ui/app.rs` explicitly
  re-fetches the edited value; on error, the inline message displays in
  `src/ui/panels/watch.rs`. Editable cells support variables and registers
  only — the struct panel remains read-only. Manual testing against GDB 17.2
  verified local write, global write, control register write without
  confirmation, and error hard-revert behavior. All 123/123 tests passing.
- **Preload main's source without requiring Run (implemented)** —
  `Command::ProbeMainSource` in `src/ui/command.rs` sends a temporary
  `-break-insert -t main` probe on `ProgramLoaded`, correlated and intercepted
  before `parse_line` in `src/gdb/process.rs` (`pending_probe` and
  `correlate_pending_probe`) so it never becomes a real `BreakpointAdded`
  event, immediately deleted via a direct `-break-delete`. The resolved file
  populates a new `preview_file` in `src/state/debugger_state.rs`, consumed by
  `source_view_file()` so the source view (and therefore the breakpoint gutter)
  is populated as soon as an executable loads, without requiring the user to
  press Run first. The probe's non-interference with user breakpoints at the
  same line and no-main fallback behavior were verified empirically against
  GDB 17.2. All 131/131 tests passing (8 new tests added).
- **Watchpoints (implemented, Phase 1)** — New `Watchpoint` type in
  `src/state/debugger_state.rs` (Write/Read/Access kinds) with MI parsing for
  `-break-watch` replies (`wpt=`/`hw-rwpt=`/`hw-awpt=`). Separate UI panel
  (`src/ui/panels/watchpoints.rs`) independent of breakpoints, displaying
  creation errors persistently, duplicate-expression rejection with inline
  message, and trigger old→new values in a yellow banner. Automatic scope
  cleanup on `watchpoint-scope` events (no UI toast). Strict TDD: 185/185 tests
  passing (47 new).
- **Catchpoints (implemented, Phase 2a+2b+2c)** — New `Catchpoint` type in
  `src/state/debugger_state.rs` (Fork/Vfork/Exec/Signal/Load/Unload/Syscall/
  Throw/Rethrow/Catch kinds) with three MI ingress shapes:
  console-passthrough for fork/vfork/exec/signal/syscall (via
  `-interpreter-exec console "catch <kind> [args]"`), native verbs for
  load/unload (`-catch-load/-catch-unload`), and native verbs for the C++
  exception kinds (`-catch-throw/-catch-rethrow/-catch-catch`, optional
  `-r <regexp>`). Separate UI panel (`src/ui/panels/catchpoints.rs`) with
  stop-event labeling (fork/vfork show new PID, exec shows path, load/unload
  show library name, syscall shows number/name for both entry and return
  stops, exceptions show "Exception thrown/rethrown/caught"). Strict TDD:
  367/367 tests passing (73 new catchpoints tests in Phase 2a, 49 new
  syscall tests in Phase 2b, 41 new exception tests in Phase 2c). Catchpoints
  feature is now complete.
- **Architecture cleanup (Phase 1)** — Broke the sole bidirectional import
  cycle (`app.rs <-> panels/watch.rs`) by relocating `WatchTab` enum from
  `watch.rs` to `app.rs`, restoring uniform one-way dependency direction
  across all 11 panels. Verified by graphify: "Import Cycles: None detected".
  Extracted error state maps (`edit_errors`, `watchpoint_errors`,
  `catchpoint_errors`) from `DebuggerState` into new `ErrorState` substruct,
  improving cohesion structurally as a first step toward larger decomposition.
  All 367/367 tests passing. See `openspec/changes/archive/2026-07-30-{break-app-watch-cycle,extract-error-state}/`.
- **Architecture cleanup (Phase 2)** — Decomposed `run_loop()` (310 lines,
  cohesion 0.11) by extracting 3 functions (`spawn_reader_thread`,
  `handle_commands`, `handle_gdb_output`) that encapsulate distinct concerns
  (reader setup, command dispatch, output processing). Grouped 7 independent
  `pending_*` maps into `struct PendingRegistry` with distinctly-typed fields
  (preserving type-level mutual isolation), eliminating the 7-parameter fan-out
  smell and improving cohesion to 0.33. Preserved exact check order and
  continue/fall-through semantics verified by manual smoke test against live GDB.
  Strict TDD: 369/369 tests passing (2 new characterization tests added).
  See `openspec/changes/archive/2026-07-30-{extract-run-loop-functions,pending-registry}/`.

## Missing debugging functionality

- **No memory view (hex dump)** — no command wraps
  `-data-read-memory-bytes`; raw memory cannot be inspected outside of
  named variables.
- **No attach to a running process** — only `LoadExecutable(String)` via
  `-file-exec-and-symbols`; no `-target-attach <pid>`.
- **No remote debugging or core dumps** — no `target remote`, no core file
  loading.

## Fragility already acknowledged by the project

- Breakpoint conditions are only validated **after** the round-trip to GDB;
  there is no local validation of the C expression before sending it.

## Persistence

- `PersistentState` (`src/state/debugger_state.rs`) only holds `executable`
  and `breakpoints` in process memory — nothing is serialized to disk.
  Closing the app loses breakpoints and session state; there is no
  "project" or config file concept.

## Already solid

Basic execution control, conditional breakpoints with MI-injection-safe
quoting (covered by explicit tests), an MI parser with good test coverage,
and a clean two-thread architecture.
