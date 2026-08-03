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
- **Thread switching** — the Thread panel lists every live thread (fetched via
  `-thread-info`); click a row while paused to switch GDB's active thread —
  stack, locals, registers, disassembly and the thread list itself refresh
  for the newly selected thread.
- **Locals & globals** — variables in scope plus program-level globals, with live values.
- **Registers** — general-purpose registers for the current architecture
  (x86-64, x86-32, ARM64, RISC-V), sorted in a conventional order.
- **Value editing** — click a local, global or register value cell while paused
  to edit it in place; press <kbd>Enter</kbd> (or click away) to write it to
  GDB via `-gdb-set`. The cell always reflects GDB's own value, never the
  typed text — on success it is refreshed from GDB, on error it hard-reverts
  and shows GDB's message inline with a ⚠ marker. Struct fields remain
  read-only.
- **Disassembly** — instructions around the program counter, with the current
  instruction marked.
- **Memory view (hex dump)** — enter an address expression (e.g. `$sp`,
  `0x1000`, `&my_array`) in the Memory tab to read 256 bytes via
  `-data-read-memory-bytes` and view them as a 16-bytes/row offset|hex|ASCII
  grid. Read-only; auto-refetches the same address at every subsequent
  pause. If GDB rejects the address, its error message replaces the grid.
- **Integrated GDB console** — type raw GDB/MI commands and see GDB's output.
- **Watchpoints** — click in the Watchpoints panel to add read/write/access watchpoints
  for any expression; watchpoints trigger and display old→new values in a banner when
  the memory location changes.
- **Catchpoints** — set catch points for program events (fork, vfork, exec, signal, 
  library load/unload, syscall entry/return, and C++ exceptions). Each catchpoint 
  type displays contextual information at stop time: new process ID for fork/vfork, 
  executable path for exec, library name for load/unload, syscall number/name for 
  syscall, and "Exception thrown/rethrown/caught" for C++ exception events. Syscall 
  and C++ exception catchpoints support optional regex filtering.
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

## Installation

The following dependencies are required to compile and install the project:

- **rustup** (provides the Rust compiler and Cargo).
- **cargo-make** (runs the `Makefile.toml` tasks used below).

Install `cargo-make` the same way on every platform, once `rustup`/`cargo` is set up:

```bash
cargo install cargo-make
```

### Linux/WSL

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
cargo install cargo-make
```

Build and install the binary to `/usr/local/bin` (requires `sudo`, since it writes
outside your home directory):

```bash
sudo makers install
```

Uninstall with:

```bash
sudo makers uninstall
```

## Running the installed binary

Once installed, `gdb-gui` is on your `PATH` and can be launched from any terminal
(same on Linux, macOS and Windows):

```bash
gdb-gui ./myprogram
```

The executable argument is optional — you can launch `gdb-gui` on its own and
load a binary later through the GDB console (`file [binary_path]`).

> **Note:** Do not move the source code from the directory where the debugged
> program was compiled. Debug info embeds the absolute source paths recorded
> at compile time, and `gdb-gui` resolves source files against that path first
> before falling back to paths relative to the current directory. If the
> source tree moves, source view may fail to locate the file.

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
| `panels/watch.rs`               | Watch / Registers / Disasm / Memory tabs.        |
| `panels/struct_panel.rs`        | Expansion of struct-typed values.                |
| `panels/thread.rs`              | Thread list and switching.                       |
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
breakpoint conditions and the injection-safe composition of `-gdb-set`
commands), the `DebuggerState` transitions, and the pure decision functions
extracted from the UI — for example `should_commit_breakpoint_condition` and
`should_commit_value_edit`, which guarantee a condition or value is sent once
on commit and never per keystroke.

## Known limitations

- Breakpoint conditions are validated by GDB, not by the UI: a malformed
  expression is only reported after the command round-trips.
