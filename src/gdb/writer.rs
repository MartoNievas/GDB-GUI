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

        Command::AddBreakpoint { file, line } => format!("-break-insert {file}:{line}"),
        Command::RemoveBreakpoint(id) => format!("-break-delete {id}"),
        Command::ToggleBreakpoint { id, enable } => {
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

        Command::Evaluate(expr) => format!("-data-evaluate-expression {expr}"),

        Command::Raw(s) => s.clone(),
    }
}
