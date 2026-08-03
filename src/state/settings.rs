//! App-wide settings, distinct from any single project's persisted
//! tracepoints (`persistence.rs`). Currently holds only the persistence
//! enable/disable switch (spec: "Persistence Enable/Disable Setting").

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// App-wide user settings, persisted once at `~/.config/gdb-gui/settings.toml`
/// — independent of any per-executable project file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default = "default_persistence_enabled")]
    pub persistence_enabled: bool,
}

fn default_persistence_enabled() -> bool {
    true
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            persistence_enabled: default_persistence_enabled(),
        }
    }
}

/// Whether persistence is effectively enabled. `no_persist_env` is the raw
/// `GDB_GUI_NO_PERSIST` environment value, if set — passed in explicitly
/// (rather than read here) so this stays a pure, deterministically testable
/// function with no hidden global state. Its mere *presence* disables
/// persistence, regardless of its value or the persisted setting: an
/// escape hatch for CI or debugging a suspect project file without editing
/// settings.toml.
pub fn persistence_enabled(settings: &Settings, no_persist_env: Option<&str>) -> bool {
    if no_persist_env.is_some() {
        return false;
    }
    settings.persistence_enabled
}

/// Owns the root directory `settings.toml` lives under (mirrors
/// `persistence::Store`'s injected-root pattern for the same reason: an
/// IO-unit-testable path with no unsafe global `env::set_var`).
pub struct SettingsStore {
    pub root: PathBuf,
}

impl SettingsStore {
    /// Resolves the OS-appropriate config directory via `directories`.
    /// Returns `None` when the platform has no resolvable home/config
    /// directory — the caller falls back to in-memory defaults.
    pub fn from_env() -> Option<Self> {
        directories::ProjectDirs::from("", "", "gdb-gui")
            .map(|dirs| SettingsStore {
                root: dirs.config_dir().to_path_buf(),
            })
    }

    pub fn path(&self) -> PathBuf {
        self.root.join("settings.toml")
    }

    /// Loads settings from disk, degrading to `Settings::default()` on any
    /// failure (missing file, unreadable, or unparsable) — settings must
    /// never block or crash startup, mirroring `persistence::Store::load`'s
    /// "never panic" contract.
    pub fn load(&self) -> Settings {
        std::fs::read_to_string(self.path())
            .ok()
            .and_then(|text| toml::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, settings: &Settings) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.root)?;
        let text = toml::to_string_pretty(settings)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        std::fs::write(self.path(), text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique_temp_dir(tag: &str) -> std::path::PathBuf {
        let mut dir = std::env::temp_dir();
        dir.push(format!(
            "gdb-gui-settings-test-{tag}-{}-{:?}",
            std::process::id(),
            std::time::Instant::now()
        ));
        dir
    }

    #[test]
    fn default_settings_has_persistence_enabled() {
        assert!(Settings::default().persistence_enabled);
    }

    #[test]
    fn settings_toml_round_trip_preserves_persistence_enabled() {
        let settings = Settings {
            persistence_enabled: false,
        };

        let serialized = toml::to_string(&settings).expect("serialize");
        let round_tripped: Settings = toml::from_str(&serialized).expect("deserialize");

        assert_eq!(round_tripped, settings);
    }

    // Spec "Disable persistence": the env override always wins, regardless
    // of the persisted setting — a forced escape hatch (e.g. CI, debugging
    // a corrupt project file) that doesn't require editing settings.toml.
    #[test]
    fn env_override_disables_persistence_even_when_setting_is_enabled() {
        let settings = Settings {
            persistence_enabled: true,
        };

        assert!(!persistence_enabled(&settings, Some("1")));
    }

    #[test]
    fn env_override_absent_falls_back_to_the_persisted_setting() {
        let enabled = Settings {
            persistence_enabled: true,
        };
        let disabled = Settings {
            persistence_enabled: false,
        };

        assert!(persistence_enabled(&enabled, None));
        assert!(!persistence_enabled(&disabled, None));
    }

    #[test]
    fn settings_store_load_returns_default_when_file_absent() {
        let store = SettingsStore {
            root: unique_temp_dir("load-absent"),
        };

        assert_eq!(store.load(), Settings::default());
    }

    #[test]
    fn settings_store_save_then_load_round_trips() {
        let dir = unique_temp_dir("save-load");
        let store = SettingsStore { root: dir.clone() };
        let settings = Settings {
            persistence_enabled: false,
        };

        store.save(&settings).expect("save should succeed");

        assert_eq!(store.load(), settings);

        let _ = std::fs::remove_dir_all(&dir);
    }

    // Spec "Missing or Corrupt File Handling" applies equally to settings:
    // a corrupt settings.toml must never crash startup — it degrades to
    // the default settings.
    #[test]
    fn settings_store_load_returns_default_for_corrupt_file() {
        let dir = unique_temp_dir("load-corrupt");
        std::fs::create_dir_all(&dir).expect("create dir");
        let store = SettingsStore { root: dir.clone() };
        std::fs::write(store.path(), b"not = [[[ valid toml").expect("write corrupt file");

        assert_eq!(store.load(), Settings::default());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
