# Proposal: Struct Panel — single-expression struct inspector

## Intent

`src/ui/panels/struct_panel.rs` is a dead stub (static "No struct selected" label, no input, no wiring). When debugging, inspecting a composite/struct variable currently requires the console. This change turns the panel into a real, always-visible inspector: type one expression, see its fully-nested value, auto-refreshed on every pause.

## Scope

### In Scope
- Free-text expression input in the struct panel, commit-on-Enter/blur (one evaluation per commit, not per keystroke).
- Evaluate via the existing dead `Command::Evaluate(String)` → `-data-evaluate-expression`; display the returned pretty-printed nested value as one string.
- Token-based reply correlation for the struct evaluate command (distinct from the FIFO-based `pending_globals` path) so a struct reply is never siphoned by the global path.
- Newline-strip sanitization of the struct expression before it is sent to GDB.
- Auto re-evaluate the current expression on each pause, mirroring the existing `was_paused` globals refresh in `app.rs`.
- Errors surface through the existing console `[ERROR] ...` path (`UiEvent::GdbError`).

### Out of Scope
- GDB var-object / watch-tree API (chosen approach is flat evaluate-and-display).
- Pinned list of multiple expressions (single replaceable field only).
- Expression persistence across reload; pointer auto-deref. Acceptable v1 limitations.
- Inline error UI (console reuse is sufficient).
- Any PR/branching work — single local change.

## Capabilities

### New Capabilities
- `struct-inspection`: evaluate and display a single user-supplied composite/struct expression, auto-refreshed on pause.

### Modified Capabilities
- None.

## Approach

Wire `Command::Evaluate` end-to-end. Add an `App` expression-buffer field (precedent: `bp_cond_buffer`) plus the committed expression. Reuse `breakpoints::should_commit_breakpoint_condition` as the commit-on-blur template. Correlate the reply by MI token via a `HashMap`, mirroring the proven `pending_cond` pattern in `process.rs`; check it before the `pending_globals` bare-value fallback. Sanitize the expression (strip `\n`/`\r`) as `quote_mi` does for conditions. On pause, re-send the committed expression like the globals refresh.

## Affected Areas

| Area | Impact | Description |
|------|--------|-------------|
| `src/ui/panels/struct_panel.rs` | Modified | Input + result rendering, commit logic |
| `src/ui/app.rs` | Modified | Expression state fields, pause re-eval, result plumbing |
| `src/gdb/process.rs` | Modified | Token-keyed struct-reply correlation before global FIFO |
| `src/gdb/writer.rs` | Modified | Sanitize Evaluate expression before send |
| `src/ui/command.rs` | Modified | Activate `Command::Evaluate` wiring |

## Risks

| Risk | Likelihood | Mitigation |
|------|------------|-----------|
| Struct reply misattributed to a global (silent corruption) | Med | Token correlation checked before FIFO fallback |
| MI command injection via newline in expression | Low | Strip `\n`/`\r` before send |
| Per-keystroke evaluation spam | Low | Commit-on-Enter/blur helper |

## Rollback Plan

Single local change across ~5 files; revert the commit. Panel returns to its inert stub; `Command::Evaluate` returns to dead-but-present. No persisted state or migrations.

## Dependencies

- None (all reused code already exists).

## Success Criteria

- [ ] Typing an expression and pressing Enter shows its nested value.
- [ ] Value auto-refreshes on each pause.
- [ ] A struct reply is never misattributed to a global (token-correlated).
- [ ] Newlines in the expression cannot inject a second MI command.
- [ ] Evaluation errors appear in the console as `[ERROR] ...`.
