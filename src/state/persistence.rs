//! Session persistence: breakpoints, watchpoints, and catchpoints are saved
//! to a per-executable TOML project file and restored on the next launch by
//! replaying add-commands through GDB (design: `persistence-serialization`,
//! decision D2). This module is pure IO + serde — no `egui`, no GDB process
//! handling. `App` (in `ui/app.rs`) owns save/load timing.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::debugger_state::{
    Breakpoint, Catchpoint, CatchpointKind, PersistentState, Watchpoint, WatchpointKind,
};

/// Current on-disk schema. Bumped only on a breaking DTO shape change; a
/// project file whose `schema_version` is higher than this is quarantined
/// read-only (design decision D7) rather than risk misparsing or clobbering
/// data written by a newer version of the app.
pub const SCHEMA_VERSION: u32 = 1;

/// One persisted breakpoint entry. Carries only user intent — no
/// GDB-assigned `id`, no transient `condition_error` (spec: "PersistentState
/// Serialization Boundary").
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BreakpointDTO {
    pub file: String,
    pub line: u32,
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<String>,
}

/// One persisted watchpoint entry. `kind` is the wire literal
/// `"write" | "read" | "access"`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WatchpointDTO {
    pub expr: String,
    pub kind: String,
    pub enabled: bool,
}

/// One persisted catchpoint entry. `kind` is GDB's own wire literal (e.g.
/// `"fork"`, `"syscall"`), matching `CatchpointKind`'s existing `Display`
/// impl exactly so no separate mapping table is needed for this direction.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CatchpointDTO {
    pub kind: String,
    #[serde(default)]
    pub args: Vec<String>,
    pub enabled: bool,
}

/// The full per-executable project file (design.md's TOML shape).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectFile {
    pub schema_version: u32,
    pub executable: String,
    #[serde(default)]
    pub breakpoints: Vec<BreakpointDTO>,
    #[serde(default)]
    pub watchpoints: Vec<WatchpointDTO>,
    #[serde(default)]
    pub catchpoints: Vec<CatchpointDTO>,
}

impl ProjectFile {
    /// Builds the DTO to save from the live domain state. The only lossy
    /// mapping is intentional: GDB-assigned `id`s and `condition_error` are
    /// dropped, and each breakpoint's *requested* line is preferred over
    /// its resolved one (design decision D3) so a restart replays the
    /// user's original intent instead of whatever GDB relocated it to.
    pub fn from_persistent(state: &PersistentState) -> Self {
        ProjectFile {
            schema_version: SCHEMA_VERSION,
            executable: state.executable.clone().unwrap_or_default(),
            breakpoints: state.breakpoints.iter().map(BreakpointDTO::from_domain).collect(),
            watchpoints: state.watchpoints.iter().map(WatchpointDTO::from_domain).collect(),
            catchpoints: state.catchpoints.iter().map(CatchpointDTO::from_domain).collect(),
        }
    }
}

impl BreakpointDTO {
    fn from_domain(b: &Breakpoint) -> Self {
        BreakpointDTO {
            file: b.file.clone(),
            line: b.requested_line.unwrap_or(b.line),
            enabled: b.enabled,
            condition: b.condition.clone(),
        }
    }
}

impl WatchpointDTO {
    fn from_domain(w: &Watchpoint) -> Self {
        WatchpointDTO {
            expr: w.expr.clone(),
            kind: watchpoint_kind_to_wire(w.kind).to_string(),
            enabled: w.enabled,
        }
    }
}

fn watchpoint_kind_to_wire(kind: WatchpointKind) -> &'static str {
    match kind {
        WatchpointKind::Write => "write",
        WatchpointKind::Read => "read",
        WatchpointKind::Access => "access",
    }
}

impl CatchpointDTO {
    fn from_domain(c: &Catchpoint) -> Self {
        CatchpointDTO {
            kind: c.kind.to_string(),
            args: c.args.clone(),
            enabled: c.enabled,
        }
    }
}

/// FNV-1a 64-bit hash, self-implemented (design decision D5): std's
/// `DefaultHasher` (SipHash) output is explicitly documented as unstable
/// across Rust releases, but the project-file name must stay stable
/// forever — a std-hash-derived filename could silently orphan every saved
/// project on a toolchain upgrade.
fn fnv1a64(bytes: &str) -> u64 {
    const FNV_OFFSET_BASIS: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x100000001b3;

    let mut hash = FNV_OFFSET_BASIS;
    for byte in bytes.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

/// Owns the root directory persisted project files live under (design
/// decision D4: injected rather than a global path fn + `env::set_var`,
/// which is unsafe/racy in edition 2024 — this keeps IO unit-testable with
/// a plain temp dir).
pub struct Store {
    pub root: PathBuf,
}

impl Store {
    /// Resolves the OS-appropriate config directory via the `directories`
    /// crate (spec "Project File Location & Schema": `~/.config/gdb-gui/`
    /// on Linux). Returns `None` when the platform has no resolvable
    /// home/config directory — the caller disables persistence for the
    /// session instead of crashing.
    pub fn from_env() -> Option<Self> {
        directories::ProjectDirs::from("", "", "gdb-gui")
            .map(|dirs| Store {
                root: dirs.config_dir().to_path_buf(),
            })
    }

    /// Path of the per-executable project file for `abs_exe`. The
    /// executable path is canonicalized before hashing (threat matrix:
    /// "Path authority") so relative, `.`-relative, or symlinked forms of
    /// the same binary resolve to the same file instead of silently
    /// diverging into two unrelated project files.
    pub fn project_path(&self, abs_exe: &str) -> PathBuf {
        let canonical = std::fs::canonicalize(abs_exe)
            .unwrap_or_else(|_| Path::new(abs_exe).to_path_buf());
        let key = canonical.to_string_lossy();
        let hash = fnv1a64(&key);
        self.root
            .join("projects")
            .join(format!("{hash:016x}.toml"))
    }

    /// Atomically writes `f` to its project file (spec: "Save Timing and
    /// Atomicity"). Writes to a sibling `.toml.tmp` file, `fsync`s it, then
    /// renames it over the final path — `rename` is atomic on the same
    /// filesystem, so a crash between the write and the rename leaves either
    /// the previous valid file (rename never happened) or the fully-written
    /// new one (rename completed), never a half-written file.
    pub fn save(&self, f: &ProjectFile) -> std::io::Result<()> {
        let path = self.project_path(&f.executable);
        let parent = path
            .parent()
            .expect("project_path always nests under root/projects/");
        std::fs::create_dir_all(parent)?;

        let tmp_path = path.with_extension("toml.tmp");
        let toml_text = toml::to_string_pretty(f)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

        {
            use std::io::Write;
            let mut file = std::fs::File::create(&tmp_path)?;
            file.write_all(toml_text.as_bytes())?;
            file.sync_all()?;
        }

        std::fs::rename(&tmp_path, &path)?;
        Ok(())
    }

    /// Loads the project file for `abs_exe`, if any (spec: "Project File
    /// Location & Schema", "Missing or Corrupt File Handling"). Never
    /// panics and never blocks startup: every failure mode degrades to a
    /// `LoadOutcome` variant the caller can react to (start clean, warn,
    /// or quarantine).
    pub fn load(&self, abs_exe: &str) -> LoadOutcome {
        let path = self.project_path(abs_exe);

        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return LoadOutcome::Absent,
            Err(e) => return LoadOutcome::Corrupt(format!("failed to read project file: {e}")),
        };

        // Peek schema_version generically first: a file from a newer schema
        // may not even match this build's DTO shape, so it must be
        // quarantined (D7) before attempting a typed deserialize that could
        // fail for the wrong reason.
        let raw: toml::Value = match toml::from_str(&text) {
            Ok(v) => v,
            Err(e) => return LoadOutcome::Corrupt(format!("invalid TOML: {e}")),
        };
        let schema_version = raw
            .get("schema_version")
            .and_then(toml::Value::as_integer)
            .unwrap_or(0);
        if schema_version > i64::from(SCHEMA_VERSION) {
            return LoadOutcome::TooNew(schema_version as u32);
        }

        let project_file: ProjectFile = match toml::from_str(&text) {
            Ok(pf) => pf,
            Err(e) => return LoadOutcome::Corrupt(format!("invalid project file shape: {e}")),
        };

        // Threat matrix "Path authority" corollary: FNV-1a64 is not
        // collision-proof. Confirm the file actually belongs to the
        // executable being loaded before trusting its contents.
        if !same_canonical_path(&project_file.executable, abs_exe) {
            return LoadOutcome::Corrupt(format!(
                "hash collision: project file at {path:?} belongs to executable {:?}, not the requested {abs_exe:?}",
                project_file.executable
            ));
        }

        LoadOutcome::Loaded(project_file)
    }
}

/// Compares two executable path strings after canonicalizing both (falling
/// back to the raw string when a path no longer exists on disk — the same
/// degrade-gracefully rule `project_path` uses).
fn same_canonical_path(a: &str, b: &str) -> bool {
    fn canonical_or_raw(p: &str) -> String {
        std::fs::canonicalize(p)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|_| p.to_string())
    }
    canonical_or_raw(a) == canonical_or_raw(b)
}

/// Outcome of `Store::load`. Every variant is a normal, expected startup
/// path — none of them should ever cause a panic or block startup.
#[derive(Debug)]
pub enum LoadOutcome {
    /// A well-formed project file for the requested executable.
    Loaded(ProjectFile),
    /// No project file exists yet for this executable.
    Absent,
    /// The file exists but could not be trusted: unparsable TOML, a shape
    /// that doesn't match this schema version, or an `executable` field
    /// that doesn't match the requested path (hash collision). Carries a
    /// human-readable explanation.
    Corrupt(String),
    /// `schema_version` is higher than this build understands. Read-only
    /// quarantine (design decision D7): never overwritten this session.
    TooNew(u32),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique_temp_dir(tag: &str) -> std::path::PathBuf {
        let mut dir = std::env::temp_dir();
        dir.push(format!(
            "gdb-gui-persistence-test-{tag}-{}-{:?}",
            std::process::id(),
            std::time::Instant::now()
        ));
        dir
    }

    // Threat matrix: "Path authority" — a relative/`.`-relative form of the
    // same executable must resolve to the same project file as its
    // canonical absolute form, or two launches of one binary would silently
    // get two different (and diverging) project files.
    #[test]
    fn project_path_stable_for_relative_and_absolute_forms_of_same_exe() {
        let dir = unique_temp_dir("fnv-stable");
        std::fs::create_dir_all(&dir).expect("create temp exe dir");
        let exe_path = dir.join("a.out");
        std::fs::write(&exe_path, b"stub").expect("write stub exe");

        let store = Store {
            root: dir.join("store_root"),
        };

        let absolute = exe_path.to_str().expect("utf8 path").to_string();
        let via_dot = format!("{}/./a.out", dir.display());

        let p1 = store.project_path(&absolute);
        let p2 = store.project_path(&via_dot);

        assert_eq!(p1, p2);

        let _ = std::fs::remove_dir_all(&dir);
    }

    // Triangulation: two different executables must never collide on the
    // same project file.
    #[test]
    fn project_path_differs_for_different_executables() {
        let dir = unique_temp_dir("fnv-distinct");
        std::fs::create_dir_all(&dir).expect("create temp exe dir");
        let exe_a = dir.join("a.out");
        let exe_b = dir.join("b.out");
        std::fs::write(&exe_a, b"a").expect("write a.out");
        std::fs::write(&exe_b, b"b").expect("write b.out");

        let store = Store {
            root: dir.join("store_root"),
        };

        let p1 = store.project_path(exe_a.to_str().unwrap());
        let p2 = store.project_path(exe_b.to_str().unwrap());

        assert_ne!(p1, p2);

        let _ = std::fs::remove_dir_all(&dir);
    }

    // A non-canonicalizable path (no such file on disk) must not panic —
    // it degrades to hashing the raw string instead of crashing the app.
    #[test]
    fn project_path_does_not_panic_for_nonexistent_exe() {
        let store = Store {
            root: unique_temp_dir("fnv-missing-root"),
        };

        let path = store.project_path("/definitely/does/not/exist/binary");

        assert!(path.to_string_lossy().ends_with(".toml"));
    }

    // ── DTO conversion ───────────────────────────────────────────────────

    // Spec "PersistentState Serialization Boundary": a live domain struct's
    // GDB-assigned `id` (and, for breakpoints, the transient
    // `condition_error`) must never reach the serialized DTO. Design
    // decision D3: the breakpoint's *requested* line survives, not the
    // resolved one, so replay reproduces the user's original intent.
    #[test]
    fn project_file_from_persistent_drops_ids_and_uses_requested_line() {
        let persistent = PersistentState {
            executable: Some("/abs/a.out".into()),
            breakpoints: vec![Breakpoint {
                id: 5,
                file: "/abs/main.c".into(),
                line: 42,
                requested_line: Some(40),
                enabled: true,
                condition: Some("i>3".into()),
                condition_error: Some("stale error, must not persist".into()),
            }],
            watchpoints: vec![Watchpoint {
                id: 9,
                expr: "counter".into(),
                kind: WatchpointKind::Write,
                enabled: false,
            }],
            catchpoints: vec![Catchpoint {
                id: 3,
                kind: CatchpointKind::Syscall,
                args: vec!["open".into()],
                enabled: true,
            }],
        };

        let project_file = ProjectFile::from_persistent(&persistent);

        assert_eq!(project_file.schema_version, SCHEMA_VERSION);
        assert_eq!(project_file.executable, "/abs/a.out");
        assert_eq!(project_file.breakpoints.len(), 1);
        assert_eq!(project_file.breakpoints[0].line, 40, "D3: requested_line wins over resolved line");
        assert_eq!(project_file.breakpoints[0].condition, Some("i>3".to_string()));
        assert_eq!(project_file.watchpoints[0].kind, "write");
        assert!(!project_file.watchpoints[0].enabled);
        assert_eq!(project_file.catchpoints[0].kind, "syscall");
        assert_eq!(project_file.catchpoints[0].args, vec!["open".to_string()]);

        let serialized = toml::to_string(&project_file).expect("serialize project file");
        assert!(
            !serialized.contains("condition_error"),
            "condition_error must never be serialized: {serialized}"
        );
        assert!(
            !serialized.contains("id ="),
            "no DTO field is named `id`: {serialized}"
        );
    }

    // Triangulation: a breakpoint with no `requested_line` falls back to
    // the resolved `line` (the other branch of D3's `unwrap_or`).
    #[test]
    fn project_file_from_persistent_falls_back_to_resolved_line_when_no_requested_line() {
        let persistent = PersistentState {
            executable: Some("/abs/a.out".into()),
            breakpoints: vec![Breakpoint {
                id: 1,
                file: "/abs/main.c".into(),
                line: 7,
                requested_line: None,
                enabled: true,
                condition: None,
                condition_error: None,
            }],
            watchpoints: vec![],
            catchpoints: vec![],
        };

        let project_file = ProjectFile::from_persistent(&persistent);

        assert_eq!(project_file.breakpoints[0].line, 7);
        assert_eq!(project_file.breakpoints[0].condition, None);
    }

    // Round trip: serializing then deserializing a ProjectFile must
    // reproduce the exact same DTO values (schema_version, executable, and
    // every list), proving the TOML shape in design.md is actually parsable.
    #[test]
    fn project_file_toml_round_trip_preserves_all_fields() {
        let project_file = ProjectFile {
            schema_version: SCHEMA_VERSION,
            executable: "/abs/path/a.out".into(),
            breakpoints: vec![BreakpointDTO {
                file: "/abs/main.c".into(),
                line: 42,
                enabled: true,
                condition: Some("i>3".into()),
            }],
            watchpoints: vec![WatchpointDTO {
                expr: "counter".into(),
                kind: "write".into(),
                enabled: true,
            }],
            catchpoints: vec![CatchpointDTO {
                kind: "syscall".into(),
                args: vec!["open".into()],
                enabled: true,
            }],
        };

        let serialized = toml::to_string(&project_file).expect("serialize");
        let round_tripped: ProjectFile = toml::from_str(&serialized).expect("deserialize");

        assert_eq!(round_tripped, project_file);
    }

    // ── Atomic save ──────────────────────────────────────────────────────

    fn empty_project_file(exe: &str) -> ProjectFile {
        ProjectFile {
            schema_version: SCHEMA_VERSION,
            executable: exe.into(),
            breakpoints: vec![],
            watchpoints: vec![],
            catchpoints: vec![],
        }
    }

    // Spec "Save Timing and Atomicity" / "Crash mid-write": a leftover temp
    // file from a previous crashed save must not survive (nor corrupt) a
    // subsequent successful save — the rename-based swap must fully replace
    // it, leaving exactly the new content and no `.tmp` residue.
    #[test]
    fn save_leaves_no_temp_file_and_replaces_a_leftover_crash_temp_file() {
        let dir = unique_temp_dir("save-atomic");
        let store = Store { root: dir.clone() };
        let exe = "/abs/atomic-exe";
        let project_file = ProjectFile {
            breakpoints: vec![BreakpointDTO {
                file: "/abs/main.c".into(),
                line: 3,
                enabled: true,
                condition: None,
            }],
            ..empty_project_file(exe)
        };

        let path = store.project_path(exe);
        std::fs::create_dir_all(path.parent().expect("project file has a parent dir"))
            .expect("create parent dir");
        let tmp_path = path.with_extension("toml.tmp");
        std::fs::write(&tmp_path, b"garbage left over from a previous crash")
            .expect("seed leftover temp file");

        store.save(&project_file).expect("save should succeed");

        assert!(
            !tmp_path.exists(),
            "no temp file must remain after a successful save"
        );
        let saved: ProjectFile =
            toml::from_str(&std::fs::read_to_string(&path).expect("read saved file"))
                .expect("saved file must parse");
        assert_eq!(saved, project_file);

        let _ = std::fs::remove_dir_all(&dir);
    }

    // Triangulation: `Store::save` must create every missing parent
    // directory (`~/.config/gdb-gui/projects/` may not exist on first run).
    #[test]
    fn save_creates_missing_parent_directories() {
        let dir = unique_temp_dir("save-mkdir");
        let store = Store {
            root: dir.join("nested").join("root"),
        };
        let exe = "/abs/mkdir-exe";
        let project_file = empty_project_file(exe);

        store
            .save(&project_file)
            .expect("save should create parent dirs and succeed");

        let path = store.project_path(exe);
        assert!(path.exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── Load ─────────────────────────────────────────────────────────────

    #[test]
    fn load_returns_absent_when_no_project_file_exists() {
        let dir = unique_temp_dir("load-absent");
        let store = Store { root: dir.clone() };

        let outcome = store.load("/abs/never-saved-exe");

        assert!(matches!(outcome, LoadOutcome::Absent));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_returns_loaded_for_a_well_formed_matching_file() {
        let dir = unique_temp_dir("load-ok");
        let store = Store { root: dir.clone() };
        let exe = "/abs/ok-exe";
        let project_file = empty_project_file(exe);
        store.save(&project_file).expect("seed file via save");

        let outcome = store.load(exe);

        match outcome {
            LoadOutcome::Loaded(loaded) => assert_eq!(loaded, project_file),
            other => panic!("expected Loaded, got {other:?}"),
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    // Spec "Missing or Corrupt File Handling": invalid TOML must never
    // crash or block startup — it degrades to a reported `Corrupt` outcome.
    #[test]
    fn load_returns_corrupt_for_unparsable_toml() {
        let dir = unique_temp_dir("load-corrupt");
        let store = Store { root: dir.clone() };
        let exe = "/abs/corrupt-exe";
        let path = store.project_path(exe);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"this is not valid [[[ toml").unwrap();

        let outcome = store.load(exe);

        match outcome {
            LoadOutcome::Corrupt(msg) => assert!(!msg.is_empty()),
            other => panic!("expected Corrupt, got {other:?}"),
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    // Design decision D7: a `schema_version` newer than this build
    // understands must be quarantined read-only, never guessed at.
    #[test]
    fn load_returns_too_new_for_a_schema_version_above_current() {
        let dir = unique_temp_dir("load-toonew");
        let store = Store { root: dir.clone() };
        let exe = "/abs/toonew-exe";
        let path = store.project_path(exe);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            format!("schema_version = {}\nexecutable = \"{exe}\"\n", SCHEMA_VERSION + 1),
        )
        .unwrap();

        let outcome = store.load(exe);

        assert!(matches!(outcome, LoadOutcome::TooNew(v) if v == SCHEMA_VERSION + 1));

        let _ = std::fs::remove_dir_all(&dir);
    }

    // Threat matrix "Path authority" corollary: FNV-1a64 is not
    // collision-proof. If two distinct executable paths ever hashed to the
    // same project file, blindly trusting its contents would silently
    // restore the wrong executable's breakpoints. The file's own
    // `executable` field must match the path being loaded, or the file is
    // treated as corrupt rather than authoritative.
    #[test]
    fn load_returns_corrupt_when_executable_field_does_not_match_requested_path() {
        let dir = unique_temp_dir("load-collision");
        let store = Store { root: dir.clone() };
        let exe = "/abs/collision-exe";
        let path = store.project_path(exe);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let other = empty_project_file("/abs/a-completely-different-exe");
        std::fs::write(&path, toml::to_string(&other).unwrap()).unwrap();

        let outcome = store.load(exe);

        match outcome {
            LoadOutcome::Corrupt(msg) => {
                let lower = msg.to_lowercase();
                assert!(
                    lower.contains("collision") || lower.contains("mismatch"),
                    "message should explain the mismatch: {msg}"
                );
            }
            other => panic!("expected Corrupt (collision), got {other:?}"),
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    // `from_env` is a thin wrapper over the `directories` crate; the
    // meaningful, testable contract is the shape of the resolved root when
    // one is available at all (a stripped-down environment with no
    // resolvable home directory legitimately yields `None`, per spec — the
    // caller disables persistence for the session rather than panicking).
    #[test]
    fn from_env_resolves_a_root_directory_named_after_the_app_when_available() {
        if let Some(store) = Store::from_env() {
            assert!(
                store.root.ends_with("gdb-gui"),
                "expected root to end in the app name, got {:?}",
                store.root
            );
        }
    }
}
