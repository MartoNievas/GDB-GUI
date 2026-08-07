use std::sync::mpsc;
use std::thread;

mod gdb;
mod state;
mod ui;

use state::DebuggerState;
use ui::{App, command::Command};

fn main() -> eframe::Result<()> {
    let (cmd_tx, cmd_rx) = mpsc::channel::<Command>();
    let (event_tx, event_rx) = mpsc::channel::<state::DebuggerEvent>();

    let raw_args: Vec<String> = std::env::args().skip(1).collect();
    let executable = detect_executable(&raw_args);

    thread::spawn(move || {
        gdb::run_loop(executable, raw_args, cmd_rx, event_tx);
    });

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("GDB GUI")
            .with_inner_size([1400.0, 800.0]),
        ..Default::default()
    };

    eframe::run_native(
        "GDB GUI",
        native_options,
        Box::new(|_cc| {
            let state = DebuggerState::new();
            Ok(Box::new(App::new(state, event_rx, cmd_tx)))
        }),
    )
}

/// Best-effort detection of the debuggee executable path from the raw CLI
/// args, for gdb-gui's own UI bookkeeping only (topbar label, Files panel,
/// persistence's per-executable project lookup — see `StateEvent::ProgramLoaded`
/// usage). This does NOT affect what gets forwarded to the `gdb` subprocess:
/// the full, unmodified `raw_args` list is always passed through verbatim
/// (see `gdb::process::spawn_gdb`).
///
/// Mirrors real `gdb`'s own option scanning just enough to find the
/// executable: the first token that doesn't start with `-` is taken as the
/// executable, EXCEPT that the value belonging to a known value-taking flag
/// (e.g. `-ex "some command"`) is skipped rather than mistaken for the
/// executable. `--args` itself is not in the value-taking list — it's just
/// another `-`-prefixed token skipped by the base rule, so the token right
/// after it is still detected correctly.
fn detect_executable(args: &[String]) -> Option<String> {
    const VALUE_TAKING_FLAGS: &[&str] = &[
        "-ex",
        "--eval-command",
        "-x",
        "--command",
        "-d",
        "--directory",
        "-c",
        "--core",
        "-p",
        "--pid",
        "-b",
        "--baud",
        "-tty",
    ];

    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if !arg.starts_with('-') {
            return Some(arg.clone());
        }
        if VALUE_TAKING_FLAGS.contains(&arg.as_str()) {
            // Skip this flag's value so it isn't mistaken for the executable.
            iter.next();
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_args_returns_none() {
        assert_eq!(detect_executable(&[]), None);
    }

    #[test]
    fn executable_first_no_flags() {
        let args = vec!["myprog".to_string()];
        assert_eq!(detect_executable(&args), Some("myprog".to_string()));
    }

    #[test]
    fn executable_first_followed_by_multiple_ex_flags_is_not_confused() {
        // Exact bug-report shape: `gdb-gui kernel.elf -ex "..." -ex "..."` —
        // must still return the leading executable, not one of the -ex values.
        let args = vec![
            "kernel.elf".to_string(),
            "-ex".to_string(),
            "source orga2.py".to_string(),
            "-ex".to_string(),
            "target remote localhost:1234".to_string(),
        ];
        assert_eq!(detect_executable(&args), Some("kernel.elf".to_string()));
    }

    #[test]
    fn ex_flag_value_before_executable_is_skipped_not_mistaken() {
        let args = vec![
            "-ex".to_string(),
            "some value".to_string(),
            "myprog".to_string(),
        ];
        assert_eq!(detect_executable(&args), Some("myprog".to_string()));
    }

    #[test]
    fn args_flag_itself_is_just_skipped_like_any_dash_token() {
        // `--args` isn't in the value-taking list, but it starts with `-` so
        // it's naturally skipped by the base rule; the executable right
        // after it is still found correctly.
        let args = vec!["--args".to_string(), "myprog".to_string()];
        assert_eq!(detect_executable(&args), Some("myprog".to_string()));
    }
}
