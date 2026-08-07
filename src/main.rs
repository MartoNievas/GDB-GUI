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

    let (executable, program_args) = parse_launch_args(std::env::args().skip(1));

    thread::spawn(move || {
        gdb::run_loop(executable, program_args, cmd_rx, event_tx);
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

/// Splits raw CLI args into the debuggee executable path and its own argv,
/// mirroring `gdb`'s `gdb [gdb-options] program [program-args...]` handling:
/// the first token that doesn't start with `-` is the executable, and
/// everything after it (regardless of leading `-`) is passed through raw as
/// the debuggee's own arguments.
fn parse_launch_args(mut args: impl Iterator<Item = String>) -> (Option<String>, Vec<String>) {
    while let Some(arg) = args.next() {
        if !arg.starts_with('-') {
            return (Some(arg), args.collect());
        }
        // no gdb-gui-specific flags exist yet; unrecognized `-`-prefixed
        // tokens before the executable are currently just skipped
    }
    (None, Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_args_returns_none_and_empty() {
        let (exe, args) = parse_launch_args(std::iter::empty());
        assert_eq!(exe, None);
        assert_eq!(args, Vec::<String>::new());
    }

    #[test]
    fn executable_only_returns_it_with_empty_args() {
        let input = vec!["myprog".to_string()];
        let (exe, args) = parse_launch_args(input.into_iter());
        assert_eq!(exe, Some("myprog".to_string()));
        assert_eq!(args, Vec::<String>::new());
    }

    #[test]
    fn executable_followed_by_args_including_dash_prefixed() {
        let input = vec![
            "myprog".to_string(),
            "--flag".to_string(),
            "value".to_string(),
            "-x".to_string(),
        ];
        let (exe, args) = parse_launch_args(input.into_iter());
        assert_eq!(exe, Some("myprog".to_string()));
        assert_eq!(
            args,
            vec![
                "--flag".to_string(),
                "value".to_string(),
                "-x".to_string(),
            ]
        );
    }

    #[test]
    fn dash_prefixed_token_before_executable_is_skipped() {
        let input = vec!["--some-gdb-gui-flag".to_string(), "myprog".to_string()];
        let (exe, args) = parse_launch_args(input.into_iter());
        assert_eq!(exe, Some("myprog".to_string()));
        assert_eq!(args, Vec::<String>::new());
    }
}
