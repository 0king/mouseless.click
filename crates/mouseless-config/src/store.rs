//! Atomic config store with last-good rollback (SPEC-settings lifecycle #9).
//!
//! ```text
//! candidate -> validate -> temp file -> check gate -> backup current
//!           -> atomic rename -> success
//! ANY failure: temp removed, live file untouched, last-good preserved
//! ```
//!
//! The store is path-based on purpose: the daemon uses its `StateDirectory`
//! copy for the authoritative config, while user presets live under the XDG
//! config directory. It never invents paths or escalates privileges.

use crate::schema::{Config, CONFIG_VERSION};
use crate::util::{copy_atomic, write_atomic};
use crate::validate::validate;
use crate::ConfigError;
use std::fs;
use std::path::{Path, PathBuf};

/// Store errors are the shared [`ConfigError`] plus [`ConfigError::NotFound`].
pub type StoreError = ConfigError;

/// Versioned config file with an adjacent `*.last-good.yaml` backup.
#[derive(Debug, Clone)]
pub struct ConfigStore {
    config_path: PathBuf,
    last_good_path: PathBuf,
    presets_dir: PathBuf,
}

impl ConfigStore {
    /// Store for an explicit config path. The last-good copy and presets
    /// directory live beside it.
    pub fn new(config_path: impl Into<PathBuf>) -> Self {
        let config_path = config_path.into();
        let parent = config_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        let stem = config_path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("config");
        let last_good_path = parent.join(format!("{stem}.last-good.yaml"));
        let presets_dir = parent.join("presets");
        Self {
            config_path,
            last_good_path,
            presets_dir,
        }
    }

    /// `${XDG_CONFIG_HOME:-~/.config}/mouseless/config.yaml`.
    pub fn user_default() -> Result<Self, StoreError> {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|path| !path.as_os_str().is_empty())
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
            .ok_or_else(|| ConfigError::Io {
                path: "XDG_CONFIG_HOME".into(),
                message: "neither XDG_CONFIG_HOME nor HOME is set".into(),
            })?;
        Ok(Self::new(base.join("mouseless").join("config.yaml")))
    }

    /// Daemon-owned copy under a state directory (`/var/lib/mouseless`).
    pub fn for_daemon(state_dir: impl AsRef<Path>) -> Self {
        Self::new(state_dir.as_ref().join("config.yaml"))
    }

    /// Live config path.
    pub fn config_path(&self) -> &Path {
        &self.config_path
    }

    /// Last-good backup path.
    pub fn last_good_path(&self) -> &Path {
        &self.last_good_path
    }

    /// User preset directory (`<config dir>/presets`).
    pub fn presets_dir(&self) -> &Path {
        &self.presets_dir
    }

    /// Read + parse + validate the live config.
    ///
    /// Missing file → [`ConfigError::NotFound`]. Parse/validation errors are
    /// returned as-is; callers that must stay available should use
    /// [`Self::effective`].
    pub fn load(&self) -> Result<Config, StoreError> {
        if !self.config_path.exists() {
            return Err(ConfigError::NotFound {
                path: self.config_path.display().to_string(),
            });
        }
        load_file(&self.config_path)
    }

    /// Load the live config, falling back to last-good then defaults.
    ///
    /// Returns the error alongside the fallback so the caller can surface it
    /// (daemon logs it; the GUI shows a banner).
    pub fn effective(&self) -> (Config, Option<ConfigError>) {
        match self.load() {
            Ok(config) => (config, None),
            Err(primary) => match load_file(&self.last_good_path) {
                Ok(config) => (config, Some(primary)),
                Err(_) => (Config::default(), Some(primary)),
            },
        }
    }

    /// Validate and atomically save a candidate without a check gate.
    pub fn save(&self, candidate: &Config) -> Result<(), StoreError> {
        self.apply(candidate, |_| Ok(()))
    }

    /// Validate, run `check` against a staged candidate file, then commit.
    ///
    /// `check` receives the staged path; it must return `Err(message)` to
    /// veto. On veto the live config, last-good, and last-good semantics are
    /// untouched.
    pub fn apply<F>(&self, candidate: &Config, check: F) -> Result<(), StoreError>
    where
        F: FnOnce(&Path) -> Result<(), String>,
    {
        validate(candidate)?;
        let yaml = candidate.to_yaml_string()?;
        let staged = self.staged_path();
        fs::create_dir_all(self.config_path.parent().unwrap_or_else(|| Path::new("."))).map_err(
            |e| ConfigError::Io {
                path: self.config_path.display().to_string(),
                message: e.to_string(),
            },
        )?;
        fs::write(&staged, yaml.as_bytes()).map_err(|e| ConfigError::Io {
            path: staged.display().to_string(),
            message: e.to_string(),
        })?;

        if let Err(message) = check(&staged) {
            let _ = fs::remove_file(&staged);
            return Err(ConfigError::CheckFailed { message });
        }

        // Keep the previous validated file as last-good. First save seeds
        // last-good with the candidate, so rollback always has a valid file.
        let had_previous = self.config_path.exists();
        if had_previous {
            if let Err(message) = copy_atomic(&self.config_path, &self.last_good_path) {
                let _ = fs::remove_file(&staged);
                return Err(ConfigError::Io {
                    path: self.last_good_path.display().to_string(),
                    message,
                });
            }
        }

        if let Err(e) = fs::rename(&staged, &self.config_path) {
            let _ = fs::remove_file(&staged);
            return Err(ConfigError::Io {
                path: self.config_path.display().to_string(),
                message: e.to_string(),
            });
        }

        if !had_previous {
            let _ = copy_atomic(&self.config_path, &self.last_good_path);
        }
        Ok(())
    }

    /// Restore the last-good file over the live config.
    pub fn rollback(&self) -> Result<Config, StoreError> {
        let config = load_file(&self.last_good_path)?;
        let yaml = config.to_yaml_string()?;
        write_atomic(&self.config_path, yaml.as_bytes()).map_err(|message| ConfigError::Io {
            path: self.config_path.display().to_string(),
            message,
        })?;
        Ok(config)
    }

    /// Replace the live config with schema defaults.
    pub fn restore_defaults(&self) -> Result<(), StoreError> {
        self.save(&Config::default())
    }

    fn staged_path(&self) -> PathBuf {
        let name = self
            .config_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("config.yaml");
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        self.config_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(format!(".{name}.candidate-{}-{nanos}", std::process::id()))
    }
}

fn load_file(path: &Path) -> Result<Config, ConfigError> {
    if !path.exists() {
        return Err(ConfigError::NotFound {
            path: path.display().to_string(),
        });
    }
    let text = fs::read_to_string(path).map_err(|e| ConfigError::Io {
        path: path.display().to_string(),
        message: e.to_string(),
    })?;
    let config: Config = serde_yaml_ng::from_str(&text).map_err(|e| ConfigError::Parse {
        path: path.display().to_string(),
        message: e.to_string(),
    })?;
    if config.config_version != CONFIG_VERSION {
        return Err(ConfigError::UnsupportedVersion {
            got: config.config_version,
            expected: CONFIG_VERSION,
        });
    }
    validate(&config)?;
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::DefaultAction;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "mouseless-config-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn store(tag: &str) -> (ConfigStore, PathBuf) {
        let dir = temp_dir(tag);
        let store = ConfigStore::new(dir.join("mouseless").join("config.yaml"));
        (store, dir)
    }

    #[test]
    fn first_run_is_not_found_and_effective_uses_defaults() {
        let (store, dir) = store("first-run");
        assert!(matches!(store.load(), Err(ConfigError::NotFound { .. })));
        let (config, error) = store.effective();
        assert_eq!(config, Config::default());
        assert!(matches!(error, Some(ConfigError::NotFound { .. })));
        assert!(!store.config_path().exists());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn save_load_round_trip_creates_last_good() {
        let (store, dir) = store("roundtrip");
        let mut config = Config::default();
        config.behavior.default_action = DefaultAction::Move;
        store.save(&config).unwrap();
        assert_eq!(store.load().unwrap(), config);
        assert_eq!(load_file(store.last_good_path()).unwrap(), config);
        assert_eq!(
            store.presets_dir(),
            store.config_path().parent().unwrap().join("presets")
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn check_veto_leaves_live_file_untouched() {
        let (store, dir) = store("veto");
        let mut good = Config::default();
        good.behavior.multi_click_threshold_ms = 300;
        store.save(&good).unwrap();

        let mut candidate = good.clone();
        candidate.behavior.multi_click_threshold_ms = 900;
        let error = store
            .apply(&candidate, |staged| {
                assert!(staged.exists());
                Err("kanata --check failed: bad layer".into())
            })
            .unwrap_err();
        assert!(matches!(error, ConfigError::CheckFailed { .. }));
        assert_eq!(store.load().unwrap(), good);
        // No staged candidate files left behind.
        let leftovers: Vec<_> = fs::read_dir(store.config_path().parent().unwrap())
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().contains(".candidate-"))
            .collect();
        assert!(leftovers.is_empty());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn invalid_candidate_is_rejected_before_any_write() {
        let (store, dir) = store("invalid");
        let good = Config::default();
        store.save(&good).unwrap();
        let before = fs::read(store.config_path()).unwrap();

        let mut candidate = good.clone();
        candidate.grid.columns.clear();
        let error = store.save(&candidate).unwrap_err();
        assert!(matches!(error, ConfigError::Invalid(_)));
        assert_eq!(fs::read(store.config_path()).unwrap(), before);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn rollback_restores_the_previous_validated_file() {
        let (store, dir) = store("rollback");
        let mut first = Config::default();
        first.behavior.multi_click_threshold_ms = 111;
        store.save(&first).unwrap();

        let mut second = first.clone();
        second.behavior.multi_click_threshold_ms = 222;
        store.save(&second).unwrap();
        assert_eq!(store.load().unwrap(), second);

        assert_eq!(store.rollback().unwrap(), first);
        assert_eq!(store.load().unwrap(), first);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn corrupt_config_falls_back_to_last_good_with_error() {
        let (store, dir) = store("corrupt");
        let mut good = Config::default();
        good.behavior.multi_click_threshold_ms = 321;
        store.save(&good).unwrap();
        // Overwrite the live file with an unknown-key document.
        fs::write(store.config_path(), "config_version: 1\nbogus: true\n").unwrap();

        let (config, error) = store.effective();
        assert_eq!(config, good);
        assert!(matches!(error, Some(ConfigError::Parse { .. })));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn unsupported_version_is_refused() {
        let (store, dir) = store("version");
        fs::create_dir_all(store.config_path().parent().unwrap()).unwrap();
        fs::write(store.config_path(), "config_version: 2\n").unwrap();
        assert!(matches!(
            store.load(),
            Err(ConfigError::UnsupportedVersion {
                got: 2,
                expected: 1
            })
        ));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn restore_defaults_writes_a_valid_file() {
        let (store, dir) = store("defaults");
        let mut config = Config::default();
        config.appearance.grid_opacity = 0.9;
        store.save(&config).unwrap();
        store.restore_defaults().unwrap();
        assert_eq!(store.load().unwrap(), Config::default());
        fs::remove_dir_all(&dir).unwrap();
    }
}
