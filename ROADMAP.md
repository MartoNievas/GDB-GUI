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
- **Memory view (hex dump, implemented)** — New `Command::RequestMemory`
  wraps `-data-read-memory-bytes` (`src/gdb/writer.rs`, with an unbypassable
  `clamp_memory_count` capping requests at 4096 bytes). The self-describing
  `memory=[...]` reply is parsed unconditionally in `src/gdb/parser.rs`
  (`parse_memory`/`decode_hex`, best-effort hex decode that never panics on
  malformed input) into `Vec<MemoryBlock>` on `DebuggerState`; only `^error`
  is correlated back to the requested address via `pending.memory` in
  `src/gdb/process.rs`. New `Memory` tab in `src/ui/panels/watch.rs`
  (address input, 16-bytes/row offset|hex|ASCII grid via pure
  `format_hex_row`) auto-refetches on every pause through the existing
  `refresh_thread_scoped_views` cascade. The `Data` tab was relabeled
  `Disasm` in the same change to avoid ambiguity with the new tab. Strict
  TDD: 408/408 tests passing (39 new).
- **Attach to a running process (implemented)** — `Command::AttachToProcess(u32)`
  (`src/ui/command.rs:97`) maps to `-target-attach {pid}`, and `Command::DetachForShutdown`
  (`command.rs:104`) to `-target-detach` (`src/gdb/writer.rs`). `src/gdb/process.rs`'s
  `PendingRegistry` gained error-only `attach`/`detach` token maps, correlating
  `^error` back to `StateEvent::ProcessAttachFailed`/`DetachFinished` (`^done` is
  cleanup-only for `attach`, mirroring `catch`/`watch`); success is signalled
  optimistically at dispatch via `StateEvent::ProcessAttached`, since attach's
  `*stopped` reply carries no pid. `src/state/debugger_state.rs` adds
  `ProgramState::Attached{pid}` plus a durable `attached_pid: Option<u32>` that
  survives the `*stopped` → `Paused` transition, and `attach_error` for
  persistent (not toast) error display. A new Attach panel
  (`src/ui/panels/attach.rs`) takes a PID and gates the button on no
  program being loaded/attached plus a valid non-zero `u32` (`attach_enabled`).
  Closing the GUI while attached auto-detaches instead of leaving the
  process stopped-and-traced or killing it outright: `App::on_exit`
  (`src/ui/app.rs:1049`) delegates to `should_detach_on_exit` (`app.rs:880`,
  pure `Option<u32>` predicate) and `wait_for_detach_ack` (`app.rs:855`, a
  bounded `recv_timeout` loop against a 2s deadline, `DETACH_TIMEOUT` at
  `app.rs:833`) — interrupting first if the inferior is running (GDB in
  synchronous MI mode does not read stdin while running), since a piped
  `-target-detach` would otherwise never be consumed. A timed-out or
  disconnected ack falls back to the pre-existing kill-on-exit path in
  `process.rs::run_loop` (unchanged) and is reported via `eprintln!` naming
  the pid. There is no in-app process picker (the PID must be known ahead of
  time, e.g. via `ps`/`pgrep`) and no interactive mid-session detach — only
  the shutdown path detaches. Strict TDD: 502/502 tests passing (34 new
  across the attach implementation).

  Verified live against GDB 17.2: `-target-attach <pid>` against a real
  long-lived process (opted into being traced via its own `PR_SET_PTRACER`,
  not a `ptrace_scope` change) produced a real `*stopped` record with no
  `reason=` field and a populated `frame=`, exactly matching the
  `parser.rs` no-reason fixture; the follow-up `-data-list-register-names`,
  `-symbol-info-variables`, `-thread-info`, `-stack-list-variables
  --all-values`, `-stack-list-frames`, and `-data-list-register-values`
  commands (the exact MI strings `RequestRegisterNames`/`RequestGlobalNames`/
  `refresh_thread_scoped_views` send) all returned populated `^done` replies.
  Resuming the inferior, then replicating `on_exit`'s
  interrupt-then-detach sequence (`SIGINT` to GDB's own pid, then
  `-target-detach`) produced GDB's own `*stopped,reason="signal-received"`
  followed by `9^done` for the detach, and the target process was confirmed
  alive and running independently (`ps` state `S`, not `T`/zombie) after the
  GDB process exited — the core mechanism `App::on_exit` relies on. Yama
  `ptrace_scope=1` denial (attaching to an unrelated, non-opted-in sibling
  process) reproduced GDB's own `^error,msg="ptrace: Operation not
  permitted."` verbatim, confirming the error passes through
  `ProcessAttachFailed`/`attach_error` unmodified in the common (ASCII)
  case — a non-English locale was observed to emit an accented message that
  `unescape` (`parser.rs`) does not octal-decode, a pre-existing limitation
  of that function unrelated to attach specifically, not fixed here.
  These checks were run directly against GDB's MI protocol with the exact
  command strings `gdb-gui` sends, **not** through the compiled `gdb-gui`
  binary itself — this sandbox runs inside a real, active desktop session,
  and launching/controlling GUI windows on it without being asked was
  avoided. **Still needs manual verification**: actually running the built
  `gdb-gui` binary, attaching to a real long-lived process you started
  yourself, and closing the window — confirming (a) `eframe::App::on_exit`
  actually fires on window-close in eframe 0.33.3, and (b) the process is
  still alive via `ps` afterward, including while it was mid-`Continue`.
- **Remote target connection (implemented)** — `Command::ConnectRemote{target}`
  (`src/ui/command.rs`) maps to `-target-select extended-remote <target>`, and
  `Command::DisconnectForShutdown` to `-target-disconnect` (`src/gdb/writer.rs`,
  both via `strip_mi_newlines`). Unlike attach, success is **correlated**, not
  optimistic: `src/gdb/process.rs`'s `PendingRegistry` gained
  `remote_connect`/`remote_disconnect` token maps and
  `correlate_pending_remote_connect`/`correlate_pending_remote_disconnect`,
  emitting `StateEvent::RemoteConnected{target}`/`RemoteConnectFailed{target,
  message}`/`RemoteDisconnected{error}` off the actual `^connected`/`^error`/
  `^done` replies. `src/state/debugger_state.rs` adds
  `ProgramState::RemoteConnected{target}` plus a durable
  `remote_target: Option<String>` that survives the `*stopped` -> `Paused`
  transition (mirroring `attached_pid`), and `remote_connect_error` for
  persistent error display. The connect gate deliberately allows
  `ProgramLoaded` as well as `NoProgramLoaded` (not `NoProgramLoaded` alone) —
  `gdb-gui ./firmware.elf` loads symbols before connecting, and a
  symbol-less-only gate would make that workflow impossible. A new Remote
  panel (`src/ui/panels/remote.rs`) takes a `host:port`, rebuilds it
  canonically from a validated host charset + `u16` port
  (`parse_remote_target`, `src/state/debugger_state.rs`) rather than
  raw-interpolating user text, and gates the Connect button
  (`remote_connect_enabled`) on that plus no local program/attach and no
  existing remote connection. `attach_enabled` (`src/ui/panels/attach.rs`)
  gained a fourth `remote_target: Option<&str>` parameter so local attach is
  disabled while connected, and vice versa. Closing the GUI while connected
  sends `-target-disconnect` (never `-target-detach`) so the target is left
  **stopped, not resumed** — generalized from the attach shutdown path via
  `ShutdownRelease{Detach{pid}|Disconnect{target}}` and `shutdown_release()`
  (`src/ui/app.rs`, replacing `should_detach_on_exit`), with
  `wait_for_release_ack`/`ReleaseAck` (renamed from
  `wait_for_detach_ack`/`DetachAck`) accepting either `DetachFinished` or
  `RemoteDisconnected` on the same bounded 2s ack. Out of scope: serial/other
  device targets, core-dump loading, and running or attaching to a process
  after connect. No interactive/manual disconnect exists — only shutdown
  triggers it. Strict TDD: 569/569 tests passing across both delivery slices
  (Slice 1: protocol/state core; Slice 2: panel, `app.rs` wiring, shutdown
  generalization, docs).

  **Still needs manual verification against a real `gdbserver`** (Phase 10,
  not yet run in this sandboxed environment — no live GDB remote target
  available): connect populates stack/threads/locals/registers; the actual
  `^connected` payload and whether a `*stopped` follows it; a refused/
  unreachable target's message renders verbatim and the app stays
  reconnectable; a watchpoint/catchpoint `qSupported` rejection surfaces as
  `^error`; closing the GUI while connected leaves the stub stopped and
  reconnectable (`extended-remote`/`--multi`), not exited; and the
  symbols-loaded-then-connect workflow (`gdb-gui ./firmware.elf`, then
  connect) actually succeeds. _Verified live against GDB X.Y: pending —
  fill in after running Phase 10's manual smoke test._
- **Session persistence (implemented)** — Breakpoints, watchpoints, and
  catchpoints now survive across restarts. `src/state/persistence.rs` owns a
  pure (no `egui`, no GDB) TOML DTO, a per-executable project file at
  `~/.config/gdb-gui/projects/<fnv1a64-hex>.toml` (self-implemented FNV-1a
  64-bit hash of the canonicalized absolute executable path — not SHA-256;
  std's `DefaultHasher`/SipHash was rejected because its output is
  explicitly unstable across Rust releases), and atomic save (tmp file +
  `fsync` + rename, surviving a crash mid-write). `src/ui/app.rs`'s
  `apply_state_event` saves after every mutating tracepoint event
  (`mutates_tracepoints`) and, on `StateEvent::ProgramLoaded`, loads the
  project file and replays every entry as a fresh `Command::Add{Breakpoint,
  Watchpoint,Catchpoint}` so GDB assigns new ids — no id ever round-trips
  through the file. A disabled entry is added, then toggled off only after
  its own `*Added` event confirms the fresh id (never before). Partial
  restore failures (e.g. a deleted source file) are collected into a
  `RestoreSession` and reported via a modal (`src/ui/panels/restore_report.rs`)
  offering `Keep` (leave the file untouched — retry next launch) or
  `Remove N failed` (explicitly rewrite the file, dropping only the
  failures); failures are never auto-dropped. An unrecognized/newer
  `schema_version` quarantines saves for the session (never clobbers a
  newer file); unparsable TOML warns and starts clean. A topbar "Persist"
  checkbox (backed by `src/state/settings.rs`'s `Settings`/`SettingsStore`,
  plus a `GDB_GUI_NO_PERSIST` environment override) is a full opt-out for
  both saving and loading/restoring. Strict TDD: 468/468 tests passing (60
  new across Phases 1-3).

## Fragility already acknowledged by the project

- Breakpoint conditions are only validated **after** the round-trip to GDB;
  there is no local validation of the C expression before sending it.

## Already solid

Basic execution control, conditional breakpoints with MI-injection-safe
quoting (covered by explicit tests), an MI parser with good test coverage,
and a clean two-thread architecture.
