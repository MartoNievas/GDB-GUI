use crate::ui::command::Command;

pub fn command_to_mi(cmd: &Command) -> String {
    match cmd {
        Command::Run => "-exec-run --start".into(),
        Command::Continue => "-exec-continue".into(),
        Command::Step => "-exec-step".into(),
        Command::Next => "-exec-next".into(),
        Command::Finish => "-exec-finish".into(),
        Command::Interrupt => "-exec-interrupt".into(),
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

        Command::Evaluate(expr) => format!("-data-evaluate-expression {expr}"),

        Command::RequestGlobalNames => "-symbol-info-variables".into(),
        Command::EvaluateGlobal(name) => format!("-data-evaluate-expression {name}"),

        Command::Raw(s) => s.clone(),
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
    let sanitized: String = arg.chars().filter(|&c| c != '\n' && c != '\r').collect();
    let escaped = sanitized.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
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
}
