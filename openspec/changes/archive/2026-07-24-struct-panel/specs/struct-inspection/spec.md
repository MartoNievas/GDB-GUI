# Struct Inspection Specification

## Purpose

Defines the behavior of the struct panel: a single free-text expression input that is evaluated via GDB's `-data-evaluate-expression`, displays the resulting value, and stays correlated and auto-refreshed independently of the existing globals evaluation path.

## Requirements

### Requirement: Commit-on-Enter Expression Evaluation

The struct panel MUST send exactly one `Command::Evaluate` per committed expression. Commit MUST occur on Enter or on the input losing focus, and MUST NOT occur per keystroke.

#### Scenario: User types and presses Enter

- GIVEN the struct panel input is focused and empty
- WHEN the user types `my_struct.field` and presses Enter
- THEN exactly one evaluate command for `my_struct.field` is sent to GDB
- AND no evaluate command was sent while individual characters were typed

#### Scenario: User types and loses focus without pressing Enter

- GIVEN the struct panel input contains unsent text `other_var`
- WHEN the input loses focus (e.g. user clicks elsewhere)
- THEN exactly one evaluate command for `other_var` is sent

### Requirement: Evaluated Value Display

The struct panel MUST display the value returned by GDB for the currently committed expression once the reply arrives.

#### Scenario: Reply received for committed expression

- GIVEN an expression has been committed and its evaluate command sent
- WHEN GDB replies with the evaluated value
- THEN the panel displays that value associated with the committed expression

### Requirement: Auto Re-evaluation on Pause

The system MUST automatically re-send the currently committed expression for evaluation every time the debugged program pauses, mirroring the existing globals auto-refresh, without requiring the user to re-type or re-commit it.

#### Scenario: Program pauses at a breakpoint

- GIVEN a struct expression is already committed from a prior evaluation
- WHEN the debugged program pauses (e.g. hits a breakpoint)
- THEN the committed expression is automatically re-evaluated
- AND the displayed value updates once GDB replies

#### Scenario: No expression committed yet

- GIVEN no expression has ever been committed
- WHEN the program pauses
- THEN no evaluate command is sent for the struct panel

### Requirement: Token-Based Reply Correlation

Struct-panel evaluate replies MUST be correlated to their request via MI token, independent of the FIFO-based `pending_globals` queue. A struct reply MUST NOT be consumed or misattributed by the globals FIFO path, and a globals reply MUST NOT be misattributed to the struct panel, even when both requests are in flight at the same pause.

#### Scenario: Struct and globals evaluations in flight simultaneously

- GIVEN a globals refresh and a struct expression evaluation are both pending after the same pause
- WHEN GDB sends replies for both, in any order
- THEN the struct reply updates only the struct panel value
- AND the globals reply updates only the globals panel, unaffected by the struct request

#### Scenario: Struct reply arrives before a queued global reply

- GIVEN the struct evaluate command was sent after a globals evaluate command
- WHEN the struct reply arrives first
- THEN it is matched to the struct panel by its token
- AND the globals FIFO queue is left untouched for the still-pending global reply

### Requirement: Expression Sanitization Against Command Injection

The system MUST strip embedded newline and carriage-return characters from the struct expression before it is sent to GDB, so a single input cannot smuggle a second MI command into GDB's stdin.

#### Scenario: Expression contains an embedded newline

- GIVEN the user enters an expression containing `\n` followed by another MI-like command fragment
- WHEN the expression is committed
- THEN the `\n` and any `\r` characters are removed before the command is sent
- AND GDB receives a single evaluate command, not two

### Requirement: Error Handling via Console Log

An invalid or erroring expression MUST NOT crash the application or corrupt the state of other panels. The error MUST be surfaced via the existing console `[ERROR] ...` log path (`UiEvent::GdbError`).

#### Scenario: Expression references an unknown symbol

- GIVEN the user commits an expression referencing an undefined symbol
- WHEN GDB replies with an error
- THEN the error is logged to the console as `[ERROR] ...`
- AND the globals panel and other panels retain their prior valid state

### Requirement: Empty Placeholder State Before Commit

Before any expression has been committed, the struct panel MUST show a neutral empty/placeholder state, not stale data and not an error.

#### Scenario: Panel opened with no prior commit

- GIVEN the application has just started and no expression has been committed
- WHEN the user views the struct panel
- THEN the panel shows a neutral empty/placeholder state
- AND no error and no leftover value from a previous session are shown
