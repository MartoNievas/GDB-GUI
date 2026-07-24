# Tasks: Struct Panel — single-expression struct inspector

## Review Workload Forecast

| Field | Value |
|-------|-------|
| Estimated changed lines | ~300-420 (additions+deletions across 5 files + tests) |
| 400-line budget risk | Low (assigned budget for this change is 800; fits comfortably) |
| Chained PRs recommended | No |
| Suggested split | Single local change (no PR/branching per project constraint) |
| Delivery strategy | single-pr (no git commit/PR/branching work in scope) |
| Chain strategy | pending (not applicable — no PR chain used) |

Decision needed before apply: No
Chained PRs recommended: No
Chain strategy: pending
400-line budget risk: Low

### Suggested Work Units

| Unit | Goal | Likely PR | Focused test command | Runtime harness | Rollback boundary |
|------|------|-----------|----------------------|-----------------|-------------------|
| 1 | Full struct-panel wiring (state → gdb → ui) as one atomic local change | N/A — no PR chain (single local change per constraint) | `cargo test` | N/A — no scripted e2e harness in repo; manual verification via `cargo run` + attach GDB session, type expression, pause/continue | Revert the single commit; panel returns to inert stub, `Command::Evaluate` returns to dead-but-present |

Rationale for single unit: the token correlation contract spans `writer.rs` → `process.rs` → `debugger_state.rs` → `app.rs` → `struct_panel.rs`; splitting across PRs would leave intermediate slices uncompilable or untestable against the spec scenarios. Estimated size (~300-420 lines) sits well under the 800-line budget assigned to this change.

## Phase 1: State Foundation (`src/state/debugger_state.rs`)

- [x] 1.1 RED: test `apply(StructValueUpdated)` — matching `expr == state.struct_expr` sets `struct_value`; stale/mismatched `expr` is dropped (spec: Evaluated Value Display).
- [x] 1.2 RED: test lifecycle clears — `ProgramLoaded` clears both `struct_expr` and `struct_value`; `ProgramStarted`/`ProgramExited` clear only `struct_value`, keeping `struct_expr` (spec: Empty Placeholder State Before Commit; design lifecycle decision).
- [x] 1.3 GREEN: add `pub struct_expr: String` and `pub struct_value: Option<String>` to `DebuggerState`; add `StateEvent::StructValueUpdated { expr: String, value: String }`; implement the guarded `apply()` arm; implement the three lifecycle clears above — all in `src/state/debugger_state.rs`.
- [x] 1.4 REFACTOR: run `cargo test debugger_state`; confirm no dead branches, naming matches `struct_expr`/`struct_value` exactly per design contract.

## Phase 2: GDB Wire Protocol

- [x] 2.1 RED: test `command_to_mi(Command::Evaluate("*p\n-exec-continue".into()))` has no `\n`/`\r` and is a single MI command (threat matrix: PR commands arg composition — newline smuggling), in `src/gdb/writer.rs`.
- [x] 2.2 RED: test an expression with spaces (e.g. `arr[i + 1]`) survives quoted and round-trips correctly (spec: Expression Sanitization Against Command Injection), in `src/gdb/writer.rs`.
- [x] 2.3 GREEN: change the `Command::Evaluate` arm in `src/gdb/writer.rs` to `format!("-data-evaluate-expression {}", quote_mi(expr))`.
- [x] 2.4 RED: test `correlate_pending_struct` — `^done,value=...` removes the token and emits `StructValueUpdated{expr,value}`; `^error` removes the token, emits no event, and does not `continue` (falls through to `parse_line` for console `[ERROR]`); a foreign token is left untouched (spec: Token-Based Reply Correlation, both in-flight and struct-before-global scenarios), in `src/gdb/process.rs`.
- [x] 2.5 GREEN: declare `pending_struct: HashMap<u32, String>` beside `pending_cond` in `src/gdb/process.rs`; populate it where `DebuggerCommand::Evaluate(expr)` is dispatched; insert the token-correlation check as the FIRST branch in the `line_rx` reply loop, before the existing `pending_globals` block (~line 202).
- [x] 2.6 REFACTOR: run `cargo test process` and `cargo test writer`; verify `pending_struct` is checked strictly before `pending_globals` in source order.

## Phase 3: UI Wiring

- [x] 3.1 RED: tests for `should_commit_struct_expr(lost_focus, buffer, committed)` adapted from `should_commit_breakpoint_condition` — false while typing (not `lost_focus`); true on blur when `buffer != committed`; false on blur when unchanged; true on blur when buffer emptied (spec: Commit-on-Enter Expression Evaluation), in `src/ui/panels/struct_panel.rs`.
- [x] 3.2 GREEN: implement `pub(crate) fn should_commit_struct_expr(lost_focus: bool, buffer: &str, committed: &str) -> bool` per design contract in `src/ui/panels/struct_panel.rs`.
- [x] 3.3 GREEN: replace the stub in `src/ui/panels/struct_panel.rs` with a text input bound to `App::struct_input`, committing via `should_commit_struct_expr` on Enter/blur; on commit set `state.struct_expr`, clear `state.struct_value`, send `Command::Evaluate` only when non-empty; render `state.struct_value` when `Some`, else the neutral placeholder (spec: Empty Placeholder State Before Commit).
- [x] 3.4 GREEN: add `struct_input: String` to `App` in `src/ui/app.rs`; inside the existing `if was_paused` block, immediately after the `global_names` loop, add `if !self.state.struct_expr.is_empty() { self.send(Command::Evaluate(self.state.struct_expr.clone())); }` (spec: Auto Re-evaluation on Pause, incl. "no expression committed → no command sent").
- [x] 3.5 REFACTOR: run `cargo test panels::struct_panel` and `cargo test app`; confirm no per-keystroke sends, single commit-driven send.

## Phase 4: Verification & Cleanup

- [x] 4.1 Add a doc comment on `Command::Evaluate` in `src/ui/command.rs` recording that it is reused (no new variant) and is the struct panel's sole producer.
- [x] 4.2 Run full `cargo test`; manually trace every spec scenario (commit-on-Enter/blur, evaluated value display, auto re-eval on pause incl. no-expression-yet, token correlation incl. simultaneous/struct-before-global, newline sanitization, console `[ERROR]` on error, empty placeholder state) against the implementation.
- [x] 4.3 Confirm out-of-scope items remain untouched: no var-object/watch-tree API, no pinned multi-expression list, no persistence across reload, no inline error UI, no new `Command` variant.
