//! Pure, panel-agnostic helper functions surfaced during the app.rs panel
//! split. Populated slice-by-slice (S1/S3/S4) with characterization tests
//! written before each extraction (strict TDD).

/// Last path component after `/`. Note: only splits on `/` — a path with
/// only `\\` separators is returned unchanged (see
/// `short_file_name_windows_style_path_is_not_split`), matching the
/// pre-extraction inline logic exactly.
pub(crate) fn short_file_name(path: &str) -> &str {
    path.split('/')
        .last()
        .or_else(|| path.split('\\').last())
        .unwrap_or(path)
}

/// Stack-frame location label: `short_file_name:line` when both file and
/// line are known, else `0x{addr:x}`.
pub(crate) fn frame_location(file: Option<&str>, line: Option<u32>, addr: u64) -> String {
    if let (Some(file), Some(line)) = (file, line) {
        let short = short_file_name(file);
        format!("{short}:{line}")
    } else {
        format!("0x{addr:x}")
    }
}

/// Registers tab row list: (name, value, class), filtering out slots with an
/// empty name (GDB reserves some), and — on x86-64 (detected via `rax` being
/// present in `register_names`) — the redundant 32-bit GP aliases
/// (`eax`/`ebx`/…) whose 64-bit counterpart is already shown.
pub(crate) fn visible_register_rows(
    register_names: &[String],
    registers: &[crate::state::Register],
) -> Vec<(String, String, crate::ui::registers::RegClass)> {
    let is_x86_64 = register_names.iter().any(|n| n == "rax");

    registers
        .iter()
        .filter_map(|r| {
            let name = register_names.get(r.number as usize)?;
            if name.is_empty() {
                return None;
            }
            if is_x86_64 && crate::ui::registers::is_x86_32_gp(name) {
                return None;
            }
            let class = crate::ui::registers::classify(name);
            Some((name.clone(), r.value.clone(), class))
        })
        .collect()
}

/// Top-bar location label: `file — function` when both are known, else the
/// program-status fallback string.
pub(crate) fn location_label(file: Option<&str>, func: Option<&str>, status: &str) -> String {
    if let (Some(file), Some(func)) = (file, func) {
        format!("{file} — {func}")
    } else {
        status.to_owned()
    }
}

/// Top-bar status label for an attached session (design D2/topbar.rs): reads
/// from `attached_pid`, not `ProgramState::Attached`, since `program`
/// overwrites to `Paused` within milliseconds of a successful attach — this
/// keeps "Attached (pid N)" visible for the whole session instead of only
/// the brief pre-pause window.
pub(crate) fn attached_status_label(pid: u32, status: &str) -> String {
    format!("Attached (pid {pid}) — {status}")
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_file_name_strips_unix_separator() {
        assert_eq!(short_file_name("src/main.rs"), "main.rs");
    }

    #[test]
    fn short_file_name_no_separator_returns_whole_string() {
        assert_eq!(short_file_name("main.rs"), "main.rs");
    }

    #[test]
    fn short_file_name_trailing_slash_returns_empty_segment() {
        // Characterizes existing behavior: split('/').last() on a
        // trailing-slash path yields the empty trailing segment.
        assert_eq!(short_file_name("src/"), "");
    }

    #[test]
    fn short_file_name_windows_style_path_is_not_split() {
        // Characterizes an existing quirk: `.split('/').last()` always
        // returns `Some(..)` for any input, so the `or_else` branch that
        // splits on `\\` never actually runs — a backslash-only path is
        // returned unchanged, backslashes included.
        assert_eq!(short_file_name("src\\main.rs"), "src\\main.rs");
    }

    #[test]
    fn frame_location_uses_short_file_name_and_line_when_present() {
        assert_eq!(
            frame_location(Some("src/main.rs"), Some(42), 0xdead),
            "main.rs:42"
        );
    }

    #[test]
    fn frame_location_falls_back_to_hex_addr_when_no_file() {
        assert_eq!(frame_location(None, None, 0xdead), "0xdead");
    }

    #[test]
    fn frame_location_falls_back_to_hex_addr_when_no_line() {
        assert_eq!(frame_location(Some("src/main.rs"), None, 0xdead), "0xdead");
    }

    fn reg(number: u32, value: &str) -> crate::state::Register {
        crate::state::Register {
            number,
            name: String::new(),
            value: value.to_string(),
        }
    }

    #[test]
    fn visible_register_rows_skips_empty_name_slots() {
        let names = vec!["rax".to_string(), String::new()];
        let registers = vec![reg(0, "0x1"), reg(1, "0x2")];
        let rows = visible_register_rows(&names, &registers);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0, "rax");
    }

    #[test]
    fn visible_register_rows_hides_x86_32_gp_when_rax_present() {
        let names = vec!["rax".to_string(), "eax".to_string()];
        let registers = vec![reg(0, "0x1"), reg(1, "0x2")];
        let rows = visible_register_rows(&names, &registers);
        // eax (the 32-bit alias) must be hidden once rax (64-bit) is present.
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0, "rax");
    }

    #[test]
    fn visible_register_rows_keeps_x86_32_gp_when_rax_absent() {
        let names = vec!["eax".to_string()];
        let registers = vec![reg(0, "0x1")];
        let rows = visible_register_rows(&names, &registers);
        // Without rax present (e.g. pure x86-32 target), eax must stay visible.
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0, "eax");
    }

    #[test]
    fn location_label_uses_file_and_function_when_both_present() {
        assert_eq!(
            location_label(Some("main.c"), Some("main"), "Paused"),
            "main.c — main"
        );
    }

    #[test]
    fn location_label_falls_back_to_status_when_file_missing() {
        assert_eq!(location_label(None, Some("main"), "Running"), "Running");
    }

    #[test]
    fn location_label_falls_back_to_status_when_function_missing() {
        assert_eq!(location_label(Some("main.c"), None, "Loaded"), "Loaded");
    }

    // ── attached_status_label (design D2/topbar.rs) ──────────────────────────

    #[test]
    fn attached_status_label_formats_pid_and_status() {
        assert_eq!(
            attached_status_label(4242, "Paused"),
            "Attached (pid 4242) — Paused"
        );
    }

    // Triangulation: a different pid/status pair must not collide with the
    // first — proves the format string is driven by its arguments, not a
    // hardcoded fake.
    #[test]
    fn attached_status_label_reflects_different_pid_and_status() {
        assert_eq!(
            attached_status_label(1, "Running"),
            "Attached (pid 1) — Running"
        );
    }
}
