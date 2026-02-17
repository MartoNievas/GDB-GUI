#[allow(unused_imports)]
use crate::state::{
    Breakpoint, DebuggerEvent, Frame, PauseState, StateEvent, StopReason, UiEvent, Variable,
};

pub fn parse_line(line: &str) -> Option<DebuggerEvent> {
    if line == "(gdb)" || line.is_empty() {
        return None;
    }

    let line = strip_token(line);

    match line.chars().next()? {
        '~' => parse_console_stream(line),
        '@' => parse_target_stream(line),
        '&' => None, // log interno, ignorar
        '*' => parse_exec_async(line),
        '=' => parse_notify_async(line),
        '^' => parse_result(line),
        _ => None,
    }
}

fn strip_token(line: &str) -> &str {
    let end = line.find(|c: char| !c.is_ascii_digit()).unwrap_or(0);
    &line[end..]
}

// ─── Stream outputs ───────────────────────────────────────────────────────────

fn parse_console_stream(line: &str) -> Option<DebuggerEvent> {
    // ~"some text\n"
    let text = unquote(&line[1..])?;
    Some(DebuggerEvent::Ui(UiEvent::ConsoleOutput(text)))
}

fn parse_target_stream(line: &str) -> Option<DebuggerEvent> {
    // @"some text\n"  → stdout del programa que se está depurando
    let text = unquote(&line[1..])?;
    Some(DebuggerEvent::Ui(UiEvent::ConsoleOutput(format!(
        "[target] {text}"
    ))))
}

// ─── Exec async (*) ───────────────────────────────────────────────────────────

fn parse_exec_async(line: &str) -> Option<DebuggerEvent> {
    let rest = &line[1..]; // quitar '*'
    let (class, fields) = split_class_fields(rest);

    match class {
        "running" => Some(DebuggerEvent::State(StateEvent::ProgramStarted)),

        "stopped" => {
            let reason = parse_stop_reason(fields);
            let frame = parse_frame_field(fields)?;
            let stack = vec![frame.clone()];
            let thread_id = extract_str(fields, "thread-id")
                .and_then(|s| s.parse().ok())
                .unwrap_or(1);

            Some(DebuggerEvent::State(StateEvent::ProgramPaused {
                pause: PauseState {
                    thread_id,
                    frame,
                    stack,
                    stop_reason: reason,
                },
            }))
        }

        _ => None,
    }
}

fn parse_stop_reason(fields: &str) -> StopReason {
    match extract_str(fields, "reason").as_deref() {
        Some("breakpoint-hit") => {
            let id = extract_str(fields, "bkptno")
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            StopReason::BreakpointHit(id)
        }
        Some("end-stepping-range") | Some("step-over-range") => StopReason::EndStepping,
        Some("signal-received") => {
            let sig = extract_str(fields, "signal-name").unwrap_or_default();
            StopReason::Signal(sig)
        }
        _ => StopReason::Unknown,
    }
}

// ─── Notify async (=) ─────────────────────────────────────────────────────────

fn parse_notify_async(line: &str) -> Option<DebuggerEvent> {
    let rest = &line[1..];
    let (class, fields) = split_class_fields(rest);

    match class {
        "breakpoint-created" | "breakpoint-modified" => {
            let bp = parse_breakpoint_field(fields, "bkpt")?;
            Some(DebuggerEvent::State(StateEvent::BreakpointAdded {
                breakpoint: bp,
            }))
        }
        "breakpoint-deleted" => {
            let id = extract_str(fields, "id").and_then(|s| s.parse().ok())?;
            Some(DebuggerEvent::State(StateEvent::BreakpointRemoved { id }))
        }
        _ => None,
    }
}

// ─── Result (^) ───────────────────────────────────────────────────────────────

fn parse_result(line: &str) -> Option<DebuggerEvent> {
    let rest = &line[1..];
    let (class, fields) = split_class_fields(rest);

    match class {
        "error" => {
            let msg = extract_str(&fields, "msg").unwrap_or_else(|| "GDB error".into());
            Some(DebuggerEvent::Ui(UiEvent::GdbError(msg)))
        }

        "done" => {
            // -break-insert → ^done,bkpt={...}
            if fields.contains("bkpt=") {
                if let Some(bp) = parse_breakpoint_field(&fields, "bkpt") {
                    return Some(DebuggerEvent::State(StateEvent::BreakpointAdded {
                        breakpoint: bp,
                    }));
                }
            }

            // -stack-list-variables → ^done,variables=[...]
            if fields.contains("variables=") {
                let vars = parse_variables(fields);
                if !vars.is_empty() {
                    return Some(DebuggerEvent::State(StateEvent::LocalsUpdated { vars }));
                }
            }

            // -data-list-register-names → ^done,register-names=["rax","rbx",...]
            if fields.contains("register-names=") {
                let names = parse_register_names(fields);
                return Some(DebuggerEvent::State(StateEvent::RegisterNamesReceived {
                    names,
                }));
            }

            // -data-list-register-values → ^done,register-values=[{number="0",value="0x..."}...]
            if fields.contains("register-values=") {
                let regs = parse_registers(fields);
                if !regs.is_empty() {
                    return Some(DebuggerEvent::State(StateEvent::RegistersUpdated {
                        registers: regs,
                    }));
                }
            }

            // -data-disassemble → ^done,asm_insns=[{address="0x...",inst="..."}...]
            if fields.contains("asm_insns=") {
                let lines = parse_disasm(fields);
                if !lines.is_empty() {
                    return Some(DebuggerEvent::State(StateEvent::DisasmUpdated { lines }));
                }
            }

            None
        }

        "running" => Some(DebuggerEvent::State(StateEvent::ProgramStarted)),

        "exit" => Some(DebuggerEvent::State(StateEvent::ProgramExited {
            code: None,
        })),

        _ => None,
    }
}

pub fn extract_str(fields: &str, key: &str) -> Option<String> {
    let needle = format!("{key}=\"");
    let start = fields.find(&needle)? + needle.len();
    let rest = &fields[start..];
    let end = find_closing_quote(rest)?;
    Some(unescape(&rest[..end]))
}

fn extract_block<'a>(fields: &'a str, key: &str) -> Option<&'a str> {
    let needle = format!("{key}={{");
    let start = fields.find(&needle)? + needle.len();
    let rest = &fields[start..];
    let end = find_closing_brace(rest)?;
    Some(&rest[..end])
}

fn extract_list<'a>(fields: &'a str, key: &str) -> Option<&'a str> {
    let needle_bracket = format!("{key}=[");
    if let Some(start) = fields.find(&needle_bracket) {
        let rest = &fields[start + needle_bracket.len()..];
        if let Some(end) = find_closing_bracket(rest) {
            return Some(&rest[..end]);
        }
    }

    let needle_brace = format!("{key}={{");
    if let Some(start) = fields.find(&needle_brace) {
        let rest = &fields[start + needle_brace.len()..];
        if let Some(end) = find_closing_brace(rest) {
            return Some(&rest[..end]);
        }
    }

    None
}

fn parse_frame_field(fields: &str) -> Option<Frame> {
    let block = extract_block(fields, "frame")?;
    parse_frame(block)
}

fn parse_frame(block: &str) -> Option<Frame> {
    let addr = extract_str(block, "addr")
        .and_then(|s| u64::from_str_radix(s.trim_start_matches("0x"), 16).ok())
        .unwrap_or(0);
    let function = extract_str(block, "func").unwrap_or_else(|| "??".into());
    let file = extract_str(block, "fullname").or_else(|| extract_str(block, "file"));
    let line = extract_str(block, "line").and_then(|s| s.parse().ok());

    Some(Frame {
        addr,
        function,
        file,
        line,
    })
}

fn parse_breakpoint_field(fields: &str, key: &str) -> Option<Breakpoint> {
    let block = extract_block(fields, key)?;

    let id = extract_str(block, "number")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let file = extract_str(block, "fullname").or_else(|| extract_str(&block, "file"))?;
    let line = extract_str(block, "line")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let enabled = extract_str(block, "enabled")
        .map(|s| s == "y")
        .unwrap_or(true);

    Some(Breakpoint {
        id,
        file,
        line,
        enabled,
    })
}

fn parse_variables(fields: &str) -> Vec<Variable> {
    let list = match extract_list(fields, "variables") {
        Some(l) => l,
        None => return vec![],
    };

    let mut vars = vec![];

    if list.contains('{') {
        let mut rest = list;
        while let Some(start) = rest.find('{') {
            rest = &rest[start + 1..];
            if let Some(end) = find_closing_brace(rest) {
                let block = &rest[..end];
                if let Some(var) = parse_single_variable(block) {
                    vars.push(var);
                }
                rest = &rest[end + 1..];
            } else {
                break;
            }
        }
    } else {
        if let Some(var) = parse_single_variable(list) {
            vars.push(var);
        }
    }

    vars
}

fn parse_single_variable(block: &str) -> Option<Variable> {
    let name = extract_str(block, "name")?;
    let value = extract_str(block, "value").unwrap_or_default();
    let type_ = extract_str(block, "type").unwrap_or_default();

    if name.is_empty() {
        return None;
    }

    Some(Variable { name, value, type_ })
}

// ─── String utilities ─────────────────────────────────────────────────────

fn split_class_fields(s: &str) -> (&str, &str) {
    match s.find(',') {
        Some(i) => (&s[..i], &s[i + 1..]),
        None => (s, ""),
    }
}

fn unquote(s: &str) -> Option<String> {
    let s = s.trim();
    if s.starts_with('"') && s.ends_with('"') && s.len() >= 2 {
        Some(unescape(&s[1..s.len() - 1]))
    } else if s.starts_with('"') {
        Some(unescape(&s[1..]))
    } else {
        Some(s.to_owned())
    }
}

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('"') => out.push('"'),
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('\\') => out.push('\\'),
                Some(x) => {
                    out.push('\\');
                    out.push(x);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn find_closing_quote(s: &str) -> Option<usize> {
    let mut escaped = false;
    for (i, c) in s.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if c == '\\' {
            escaped = true;
            continue;
        }
        if c == '"' {
            return Some(i);
        }
    }
    None
}

fn find_closing_brace(s: &str) -> Option<usize> {
    find_closing(s, '{', '}')
}

fn find_closing_bracket(s: &str) -> Option<usize> {
    find_closing(s, '[', ']')
}

fn find_closing(s: &str, open: char, close: char) -> Option<usize> {
    let mut depth = 1usize;
    let mut in_str = false;
    let mut escaped = false;

    for (i, c) in s.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if c == '\\' {
            escaped = true;
            continue;
        }
        if c == '"' {
            in_str = !in_str;
            continue;
        }
        if in_str {
            continue;
        }
        if c == open {
            depth += 1;
        } else if c == close {
            depth -= 1;
            if depth == 0 {
                return Some(i);
            }
        }
    }
    None
}

// ─── Register names ─────────────────────────────────────────────────────────

fn parse_register_names(fields: &str) -> Vec<String> {
    let list = match extract_list(fields, "register-names") {
        Some(l) => l,
        None => return vec![],
    };

    // La lista es: "rax","rbx","rcx",... (strings separados por coma)
    let mut names = vec![];
    let mut rest = list;

    while let Some(q) = rest.find('"') {
        rest = &rest[q + 1..];
        if let Some(end) = find_closing_quote(rest) {
            names.push(rest[..end].to_owned());
            rest = &rest[end + 1..];
        } else {
            break;
        }
    }

    names
}

// ─── Registers ───────────────────────────────────────────────────────────────

fn parse_registers(fields: &str) -> Vec<crate::state::Register> {
    let list = match extract_list(fields, "register-values") {
        Some(l) => l,
        None => return vec![],
    };

    let mut regs = vec![];
    let mut rest = list;

    while let Some(start) = rest.find('{') {
        rest = &rest[start + 1..];
        if let Some(end) = find_closing_brace(rest) {
            let block = &rest[..end];
            let number = extract_str(block, "number")
                .and_then(|s| s.parse().ok())
                .unwrap_or(0u32);
            let value = extract_str(block, "value").unwrap_or_default();
            // El nombre se cruza en DebuggerState::apply usando register_names[number]
            // Aquí lo dejamos vacío; la UI lee state.register_names para el display.
            regs.push(crate::state::Register {
                number,
                name: String::new(),
                value,
            });
            rest = &rest[end + 1..];
        } else {
            break;
        }
    }

    regs
}

// ─── Disassembly ─────────────────────────────────────────────────────────────

fn parse_disasm(fields: &str) -> Vec<crate::state::AsmLine> {
    let list = match extract_list(fields, "asm_insns") {
        Some(l) => l,
        None => return vec![],
    };

    let mut lines = vec![];
    let mut rest = list;

    while let Some(start) = rest.find('{') {
        rest = &rest[start + 1..];
        if let Some(end) = find_closing_brace(rest) {
            let block = &rest[..end];
            let addr = extract_str(block, "address")
                .and_then(|s| u64::from_str_radix(s.trim_start_matches("0x"), 16).ok())
                .unwrap_or(0);
            let offset = extract_str(block, "offset")
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            let inst = extract_str(block, "inst").unwrap_or_default();
            lines.push(crate::state::AsmLine {
                addr,
                offset,
                inst,
                current: false,
            });
            rest = &rest[end + 1..];
        } else {
            break;
        }
    }

    lines
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_token() {
        assert_eq!(strip_token("42^done"), "^done");
        assert_eq!(strip_token("^done"), "^done");
        assert_eq!(
            strip_token("*stopped,reason=\"end-stepping-range\""),
            "*stopped,reason=\"end-stepping-range\""
        );
    }

    #[test]
    fn test_extract_str() {
        let s = r#"number="1",file="main.c",line="42",enabled="y""#;
        assert_eq!(extract_str(s, "number"), Some("1".into()));
        assert_eq!(extract_str(s, "file"), Some("main.c".into()));
        assert_eq!(extract_str(s, "line"), Some("42".into()));
        assert_eq!(extract_str(s, "missing"), None);
    }

    #[test]
    fn test_parse_running() {
        let event = parse_line("*running,thread-id=\"all\"");
        assert!(matches!(
            event,
            Some(DebuggerEvent::State(StateEvent::ProgramStarted))
        ));
    }

    #[test]
    fn test_parse_error() {
        let event = parse_line("^error,msg=\"No symbol table\"");
        assert!(matches!(
            event,
            Some(DebuggerEvent::Ui(UiEvent::GdbError(_)))
        ));
    }

    #[test]
    fn test_console_stream() {
        let event = parse_line("~\"Breakpoint 1 at 0x1234\\n\"");
        assert!(matches!(
            event,
            Some(DebuggerEvent::Ui(UiEvent::ConsoleOutput(_)))
        ));
    }

    #[test]
    fn test_ignore_prompt() {
        assert!(parse_line("(gdb)").is_none());
        assert!(parse_line("").is_none());
    }
}
