# GDB GUI

A lightweight graphical front-end for the GNU Debugger (GDB), written in Rust
with [egui](https://github.com/emilk/egui). It drives GDB through its
Machine Interface (`--interpreter=mi`) and renders the debug session in a
structured, panel-based UI instead of the raw command line.

## Features

- **Source view** with the current execution line highlighted.
- **Breakpoints** — click any line in the gutter to add one; click again to remove it.
- **Conditional breakpoints** — every breakpoint has an editable *Condition* cell in
  the Breakpoints panel. Type a C expression (`i == 10`, `count > 3`) and the
  breakpoint only fires when it holds; clear the cell to make it unconditional.
  If GDB rejects the expression, a ⚠ marker appears next to the row with GDB's
  error message on hover.
- **Execution control** — Run, Continue, Step, Next (step over), Finish (step out),
  Interrupt (pause a freely running program) and Restart.
- **Call stack** — full backtrace of the paused thread.
- **Locals & globals** — variables in scope plus program-level globals, with live values.
- **Registers** — general-purpose registers for the current architecture
  (x86-64, x86-32, ARM64, RISC-V), sorted in a conventional order.
- **Disassembly** — instructions around the program counter, with the current
  instruction marked.
- **Integrated GDB console** — type raw GDB/MI commands and see GDB's output.
- **Resizable panels** — drag the dividers between the side sections, the console
  and the source view.

## Requirements

- **Rust** (stable) and Cargo — edition 2024.
- **GDB** with MI support (`gdb --interpreter=mi`). Tested against GDB 17.2.
- The program you want to debug should be compiled with debug symbols, e.g.:

  ```bash
  gcc -g -O0 -o myprogram myprogram.c
  ```

  Without `-g` you can still step through machine code, but source, locals and
  line information will be unavailable.

## Build & Run

```bash
# Build
cargo build

# Run, passing the executable to debug as the first argument
cargo run -- ./myprogram
```

The executable argument is optional — you can also launch the GUI on its own and
load a binary later through the GDB console (`-file-exec-and-symbols <path>`).

## Usage

1. **Load** — pass your binary on the command line (see above).
2. **Set breakpoints** — click a line number in the source gutter. A red dot
   marks lines with an active breakpoint. Click it again to delete it.
3. **Add a condition** *(optional)* — in the Breakpoints panel, type an expression
   in the *Condition* cell of a row and press <kbd>Enter</kbd> (or click away).
   The command is only sent when the text actually changed, so typing costs
   nothing until you commit. Emptying the cell removes the condition.
4. **Run** — press *Run* to start the program (it stops at `main`).
5. **Step** — use *Step* / *Next* / *Finish* / *Continue* to move through the code.
   *Interrupt* stops a program that is running freely.
6. **Inspect** — while paused, the right-hand panel shows the call stack, locals,
   globals, registers and disassembly. The status bar (top right) shows the
   current state and location.
7. **Console** — the bottom panel echoes GDB's MI traffic and accepts commands.
   Type a command and press <kbd>Enter</kbd> (plain CLI commands like
   `info registers` work too).

## Architecture

The app runs on two threads communicating over `std::sync::mpsc` channels:

- **UI thread** (`src/ui/`) — the egui/eframe event loop. It renders state and
  sends `Command`s.
- **GDB thread** (`src/gdb/process.rs::run_loop`) — spawns `gdb --interpreter=mi`,
  writes MI commands to its stdin, and reads its stdout on a dedicated reader
  thread. Output lines are parsed into `DebuggerEvent`s and sent back to the UI.

```
UI  ──Command──▶  GDB thread  ──MI──▶  gdb process
UI  ◀─Event────  GDB thread  ◀─MI──  gdb process
```

Key modules:

| Path                              | Responsibility                                        |
| --------------------------------- | ----------------------------------------------------- |
| `src/gdb/process.rs`              | Spawns GDB, pumps commands/output between threads.    |
| `src/gdb/writer.rs`               | Translates a `Command` into an MI command string.     |
| `src/gdb/parser.rs`               | Parses MI output records into `DebuggerEvent`s.       |
| `src/state/debugger_state.rs`     | The `DebuggerState` model and how events mutate it.   |
| `src/ui/app.rs`                   | The egui shell: window layout and panel skeletons.    |
| `src/ui/panels/`                  | One module per panel's content (see below).           |
| `src/ui/widgets.rs`, `theme.rs`   | Shared small widgets and the colour/typography scale. |
| `src/ui/registers.rs`             | Register classification and per-architecture order.   |
| `src/ui/command.rs`               | The `Command` enum the UI sends to GDB.               |

### UI layout

`app.rs` used to hold every panel's rendering code. It was split so that each
panel owns its own module and `app.rs` keeps only the egui skeleton — the
`egui::SidePanel` / `TopBottomPanel` / `ScrollArea` scaffolding — and the shared
`App` state. Every module under `src/ui/panels/` exposes the same entry point:

```rust
pub(crate) fn render(app: &mut App, ui: &mut egui::Ui)
```

(`console` additionally takes `&egui::Context`.)

| Module                          | Panel                                            |
| ------------------------------- | ------------------------------------------------ |
| `panels/topbar.rs`              | Execution buttons and the status bar.            |
| `panels/breakpoints.rs`         | Breakpoint list, conditions and delete buttons.  |
| `panels/stack.rs`               | Call stack / backtrace.                          |
| `panels/watch.rs`               | Watch / Registers / Data tabs.                   |
| `panels/struct_panel.rs`        | Expansion of struct-typed values.                |
| `panels/thread.rs`              | Thread information.                              |
| `panels/files.rs`               | Source file selection.                           |
| `panels/console.rs`             | GDB console output and input line.               |
| `panels/commands.rs`            | Command shortcuts.                               |
| `panels/util.rs`                | Formatting helpers shared by the panels.         |

The split is behaviour-preserving: panels render exactly what they did before,
they are just no longer competing for the same file.

## Testing

```bash
cargo test
```

Unit tests cover the MI parser, the MI writer (including the quoting of
breakpoint conditions), the `DebuggerState` transitions, and the pure decision
functions extracted from the UI — for example `should_commit_breakpoint_condition`,
which guarantees a condition is sent once on commit and never per keystroke.

## Known limitations

- The call stack shows a single-threaded backtrace; there is no thread selector yet.
- Breakpoint conditions are validated by GDB, not by the UI: a malformed
  expression is only reported after the command round-trips.
