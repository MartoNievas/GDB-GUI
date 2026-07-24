# Design: Struct Panel — single-expression struct inspector

## Technical Approach

Wire the dead `Command::Evaluate(String)` end-to-end for one user expression. The
committed expression lives in `DebuggerState`; the panel's edit buffer lives in
`App`. Replies are correlated by MI token (not by FIFO order, not by command
identity) via a new `pending_struct: HashMap<u32,String>`, checked **before** the
`pending_globals` fallback so a struct reply can never be siphoned into a global.
Sanitization + MI-correct quoting reuse the proven `quote_mi`. Implements
`specs/struct-inspection/spec.md`.

## Architecture Decisions

### Decision: Reuse `Command::Evaluate`, do not add `EvaluateStruct`
**Choice**: Keep the existing `Command::Evaluate(String)`; the struct panel is its
sole producer.
**Alternatives**: A distinct `Command::EvaluateStruct` variant.
**Rationale**: Correlation is by MI token, not command identity — `pending_struct`
is populated only when an `Evaluate` is dispatched, so the token alone disambiguates
the reply. A second variant adds a parallel dead-code path with zero correlation
benefit. `Evaluate` has no other producer, so ownership is unambiguous.

### Decision: New `StateEvent::StructValueUpdated { expr, value }`
**Choice**: Add a dedicated variant rather than reuse `GlobalValueUpdated`.
**Rationale**: `GlobalValueUpdated` pushes a named row into the `globals` Vec; the
struct is a single scalar field with different semantics. Carrying `expr` lets
`apply()` drop a stale reply whose expression no longer matches the committed one.

### Decision: Reuse `quote_mi` for the Evaluate expression
**Choice**: `Command::Evaluate(expr) => format!("-data-evaluate-expression {}", quote_mi(expr))`.
**Alternatives**: A narrow `strip_newlines`-only helper leaving the expression bare.
**Rationale**: `quote_mi` strips `\n`/`\r` (the injection mitigation) **and** wraps
in a quoted C-string. MI's `-data-evaluate-expression` accepts (and for
multi-token expressions like `arr[i + 1]` requires) a quoted argument — bare
interpolation would let MI tokenize on spaces and misparse. Backslash/quote
escaping round-trips correctly through GDB's MI string unescaping. No new helper
needed; existing `quote_mi` tests already pin newline stripping.

### Decision: `struct_expr` persists across run/pause/exit, clears only on load
**Choice**: `ProgramLoaded` clears both `struct_expr` and `struct_value`;
`ProgramStarted`/`ProgramExited` clear only `struct_value`, keeping `struct_expr`.
**Rationale**: Auto-refresh on every pause requires the expression to survive
run→pause→continue cycles. "No persistence across reload" (out of scope) means only
a *new executable* (`ProgramLoaded`) resets it, since symbols change.

## Data Flow

    struct_panel (commit-on-blur)
      → set state.struct_expr, clear state.struct_value
      → Command::Evaluate(expr) ──cmd_tx──► process.rs dispatch
           writer: -data-evaluate-expression "<quoted>"  (token t)
           pending_struct.insert(t, expr)
      ◄── "t^done,value=..."  correlate_pending_struct (BEFORE pending_globals)
           → StateEvent::StructValueUpdated{expr,value} → apply (guarded) → panel
      ◄── "t^error,msg=..."   remove token; NO continue → parse_line → [ERROR] console

On pause, `App::update` re-sends `Command::Evaluate(state.struct_expr)` when non-empty.

## File Changes

| File | Action | Description |
|------|--------|-------------|
| `src/ui/command.rs` | Modify | No new variant; confirm `Evaluate` reused (doc only) |
| `src/gdb/writer.rs` | Modify | `Evaluate` arm → `quote_mi(expr)` for strip + MI quoting |
| `src/gdb/process.rs` | Modify | `pending_struct` map + `correlate_pending_struct`, checked first |
| `src/state/debugger_state.rs` | Modify | `struct_expr`/`struct_value` fields, event, lifecycle clears |
| `src/ui/app.rs` | Modify | `struct_input` buffer field; pause re-eval send |
| `src/ui/panels/struct_panel.rs` | Modify | Input + result render, `should_commit_struct_expr` |

## Interfaces / Contracts

```rust
// process.rs — declared beside pending_cond; populated where pending_cond is:
let mut pending_struct: HashMap<u32, String> = HashMap::new();
if let DebuggerCommand::Evaluate(expr) = &cmd { pending_struct.insert(token, expr.clone()); }
// line_rx loop, as the FIRST check (before the pending_globals block ~line 202):
//   parse token; if in pending_struct:
//     ^done,value → remove, emit StructValueUpdated{expr,value}, `continue`
//     ^error      → remove token, do NOT continue (parse_line emits GdbError)

// debugger_state.rs
pub struct_expr: String,          // committed; "" = none
pub struct_value: Option<String>, // last value; error stays console-only
StateEvent::StructValueUpdated { expr: String, value: String }
// apply: if expr == self.struct_expr { self.struct_value = Some(value); }

// app.rs — struct_input: String; inside `if was_paused` after the globals loop:
if !self.state.struct_expr.is_empty() { self.send(Command::Evaluate(self.state.struct_expr.clone())); }

// struct_panel.rs — adapted from should_commit_breakpoint_condition (committed is &str, not Option):
pub(crate) fn should_commit_struct_expr(lost_focus: bool, buffer: &str, committed: &str) -> bool {
    if !lost_focus { return false; } buffer != committed
}
// on commit: set state.struct_expr, clear state.struct_value; send Evaluate only if non-empty (empty clears)
```

## Testing Strategy

| Layer | What | Approach |
|-------|------|----------|
| Unit | `should_commit_struct_expr` | never on keystroke; once on blur-changed; skip unchanged; empty clears |
| Unit | `correlate_pending_struct` | done→event+removal; error→removal, no event; foreign token untouched |
| Unit | `command_to_mi(Evaluate)` | newline injection stripped; spaces survive quoted |
| Unit | `apply(StructValueUpdated)` | matching expr sets value; stale expr dropped; lifecycle clears |

## Threat Matrix

Process integration (MI command written to GDB stdin). Only injection-relevant rows:

| Boundary | Cases | Applicability | Design response | RED test |
|---|---|---|---|---|
| PR commands (arg composition) | expr with `\n`/`\r` smuggling a 2nd MI command | Applicable | `quote_mi` strips `\n`/`\r` before quoting | `command_to_mi(Evaluate("*p\n-exec-continue"))` has no newline |
| Documentation-like paths | — | N/A: no file classification | — | — |
| Git selection / Commit / Push | — | N/A: no VCS in this change | — | — |

## Migration / Rollout

No migration. No persisted state. Revert the commit to restore the inert stub.

## Open Questions

- None.
