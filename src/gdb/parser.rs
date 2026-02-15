use std::fmt::format;

use crate::state::{
    Breakpoint, DebuggerEvent, Frame, PauseState, StateEvent, StopReason, UiEvent, Variable,
};

fn strip_token(line: &str) -> &str {
    let end = line.find(|c: char| !c.is_ascii_digit()).unwrap_or(0);
    &line[end..]
}

// Stream outputs

fn parse_console_stream(line: &str) -> Option<DebuggerEvent> {
    let text = unquote(&line[1..])?;
    Some(DebuggerEvent::Ui(UiEvent::ConsoleOutput(text)))
}

fn parse_target_stream(line: &str) -> Option<DebuggerEvent> {
    let text = unquote(&line[1..])?;
    Some(DebuggerEvent::Ui(UiEvent::ConsoleOutput(format!(
        "[target] {text}"
    ))))
}

// Exec async (*)

fn parse_exec_async(line: &str) -> Option<DebuggerEvent> {
    let rest = &line[1..]; //erase => * token async class 
    let (class, fields) = split_class_fields(rest);

    match class {
        "running" => Some(DebuggerEvent::State(StateEvent::ProgramStarted)),

        "stopped" => {
            let reason = parse_stop_reason(&fields);
            let frame = parse_frame_field(&fields)?;
            let stack = vec![frame.clone()];
            let thread_id = extract_str(&fields, "thread-id")
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
            let id = extract_str(fields, "bktpno")
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            StopReason::BreakpointHit(id)
        }

        Some("end-stepping-range") | Some("step-over-range") => StopReason::EndStepping,

        Some("singal-received") => {
            let sig = extract_str(fields, "signal-name").unwrap_or_default();
            StopReason::Signal(sig)
        }

        _ => StopReason::Unknown,
    }
}

fn extract_block<'a>(fields: &'a str, key: &str) -> Option<&'a str> {
    let needle = format!("{key}=\"");
    let start = fields.find(&needle)? + needle.len();
    let rest = &fields[start..];
    let end = find_closing_brace(rest)?;
    Some(&rest[end..])
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

fn extract_str(fields: &str, key: &str) -> Option<String> {
    let needle = format!("{key}=\"");
    let start = fields.find(&needle)? + needle.len();
    let rest = &fields[start..];
    let end = find_closing_quote(rest)?;
    Some(unescape(&rest[..end]))
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
            depth += 1
        } else if c == close {
            depth -= 1;
            if depth == 0 {
                return Some(1);
            }
        }
    }
    None
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

fn split_class_fields(s: &str) -> (&str, &str) {
    match s.find(',') {
        Some(i) => (&s[0..i], &s[i + i..]),
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
