use crate::ui::command::Command;
pub fn command_to_mi(cmd: &Command) -> String {
    match cmd {
        Command::Run => "-exec-run".into(),
        Command::Continue => "-exec-continue".into(),
        Command::Step => "-exec-step".into(),
        Command::Next => "-exec-next".into(),
        Command::Finish => "-exec-finish".into(),
        Command::Interrupt => "-exec-interrupt".into(),
        Command::Restart => "-exec-run".into(),

        Command::AddBreakpoint { file, line } => {
            format!("-break-insert {file}:{line}")
        }

        Command::RemoveBreakpoint(id) => {
            format!("-break-delete {id}")
        }

        Command::ToggleBreakpoint { id, enable } => {
            if *enable {
                format!("-break-enable {id}")
            } else {
                format!("-break-disable {id}")
            }
        }

        Command::LoadExecutable(path) => {
            format!("-file-exec-and-symbols {path}")
        }

        Command::RequestLocals => {
            // Pide variables del frame actual con nombre + valor + tipo
            "-stack-list-variables --all-values".into()
        }

        Command::RequestStack => "-stack-list-frames".into(),

        Command::Evaluate(expr) => {
            format!("-data-evaluate-expression {expr}")
        }

        Command::Raw(s) => s.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_commands() {
        assert_eq!(command_to_mi(&Command::Run), "-exec-run");
        assert_eq!(command_to_mi(&Command::Continue), "-exec-continue");
        assert_eq!(command_to_mi(&Command::Next), "-exec-next");
        assert_eq!(command_to_mi(&Command::Step), "-exec-step");
        assert_eq!(command_to_mi(&Command::Finish), "-exec-finish");
    }

    #[test]
    fn test_breakpoint() {
        assert_eq!(
            command_to_mi(&Command::AddBreakpoint {
                file: "main.c".into(),
                line: 43
            }),
            "-break-insert main.c:42"
        );
        assert_eq!(
            command_to_mi(&Command::RemoveBreakpoint(3)),
            "-break-delete 3"
        );
    }

    #[test]
    fn test_raw() {
        assert_eq!(
            command_to_mi(&Command::Raw("info locals".into())),
            "info locals"
        );
    }
}
