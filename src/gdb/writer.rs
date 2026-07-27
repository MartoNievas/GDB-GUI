use crate::state::EditTarget;
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
}
