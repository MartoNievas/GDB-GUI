use crate::state::{CatchpointKind, EditTarget, WatchpointKind};
use crate::ui::command::Command;

pub fn command_to_mi(cmd: &Command) -> String {
    match cmd {
        Command::Run => "-exec-run --start".into(),
        Command::Continue => "-exec-continue".into(),
        Command::Step => "-exec-step".into(),
        Command::Next => "-exec-next".into(),
        Command::Finish => "-exec-finish".into(),
        Command::Restart => "-exec-run --start".into(),

        Command::AddBreakpoint {
            file,
            line,
            condition,
        } => build_break_insert(file, *line, condition.as_deref()),
        Command::RemoveBreakpoint(id) => format!("-break-delete {id}"),
        Command::ToggleBreakpoint { id, enable } => {
            if *enable {
                format!("-break-enable {id}")
            } else {
                format!("-break-disable {id}")
            }
        }
        Command::SetBreakpointCondition { id, condition } => build_break_condition(*id, condition),
        Command::ProbeMainSource => "-break-insert -t main".into(),

        Command::AddWatchpoint { expr, kind } => build_break_watch(*kind, expr),
        Command::RemoveWatchpoint(id) => format!("-break-delete {id}"),
        Command::ToggleWatchpoint { id, enable } => {
            if *enable {
                format!("-break-enable {id}")
            } else {
                format!("-break-disable {id}")
            }
        }

        Command::AddCatchpoint { kind, args } => build_catch_command(*kind, args),
        Command::RemoveCatchpoint(id) => format!("-break-delete {id}"),
        Command::ToggleCatchpoint { id, enable } => {
            if *enable {
                format!("-break-enable {id}")
            } else {
                format!("-break-disable {id}")
            }
        }

        Command::LoadExecutable(path) => format!("-file-exec-and-symbols {path}"),

        Command::RequestLocals => "-stack-list-variables --all-values".into(),

        Command::RequestStack => "-stack-list-frames".into(),

        Command::RequestRegisterNames => "-data-list-register-names".into(),

        Command::RequestRegisters => "-data-list-register-values x".into(),

        Command::RequestDisasm => "-data-disassemble -s $pc -e \"$pc + 64\" -- 0".into(),

        Command::RequestThreads => "-thread-info".into(),
        Command::SelectThread(id) => format!("-thread-select {id}"),

        Command::Evaluate(expr) => format!("-data-evaluate-expression {}", quote_mi(expr)),

        Command::RequestGlobalNames => "-symbol-info-variables".into(),
        Command::EvaluateGlobal(name) => format!("-data-evaluate-expression {name}"),

        Command::SetValue { target, value } => build_gdb_set(target, value),

        Command::Raw(s) => s.clone(),

        // Interrupt is not an MI command: it is dispatched as a signal (SIGINT)
        // from `dispatch`, which intercepts it before it gets here.
        Command::Interrupt => {
            unreachable!("Interrupt is signal-dispatched via dispatch(), never MI")
        }
    }
}

/// How a `Command` must be executed: as MI text written to GDB's stdin, or
/// as a signal to the process.
///
/// `Interrupt` is the only command issued while the inferior is RUNNING. In
/// synchronous mode GDB does not read its stdin at that point, so an
/// `-exec-interrupt` sent through the pipe would have no effect; it must be
/// sent as SIGINT instead. The rest of the commands are issued while the
/// program is stopped, when GDB does read its stdin.
pub enum GdbAction {
    /// MI text to write to GDB's stdin.
    Mi(String),
    /// Stop the inferior by sending a signal to the GDB process.
    Interrupt,
}

/// Classifies a `Command` into its corresponding transport action.
pub fn dispatch(cmd: &Command) -> GdbAction {
    match cmd {
        Command::Interrupt => GdbAction::Interrupt,
        other => GdbAction::Mi(command_to_mi(other)),
    }
}

// ─── MI argument quoting ──────────────────────────────────────────────────────

/// Quotes and escapes a value for use as a single MI command argument (used
/// exclusively for breakpoint condition expressions, never for `file:line`).
///
/// Escapes `\` then `"` (in that order, to avoid double-escaping) and wraps
/// the result in double quotes. SECURITY: strips embedded `\n`/`\r` before
/// escaping — GDB's MI reads one command per line, so an unstripped newline
/// inside a "condition" argument would let an attacker smuggle a second,
/// independent MI command into the subprocess's stdin (command injection).
pub fn quote_mi(arg: &str) -> String {
    let sanitized = strip_mi_newlines(arg);
    let escaped = sanitized.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

/// SECURITY: strips embedded `\n`/`\r` from `s`. GDB's MI reads one command
/// per line, so an unstripped newline inside a composed argument would let
/// an attacker smuggle a second, independent MI command into the
/// subprocess's stdin (command injection). Shared by `quote_mi` (quoted
/// arguments) and `build_gdb_set` (raw, unquoted arguments) so both paths
/// get the same guard.
pub fn strip_mi_newlines(s: &str) -> String {
    s.chars().filter(|&c| c != '\n' && c != '\r').collect()
}

/// Builds a `-gdb-set` command writing `value` to `target`. RAW and
/// unquoted: `-gdb-set` (like `-break-insert`) reads CLI-style text, so
/// wrapping `value` in `quote_mi`'s escaping would embed literal `\"`/`\\`
/// into the written value instead of the user's intended text. The
/// composed string is still passed through `strip_mi_newlines` as the
/// injection guard.
pub fn build_gdb_set(target: &EditTarget, value: &str) -> String {
    let mi = match target {
        EditTarget::Local(name) | EditTarget::Global(name) => {
            format!("-gdb-set var {name}={value}")
        }
        EditTarget::Register(name) => format!("-gdb-set ${name}={value}"),
    };
    strip_mi_newlines(&mi)
}

/// Builds `-break-insert [-c <quoted>] file:line`. `condition` is applied
/// only to the `-c` argument, never to `file:line`.
pub fn build_break_insert(file: &str, line: u32, condition: Option<&str>) -> String {
    match condition {
        Some(cond) => format!("-break-insert -c {} {file}:{line}", quote_mi(cond)),
        None => format!("-break-insert {file}:{line}"),
    }
}

/// Builds `-break-condition <id> <quoted>` to set/replace a condition, or
/// `-break-condition <id>` with no trailing argument to clear it (empty
/// `condition` string = clear).
pub fn build_break_condition(id: u32, condition: &str) -> String {
    if condition.is_empty() {
        format!("-break-condition {id}")
    } else {
        format!("-break-condition {id} {}", quote_mi(condition))
    }
}

/// Builds `-break-watch [-r|-a] <quoted expr>` depending on `kind` (Write
/// has no flag). `expr` always goes through `quote_mi` (which itself calls
/// `strip_mi_newlines`) — never raw-interpolated — so an expression
/// containing `"`, `\`, or an embedded newline cannot smuggle a second,
/// independent MI command into GDB's stdin.
pub fn build_break_watch(kind: WatchpointKind, expr: &str) -> String {
    let quoted = quote_mi(expr);
    match kind {
        WatchpointKind::Write => format!("-break-watch {quoted}"),
        WatchpointKind::Read => format!("-break-watch -r {quoted}"),
        WatchpointKind::Access => format!("-break-watch -a {quoted}"),
    }
}

/// Builds the MI command for creating a catchpoint of `kind`, per design
/// addendum A1 (supersedes design #65's D7 — verified live against GDB
/// 17.2, which has no native `-catch-fork`/`-catch-vfork`/`-catch-exec`/
/// `-catch-signal` MI verbs).
///
/// Fork/Vfork/Exec: `-interpreter-exec console "catch <kind>"` — `args` is
/// discarded entirely (never appended), so a stray argument on a no-arg kind
/// cannot become part of the composed command. Signal and Syscall (Phase
/// 2b, D2): same console transport, joining 0..N args with spaces
/// (`"catch signal"`/`"catch syscall"` alone when empty). Load/Unload: the
/// native `-catch-load`/`-catch-unload` verb with its one mandatory argument
/// through `quote_mi`; with zero args (A5) they degrade to the same
/// console-passthrough form as Fork/Vfork/Exec, since the native verb errors
/// on a missing library name.
///
/// SECURITY: for the console-passthrough kinds, the WHOLE composed CLI text
/// (`"catch fork"`, `"catch signal SIGINT SIGTERM"`, ...) is quoted as a
/// single MI argument via `quote_mi` — never per-arg — so an embedded `"`,
/// `\`, or newline in any arg cannot smuggle a second CLI line into GDB's
/// `catch` parser (threat matrix: "CLI-in-MI nesting").
pub fn build_catch_command(kind: CatchpointKind, args: &[String]) -> String {
    match kind {
        CatchpointKind::Fork => console_catch("catch fork"),
        CatchpointKind::Vfork => console_catch("catch vfork"),
        CatchpointKind::Exec => console_catch("catch exec"),
        CatchpointKind::Signal => {
            let joined = args.join(" ");
            if joined.is_empty() {
                console_catch("catch signal")
            } else {
                console_catch(&format!("catch signal {joined}"))
            }
        }
        CatchpointKind::Load => match args.first() {
            Some(arg) => format!("-catch-load {}", quote_mi(arg)),
            None => console_catch("catch load"),
        },
        CatchpointKind::Unload => match args.first() {
            Some(arg) => format!("-catch-unload {}", quote_mi(arg)),
            None => console_catch("catch unload"),
        },
        // D2: same console-passthrough shape as Signal — GDB 17.2 has no
        // native `-catch-syscall` MI verb. Args (name/number/`group:foo`)
        // are joined with single spaces and never validated client-side.
        CatchpointKind::Syscall => {
            let joined = args.join(" ");
            if joined.is_empty() {
                console_catch("catch syscall")
            } else {
                console_catch(&format!("catch syscall {joined}"))
            }
        }
        // Phase 2c (D2): native `-catch-throw`/`-catch-rethrow`/
        // `-catch-catch` verbs, verified live against GDB 17.2 (Phase 0
        // spike). Unlike `-catch-load`/`-catch-unload`, these verbs are
        // valid at zero arity, so there is no console-passthrough fallback
        // to degrade to.
        CatchpointKind::Throw => catch_exception("-catch-throw", args),
        CatchpointKind::Rethrow => catch_exception("-catch-rethrow", args),
        CatchpointKind::Catch => catch_exception("-catch-catch", args),
    }
}

/// Builds `<verb> [-r <quoted regexp>]` for the Phase 2c exception
/// catchpoint verbs. The regexp is optional (zero-or-one arg, D1); a
/// whitespace-only entry degrades to the bare verb, same emptiness rule as
/// every other optional-arg kind. `quote_mi` (which strips embedded `\n`/
/// `\r` before escaping `\`/`"`) applies to the regexp alone — verb and
/// `-r` are literals, so there is no CLI-in-MI nesting to guard here, only
/// the single quoted argument (unlike the console-passthrough kinds above).
fn catch_exception(verb: &str, args: &[String]) -> String {
    match args.first() {
        Some(r) if !r.trim().is_empty() => format!("{verb} -r {}", quote_mi(r)),
        _ => verb.to_string(),
    }
}

/// Wraps `cli` (a raw, unescaped GDB CLI command line) as a single
/// `-interpreter-exec console <quoted>` MI command. `quote_mi` is applied to
/// the whole composed line, not to any sub-part — see `build_catch_command`.
fn console_catch(cli: &str) -> String {
    format!("-interpreter-exec console {}", quote_mi(cli))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_gdb_set_local_uses_var_form_unquoted() {
        assert_eq!(
            build_gdb_set(&EditTarget::Local("x".into()), "42"),
            "-gdb-set var x=42"
        );
    }

    #[test]
    fn build_gdb_set_global_uses_var_form_unquoted() {
        assert_eq!(
            build_gdb_set(&EditTarget::Global("g_counter".into()), "7"),
            "-gdb-set var g_counter=7"
        );
    }

    #[test]
    fn build_gdb_set_register_uses_dollar_form_unquoted() {
        assert_eq!(
            build_gdb_set(&EditTarget::Register("pc".into()), "0x400000"),
            "-gdb-set $pc=0x400000"
        );
    }

    // SECURITY (mandatory first): an embedded newline must never survive into
    // the quoted argument — otherwise it could smuggle a second, independent
    // MI command into GDB's stdin (command injection via the condition field).
    #[test]
    fn quote_mi_strips_embedded_newline() {
        let injected = "x == 1\n-exec-continue";
        let quoted = quote_mi(injected);
        assert!(
            !quoted.contains('\n'),
            "quoted MI argument must not contain a raw newline: {quoted:?}"
        );
        assert_eq!(quoted, "\"x == 1-exec-continue\"");
    }

    // SECURITY (threat-matrix: argument composition into subprocess stdin):
    // a value containing a newline must not smuggle a second, independent MI
    // command into GDB's stdin. build_gdb_set is unquoted (raw value), so it
    // relies on strip_mi_newlines rather than quote_mi's escaping.
    #[test]
    fn gdb_set_strips_embedded_newline() {
        let mi = build_gdb_set(&EditTarget::Local("x".into()), "1\n-exec-continue");
        assert!(
            !mi.contains('\n'),
            "composed -gdb-set command must not contain a raw newline: {mi:?}"
        );
        assert_eq!(mi, "-gdb-set var x=1-exec-continue");
    }

    #[test]
    fn gdb_set_strips_embedded_carriage_return() {
        let mi = build_gdb_set(&EditTarget::Register("pc".into()), "1\r-exec-continue");
        assert!(!mi.contains('\r'));
        assert_eq!(mi, "-gdb-set $pc=1-exec-continue");
    }

    #[test]
    fn quote_mi_strips_embedded_carriage_return() {
        let injected = "x == 1\r-exec-continue";
        let quoted = quote_mi(injected);
        assert!(!quoted.contains('\r'));
        assert_eq!(quoted, "\"x == 1-exec-continue\"");
    }

    #[test]
    fn quote_mi_escapes_backslash_and_quote() {
        let cases: &[(&str, &str)] = &[
            ("i == 10", "\"i == 10\""),
            ("x > 5 && y == 3", "\"x > 5 && y == 3\""),
            (r#"say "hi""#, r#""say \"hi\"""#),
            (r"C:\path", r#""C:\\path""#),
            (r#"a\"b"#, r#""a\\\"b""#),
        ];
        for (input, expected) in cases {
            assert_eq!(&quote_mi(input), expected, "input={input:?}");
        }
    }

    // SECURITY: an embedded newline in the struct-panel expression must not
    // survive into the MI command written to GDB's stdin (see quote_mi).
    #[test]
    fn evaluate_command_strips_embedded_newline() {
        let mi = command_to_mi(&Command::Evaluate("*p\n-exec-continue".into()));
        assert!(
            !mi.contains('\n'),
            "MI command must not contain a raw newline: {mi:?}"
        );
        assert_eq!(mi, "-data-evaluate-expression \"*p-exec-continue\"");
    }

    #[test]
    fn evaluate_command_quotes_expression_with_spaces() {
        let mi = command_to_mi(&Command::Evaluate("arr[i + 1]".into()));
        assert_eq!(mi, "-data-evaluate-expression \"arr[i + 1]\"");
    }

    #[test]
    fn request_threads_command_maps_to_thread_info() {
        assert_eq!(command_to_mi(&Command::RequestThreads), "-thread-info");
    }

    // SECURITY (threat-matrix: argument composition — `-thread-select <id>`
    // smuggling a 2nd MI command): `id: u32` is structurally incapable of
    // carrying a newline or any other MI-breaking character, so no `quote_mi`
    // is needed here.
    #[test]
    fn select_thread_command_maps_to_thread_select() {
        assert_eq!(command_to_mi(&Command::SelectThread(7)), "-thread-select 7");
    }

    #[test]
    fn break_insert_builder_with_condition() {
        assert_eq!(
            build_break_insert("main.c", 10, Some("x > 5 && y == 3")),
            "-break-insert -c \"x > 5 && y == 3\" main.c:10"
        );
    }

    #[test]
    fn break_insert_builder_without_condition() {
        assert_eq!(
            build_break_insert("main.c", 10, None),
            "-break-insert main.c:10"
        );
    }

    #[test]
    fn break_condition_builder_sets_condition() {
        assert_eq!(
            build_break_condition(3, "x > 5 && y == 3"),
            "-break-condition 3 \"x > 5 && y == 3\""
        );
    }

    #[test]
    fn break_condition_builder_clears_with_no_trailing_arg() {
        assert_eq!(build_break_condition(3, ""), "-break-condition 3");
    }

    // ─── Interrupt dispatch (Option A: SIGINT, not an MI command) ──────────────
    //
    // Interrupt is the only command issued while the inferior is RUNNING. In
    // synchronous mode GDB does not read its stdin then, so `-exec-interrupt`
    // written to the pipe is a no-op. It must be routed to a signal instead —
    // this test pins that routing decision.

    #[test]
    fn interrupt_is_dispatched_as_a_signal_not_mi() {
        assert!(
            matches!(dispatch(&Command::Interrupt), GdbAction::Interrupt),
            "Interrupt must be routed to the signal path, never written as MI"
        );
    }

    #[test]
    fn execution_commands_are_dispatched_as_mi() {
        match dispatch(&Command::Continue) {
            GdbAction::Mi(mi) => assert_eq!(mi, "-exec-continue"),
            GdbAction::Interrupt => panic!("Continue must be an MI command, not a signal"),
        }
    }

    // Preload-source probe: a one-shot temp breakpoint at `main`, used to
    // resolve main's source file before the user hits Run. Static literal,
    // zero interpolation — no injection surface (threat-matrix row: MI
    // argument composition into GDB stdin).
    #[test]
    fn probe_main_source_maps_to_break_insert_temp_main() {
        let mi = command_to_mi(&Command::ProbeMainSource);
        assert_eq!(mi, "-break-insert -t main");
        assert!(
            !mi.contains('\n'),
            "probe MI command must not contain a raw newline: {mi:?}"
        );
    }

    // ─── Watchpoints ────────────────────────────────────────────────────────

    #[test]
    fn build_break_watch_write_has_no_flag() {
        assert_eq!(
            build_break_watch(WatchpointKind::Write, "x"),
            "-break-watch \"x\""
        );
    }

    #[test]
    fn build_break_watch_read_uses_dash_r_flag() {
        assert_eq!(
            build_break_watch(WatchpointKind::Read, "c"),
            "-break-watch -r \"c\""
        );
    }

    #[test]
    fn build_break_watch_access_uses_dash_a_flag() {
        assert_eq!(
            build_break_watch(WatchpointKind::Access, "buf[0]"),
            "-break-watch -a \"buf[0]\""
        );
    }

    #[test]
    fn add_watchpoint_command_maps_to_build_break_watch() {
        let mi = command_to_mi(&Command::AddWatchpoint {
            expr: "x".into(),
            kind: WatchpointKind::Write,
        });
        assert_eq!(mi, "-break-watch \"x\"");
    }

    #[test]
    fn remove_watchpoint_command_maps_to_break_delete() {
        assert_eq!(
            command_to_mi(&Command::RemoveWatchpoint(3)),
            "-break-delete 3"
        );
    }

    #[test]
    fn toggle_watchpoint_command_maps_to_enable_or_disable() {
        assert_eq!(
            command_to_mi(&Command::ToggleWatchpoint {
                id: 3,
                enable: true
            }),
            "-break-enable 3"
        );
        assert_eq!(
            command_to_mi(&Command::ToggleWatchpoint {
                id: 3,
                enable: false
            }),
            "-break-disable 3"
        );
    }

    // SECURITY (threat-matrix: MI injection into stdin — `\n`/`\r`/`"`/`\`/a
    // second-MI-command payload must stay inert inside a single MI line).
    // One test per adversarial character class, mirroring quote_mi's own
    // security tests.
    #[test]
    fn build_break_watch_strips_embedded_newline() {
        let mi = build_break_watch(WatchpointKind::Write, "x\n-exec-continue");
        assert!(!mi.contains('\n'));
        assert_eq!(mi, "-break-watch \"x-exec-continue\"");
    }

    #[test]
    fn build_break_watch_strips_embedded_carriage_return() {
        let mi = build_break_watch(WatchpointKind::Read, "x\r-exec-continue");
        assert!(!mi.contains('\r'));
        assert_eq!(mi, "-break-watch -r \"x-exec-continue\"");
    }

    #[test]
    fn build_break_watch_escapes_embedded_quote() {
        let mi = build_break_watch(WatchpointKind::Access, r#"say("hi")"#);
        assert_eq!(mi, r#"-break-watch -a "say(\"hi\")""#);
    }

    #[test]
    fn build_break_watch_escapes_embedded_backslash() {
        let mi = build_break_watch(WatchpointKind::Write, r"C:\path");
        assert_eq!(mi, r#"-break-watch "C:\\path""#);
    }

    #[test]
    fn build_break_watch_second_mi_command_payload_stays_a_single_line() {
        // A full second MI command embedded via newline must collapse into
        // the same single MI line as inert text, never splitting into two
        // commands written to GDB's stdin.
        let mi = build_break_watch(WatchpointKind::Write, "x\n-exec-run");
        assert_eq!(mi.lines().count(), 1);
        assert_eq!(mi, "-break-watch \"x-exec-run\"");
    }

    // ─── Catchpoints (design addendum A1) ────────────────────────────────────

    #[test]
    fn build_catch_command_fork_vfork_exec_are_console_passthrough() {
        assert_eq!(
            build_catch_command(CatchpointKind::Fork, &[]),
            "-interpreter-exec console \"catch fork\""
        );
        assert_eq!(
            build_catch_command(CatchpointKind::Vfork, &[]),
            "-interpreter-exec console \"catch vfork\""
        );
        assert_eq!(
            build_catch_command(CatchpointKind::Exec, &[]),
            "-interpreter-exec console \"catch exec\""
        );
    }

    // Fork/Vfork/Exec must discard a stray argument entirely, never append
    // it — the arg is composed nowhere in the resulting command text.
    #[test]
    fn build_catch_command_fork_vfork_exec_discard_stray_args() {
        let hostile = vec!["evil\n-exec-run".to_string()];
        assert_eq!(
            build_catch_command(CatchpointKind::Fork, &hostile),
            "-interpreter-exec console \"catch fork\""
        );
        assert_eq!(
            build_catch_command(CatchpointKind::Vfork, &hostile),
            "-interpreter-exec console \"catch vfork\""
        );
        assert_eq!(
            build_catch_command(CatchpointKind::Exec, &hostile),
            "-interpreter-exec console \"catch exec\""
        );
    }

    #[test]
    fn build_catch_command_signal_no_args_is_bare_catch_signal() {
        assert_eq!(
            build_catch_command(CatchpointKind::Signal, &[]),
            "-interpreter-exec console \"catch signal\""
        );
    }

    #[test]
    fn build_catch_command_signal_joins_multiple_args_with_spaces() {
        let args = vec!["SIGINT".to_string(), "SIGTERM".to_string()];
        assert_eq!(
            build_catch_command(CatchpointKind::Signal, &args),
            "-interpreter-exec console \"catch signal SIGINT SIGTERM\""
        );
    }

    #[test]
    fn build_catch_command_load_with_arg_uses_native_verb() {
        assert_eq!(
            build_catch_command(CatchpointKind::Load, &["libfoo".to_string()]),
            "-catch-load \"libfoo\""
        );
    }

    #[test]
    fn build_catch_command_unload_with_arg_uses_native_verb() {
        assert_eq!(
            build_catch_command(CatchpointKind::Unload, &["libfoo".to_string()]),
            "-catch-unload \"libfoo\""
        );
    }

    // A5: no-arg Load/Unload degrade to console passthrough — native
    // `-catch-load`/`-catch-unload` errors on a missing library name.
    #[test]
    fn build_catch_command_load_unload_no_arg_degrades_to_console() {
        assert_eq!(
            build_catch_command(CatchpointKind::Load, &[]),
            "-interpreter-exec console \"catch load\""
        );
        assert_eq!(
            build_catch_command(CatchpointKind::Unload, &[]),
            "-interpreter-exec console \"catch unload\""
        );
    }

    // SECURITY (threat-matrix: MI text -> GDB stdin AND CLI-in-MI nesting).
    // One test per adversarial character class, per variadic/argument kind.
    #[test]
    fn build_catch_command_signal_strips_embedded_newline() {
        let args = vec!["SIGINT\n-exec-run".to_string()];
        let mi = build_catch_command(CatchpointKind::Signal, &args);
        assert!(!mi.contains('\n'));
        assert_eq!(
            mi,
            "-interpreter-exec console \"catch signal SIGINT-exec-run\""
        );
    }

    #[test]
    fn build_catch_command_signal_strips_embedded_carriage_return() {
        let args = vec!["SIGINT\r-exec-run".to_string()];
        let mi = build_catch_command(CatchpointKind::Signal, &args);
        assert!(!mi.contains('\r'));
    }

    #[test]
    fn build_catch_command_signal_escapes_embedded_quote() {
        let args = vec![r#"SIG"INT"#.to_string()];
        let mi = build_catch_command(CatchpointKind::Signal, &args);
        assert_eq!(mi, "-interpreter-exec console \"catch signal SIG\\\"INT\"");
    }

    #[test]
    fn build_catch_command_signal_escapes_embedded_backslash() {
        let args = vec![r"SIG\INT".to_string()];
        let mi = build_catch_command(CatchpointKind::Signal, &args);
        assert_eq!(mi, "-interpreter-exec console \"catch signal SIG\\\\INT\"");
    }

    // The canonical injection payload — a full second MI command riding an
    // embedded newline — must collapse into one inert line, never split.
    #[test]
    fn build_catch_command_signal_second_mi_command_payload_stays_a_single_line() {
        let args = vec!["SIGINT\n-exec-run".to_string()];
        let mi = build_catch_command(CatchpointKind::Signal, &args);
        assert_eq!(mi.lines().count(), 1);
    }

    #[test]
    fn build_catch_command_load_strips_embedded_newline() {
        let mi = build_catch_command(CatchpointKind::Load, &["lib\n-exec-run".to_string()]);
        assert!(!mi.contains('\n'));
        assert_eq!(mi, "-catch-load \"lib-exec-run\"");
    }

    #[test]
    fn build_catch_command_load_escapes_embedded_quote_and_backslash() {
        let mi = build_catch_command(CatchpointKind::Load, &[r#"lib"foo\bar"#.to_string()]);
        assert_eq!(mi, "-catch-load \"lib\\\"foo\\\\bar\"");
    }

    // Phase 2b task 2.1: Syscall uses the same console-passthrough shape as
    // Signal (D2) — no native `-catch-syscall` MI verb exists in GDB 17.2.
    #[test]
    fn build_catch_command_syscall_no_args_is_bare_catch_syscall() {
        assert_eq!(
            build_catch_command(CatchpointKind::Syscall, &[]),
            "-interpreter-exec console \"catch syscall\""
        );
    }

    #[test]
    fn build_catch_command_syscall_single_named_arg() {
        assert_eq!(
            build_catch_command(CatchpointKind::Syscall, &["open".to_string()]),
            "-interpreter-exec console \"catch syscall open\""
        );
    }

    #[test]
    fn build_catch_command_syscall_joins_multiple_numeric_args_with_spaces() {
        let args = vec!["2".to_string(), "4".to_string()];
        assert_eq!(
            build_catch_command(CatchpointKind::Syscall, &args),
            "-interpreter-exec console \"catch syscall 2 4\""
        );
    }

    #[test]
    fn build_catch_command_syscall_group_selector_passes_through_verbatim() {
        assert_eq!(
            build_catch_command(CatchpointKind::Syscall, &["group:process_vm".to_string()]),
            "-interpreter-exec console \"catch syscall group:process_vm\""
        );
    }

    // Phase 2b task 2.2 (threat matrix: CLI-in-MI nesting via newline).
    #[test]
    fn build_catch_command_syscall_strips_embedded_newline_no_command_injection() {
        let args = vec!["2\n-exec-continue".to_string()];
        let mi = build_catch_command(CatchpointKind::Syscall, &args);
        assert!(!mi.contains('\n'));
        assert_eq!(mi.lines().count(), 1);
        assert_eq!(
            mi,
            "-interpreter-exec console \"catch syscall 2-exec-continue\""
        );
    }

    // Phase 2b task 5.1: combined injection matrix for Syscall — every
    // adversarial character class plus the `"; rm -rf"` shell-flavored
    // payload (harmless here: `;` is passed through verbatim as part of the
    // `catch syscall` CLI grammar, not a shell separator).
    #[test]
    fn build_catch_command_syscall_passes_semicolon_through_verbatim() {
        let args = vec!["2; rm -rf".to_string()];
        let mi = build_catch_command(CatchpointKind::Syscall, &args);
        assert_eq!(
            mi,
            "-interpreter-exec console \"catch syscall 2; rm -rf\""
        );
    }

    #[test]
    fn build_catch_command_syscall_escapes_embedded_quote() {
        let args = vec![r#"open"evil"#.to_string()];
        let mi = build_catch_command(CatchpointKind::Syscall, &args);
        assert_eq!(
            mi,
            "-interpreter-exec console \"catch syscall open\\\"evil\""
        );
    }

    #[test]
    fn build_catch_command_syscall_escapes_embedded_backslash() {
        let args = vec![r"open\evil".to_string()];
        let mi = build_catch_command(CatchpointKind::Syscall, &args);
        assert_eq!(
            mi,
            "-interpreter-exec console \"catch syscall open\\\\evil\""
        );
    }

    #[test]
    fn build_catch_command_syscall_strips_embedded_carriage_return() {
        let args = vec!["2\r-exec-run".to_string()];
        let mi = build_catch_command(CatchpointKind::Syscall, &args);
        assert!(!mi.contains('\r'));
    }

    // Combined payload: semicolon + newline + smuggled MI command all in one
    // arg must still collapse to a single inert quoted line.
    #[test]
    fn build_catch_command_syscall_combined_payload_stays_a_single_line() {
        let args = vec!["2; rm -rf\nstuff".to_string()];
        let mi = build_catch_command(CatchpointKind::Syscall, &args);
        assert_eq!(mi.lines().count(), 1);
        assert!(!mi.contains('\n'));
        assert_eq!(
            mi,
            "-interpreter-exec console \"catch syscall 2; rm -rfstuff\""
        );
    }

    #[test]
    fn add_catchpoint_command_maps_to_build_catch_command() {
        let mi = command_to_mi(&Command::AddCatchpoint {
            kind: CatchpointKind::Fork,
            args: vec![],
        });
        assert_eq!(mi, "-interpreter-exec console \"catch fork\"");
    }

    #[test]
    fn remove_catchpoint_command_maps_to_break_delete() {
        assert_eq!(
            command_to_mi(&Command::RemoveCatchpoint(3)),
            "-break-delete 3"
        );
    }

    // ─── Catchpoints — Phase 2c: C++ exceptions (D2) ─────────────────────────
    //
    // Unlike Fork/Vfork/Exec/Signal/Syscall (console passthrough) and even
    // Load/Unload (native verb but errors on a missing arg), the exception
    // verbs are valid at zero arity — no console-passthrough fallback is
    // needed. Verified live against GDB 17.2 (Phase 0 spike).

    #[test]
    fn build_catch_command_throw_rethrow_catch_bare_verb_with_no_args() {
        assert_eq!(build_catch_command(CatchpointKind::Throw, &[]), "-catch-throw");
        assert_eq!(
            build_catch_command(CatchpointKind::Rethrow, &[]),
            "-catch-rethrow"
        );
        assert_eq!(build_catch_command(CatchpointKind::Catch, &[]), "-catch-catch");
    }

    #[test]
    fn build_catch_command_throw_with_regexp_uses_dash_r() {
        assert_eq!(
            build_catch_command(CatchpointKind::Throw, &["std::runtime_error".to_string()]),
            "-catch-throw -r \"std::runtime_error\""
        );
    }

    #[test]
    fn build_catch_command_rethrow_with_regexp_uses_dash_r() {
        assert_eq!(
            build_catch_command(CatchpointKind::Rethrow, &["std::logic_error".to_string()]),
            "-catch-rethrow -r \"std::logic_error\""
        );
    }

    #[test]
    fn build_catch_command_catch_with_regexp_uses_dash_r() {
        assert_eq!(
            build_catch_command(CatchpointKind::Catch, &["MyException".to_string()]),
            "-catch-catch -r \"MyException\""
        );
    }

    // Whitespace-only arg degrades to the bare verb — same emptiness rule
    // as every other optional-arg kind (`args_from_buffer` never produces
    // this from the UI, but the command builder stays total on its own).
    #[test]
    fn build_catch_command_throw_whitespace_only_regexp_degrades_to_bare_verb() {
        assert_eq!(
            build_catch_command(CatchpointKind::Throw, &["   ".to_string()]),
            "-catch-throw"
        );
    }

    // SECURITY (threat-matrix: MI injection into stdin). One test per
    // adversarial character class, mirroring build_catch_command_load's own
    // security tests.
    #[test]
    fn build_catch_command_throw_strips_embedded_newline() {
        let mi = build_catch_command(
            CatchpointKind::Throw,
            &["std::runtime_error\n-exec-continue".to_string()],
        );
        assert!(!mi.contains('\n'));
        assert_eq!(mi.lines().count(), 1);
        assert_eq!(mi, "-catch-throw -r \"std::runtime_error-exec-continue\"");
    }

    #[test]
    fn build_catch_command_throw_strips_embedded_carriage_return() {
        let mi = build_catch_command(
            CatchpointKind::Throw,
            &["std::runtime_error\r-exec-run".to_string()],
        );
        assert!(!mi.contains('\r'));
    }

    #[test]
    fn build_catch_command_rethrow_escapes_quote_and_backslash() {
        let mi = build_catch_command(
            CatchpointKind::Rethrow,
            &[r#"foo"bar\baz"#.to_string()],
        );
        assert_eq!(mi, "-catch-rethrow -r \"foo\\\"bar\\\\baz\"");
    }

    // A full second MI command riding an embedded newline must collapse
    // into one inert quoted line, never split.
    #[test]
    fn build_catch_command_catch_second_mi_command_payload_stays_a_single_line() {
        let mi = build_catch_command(
            CatchpointKind::Catch,
            &["foo\n-exec-continue".to_string()],
        );
        assert_eq!(mi.lines().count(), 1);
        assert_eq!(mi, "-catch-catch -r \"foo-exec-continue\"");
    }

    #[test]
    fn add_catchpoint_command_throw_maps_to_build_catch_command() {
        let mi = command_to_mi(&Command::AddCatchpoint {
            kind: CatchpointKind::Throw,
            args: vec!["std::runtime_error".to_string()],
        });
        assert_eq!(mi, "-catch-throw -r \"std::runtime_error\"");
    }

    #[test]
    fn toggle_catchpoint_command_maps_to_enable_or_disable() {
        assert_eq!(
            command_to_mi(&Command::ToggleCatchpoint {
                id: 3,
                enable: true
            }),
            "-break-enable 3"
        );
        assert_eq!(
            command_to_mi(&Command::ToggleCatchpoint {
                id: 3,
                enable: false
            }),
            "-break-disable 3"
        );
    }
}
