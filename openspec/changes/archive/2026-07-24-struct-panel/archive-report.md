# Archive Report: struct-panel

**Change Name**: struct-panel  
**Archive Date**: 2026-07-24  
**Status**: COMPLETE  
**Verdict**: PASS WITH WARNINGS (0 CRITICAL, 1 WARNING)  

## Executive Summary

The `struct-panel` change has been successfully implemented, verified, and archived. The struct inspection capability (single-expression struct inspector) is now integrated into the main specification baseline. All 18 implementation tasks are complete with code-backed verification (cargo build and cargo test pass 74/74 tests). One pre-existing unrelated console "flush" change warning exists in the working tree but is out of scope for this change.

## Archival Details

### Specs Synced to Main Baseline

| Domain | File | Action | Details |
|--------|------|--------|---------|
| struct-inspection | `openspec/specs/struct-inspection/spec.md` | Created | 6 core requirements defining single-expression struct evaluation, auto-refresh on pause, token-based reply correlation, expression sanitization, error handling, and empty placeholder state. This is the first struct-inspection specification in the codebase. |

### Delta Spec Merge Summary

- **New capability domain**: `struct-inspection`
- **Main spec location**: `openspec/specs/struct-inspection/spec.md`
- **Merge type**: Full copy (no prior baseline existed)
- **Requirements added**: 6 core requirements
  1. Commit-on-Enter Expression Evaluation
  2. Evaluated Value Display
  3. Auto Re-evaluation on Pause
  4. Token-Based Reply Correlation
  5. Expression Sanitization Against Command Injection
  6. Error Handling via Console Log
  7. Empty Placeholder State Before Commit

### Archive Contents

Archive location: `openspec/changes/archive/2026-07-24-struct-panel/`

```
2026-07-24-struct-panel/
├── proposal.md                 (intent, scope, approach, risks, rollback plan)
├── design.md                   (technical approach, architecture decisions, data flow, file changes)
├── specs/
│   └── struct-inspection/
│       └── spec.md             (7 core requirements with scenarios)
├── tasks.md                    (4 phases, 18 tasks total — all [x] complete)
└── archive-report.md           (this file)
```

### Task Completion Status

**Total Tasks**: 18 / 18 complete

#### Phase 1: State Foundation (4 tasks)
- [x] 1.1 RED test `apply(StructValueUpdated)`
- [x] 1.2 RED test lifecycle clears
- [x] 1.3 GREEN state fields and events
- [x] 1.4 REFACTOR and verify

#### Phase 2: GDB Wire Protocol (6 tasks)
- [x] 2.1 RED test newline injection protection
- [x] 2.2 RED test quoted expression survival
- [x] 2.3 GREEN Evaluate arm with quote_mi
- [x] 2.4 RED test token-based correlation
- [x] 2.5 GREEN pending_struct map and first-check insertion
- [x] 2.6 REFACTOR and verify

#### Phase 3: UI Wiring (5 tasks)
- [x] 3.1 RED tests for commit-on-blur logic
- [x] 3.2 GREEN should_commit_struct_expr implementation
- [x] 3.3 GREEN struct panel input and result rendering
- [x] 3.4 GREEN App field and auto re-eval on pause
- [x] 3.5 REFACTOR and verify

#### Phase 4: Verification & Cleanup (3 tasks)
- [x] 4.1 Doc comment on Command::Evaluate
- [x] 4.2 Full cargo test and spec scenario manual trace
- [x] 4.3 Out-of-scope confirmation

### Files Modified in Implementation

| File | Change | Tests |
|------|--------|-------|
| `src/state/debugger_state.rs` | Added `struct_expr` and `struct_value` fields; added `StateEvent::StructValueUpdated`; implemented guarded apply and lifecycle clears | PASS |
| `src/gdb/writer.rs` | Updated `Command::Evaluate` arm to use `quote_mi(expr)` for sanitization and MI quoting | PASS |
| `src/gdb/process.rs` | Added `pending_struct: HashMap<u32, String>`; added token-based correlation check as FIRST branch before pending_globals | PASS |
| `src/ui/panels/struct_panel.rs` | Replaced stub with text input; added `should_commit_struct_expr` logic; implemented result rendering | PASS |
| `src/ui/app.rs` | Added `struct_input: String` field; added auto re-eval send in was_paused block | PASS |
| `src/ui/command.rs` | Added doc comment on `Command::Evaluate` noting it is reused and struct panel's sole producer | PASS |

### Verification Results

- **Build**: PASS (`cargo build`)
- **Tests**: PASS (74/74 tests pass)
- **Verdict**: PASS WITH WARNINGS
  - 0 CRITICAL issues
  - 1 WARNING (pre-existing unrelated console "flush" change in dirty working tree, out of scope for struct-panel)

### Spec Compliance Checklist

All specification requirements are implemented and verified:

- [x] Commit-on-Enter Expression Evaluation — input commits on Enter or blur, exactly one command per commit, no per-keystroke sends
- [x] Evaluated Value Display — struct panel displays returned value for committed expression
- [x] Auto Re-evaluation on Pause — committed expression auto re-evaluated when program pauses; no command sent if expression is empty
- [x] Token-Based Reply Correlation — struct replies correlated via MI token, checked before FIFO globals path; no misattribution
- [x] Expression Sanitization Against Command Injection — newlines and carriage returns stripped before send
- [x] Error Handling via Console Log — errors surface via existing console [ERROR] path (UiEvent::GdbError)
- [x] Empty Placeholder State Before Commit — panel shows neutral state before any expression is committed

### Design Decisions Recorded

1. **Reuse `Command::Evaluate`** — Do not add new `EvaluateStruct` variant. Correlation by token, not command identity.
2. **New `StateEvent::StructValueUpdated`** — Dedicated event carrying expr and value, enabling stale-reply detection.
3. **Reuse `quote_mi`** — Sanitization + MI-correct quoting in one function, proven pattern from breakpoint conditions.
4. **Lifecycle semantics** — `ProgramLoaded` clears both expr and value; `ProgramStarted`/`ProgramExited` clear only value, preserving expr across pause cycles.

### No Breaking Changes

- No modifications to existing public APIs beyond dead-code activation.
- No migrations required; no persisted state affected.
- Out-of-scope items remain untouched (var-object API, multi-expression lists, persistence, inline error UI).

### Rollback Path

Single atomic change across 5 files + 1 doc comment. Revert the single commit to restore inert stub state.

### Archive Validation Checklist

- [x] Main spec created and merged to `openspec/specs/struct-inspection/spec.md`
- [x] All artifacts (proposal, design, specs, tasks) moved to archive folder
- [x] Archive folder location: `openspec/changes/archive/2026-07-24-struct-panel/`
- [x] All tasks marked complete in archived `tasks.md` (18/18)
- [x] No unchecked implementation tasks remain
- [x] Verification report confirms PASS verdict
- [x] No CRITICAL issues in verification
- [x] Active changes directory is now clean (struct-panel folder will be removed by user during manual git operation)

## Notes

The struct-inspection capability is now part of the main specification baseline. Future changes to struct inspection behavior will start from `openspec/specs/struct-inspection/spec.md` and create deltas in their own change folders under `openspec/changes/`.

The single warning (pre-existing console "flush" change) is tracked separately and does not block archival of this change.
