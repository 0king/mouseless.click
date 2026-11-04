//! Built-in and user presets (SPEC-settings).
//!
//! A preset carries a complete `behavior:` section plus a display name. The
//! file shape is intentionally the same as the config's behavior section:
//!
//! ```yaml
//! name: Quick-Select + Free-Mode
//! behavior:
//!   grid_action_level: 2
//!   default_action: move
//!   free_mode_after_move: true
//! ```

use crate::schema::{BehaviorConfig, Config, DefaultAction, GridActionLevel};
use crate::validate::validate;
use crate::ConfigError;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Built-in Standard preset id/name.
pub const STANDARD: &str = "standard";
/// Built-in quick-select + free-mode preset id/name.
pub const QUICK_SELECT_FREE_MODE: &str = "quick-select-free-mode";

/// One behavior preset.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Preset {
    /// Human-readable name shown in the GUI.
    pub name: String,
    /// Behavior section applied by this preset.
    #[serde(default)]
    pub behavior: BehaviorConfig,
}

impl Preset {
    /// Built-in Standard: subgrid / click / free-mode-after-move off.
    pub fn standard() -> Self {
        Self {
            name: "Standard".into(),
            behavior: BehaviorConfig {
                grid_action_level: GridActionLevel::Subgrid,
                default_action: DefaultAction::Click,
                free_mode_after_move: false,
                ..BehaviorConfig::default()
            },
        }
    }

    /// Built-in Quick-Select + Free-Mode: level 2 / move / free mode after move.
    pub fn quick_select_free_mode() -> Self {
        Self {
            name: "Quick-Select + Free-Mode".into(),
            behavior: BehaviorConfig {
                grid_action_level: GridActionLevel::Two,
                default_action: DefaultAction::Move,
                free_mode_after_move: true,
                ..BehaviorConfig::default()
            },
        }
    }

    /// Filesystem slug derived from the display name.
    pub fn slug(&self) -> String {
        slugify(&self.name)
    }

    /// Validate the preset by validating a full config with this behavior.
    pub fn validate(&self) -> Result<(), PresetError> {
        if self.name.trim().is_empty() {
            return Err(PresetError::InvalidName("preset name is empty".into()));
        }
        if self.name.chars().count() > 80 {
            return Err(PresetError::InvalidName(
                "preset name is longer than 80 characters".into(),
            ));
        }
        if self.slug().is_empty() {
            return Err(PresetError::InvalidName(
                "preset name needs at least one ASCII letter or digit".into(),
            ));
        }
        let config = Config {
            behavior: self.behavior.clone(),
            ..Config::default()
        };
        validate(&config).map_err(PresetError::Invalid)
    }

    /// File contents (YAML, stable field order).
    pub fn to_yaml_string(&self) -> Result<String, PresetError> {
        self.validate()?;
        serde_yaml_ng::to_string(self).map_err(|e| PresetError::Parse(e.to_string()))
    }

    /// Parse and validate one preset file.
    pub fn from_yaml_str(text: &str) -> Result<Self, PresetError> {
        let preset: Self =
            serde_yaml_ng::from_str(text).map_err(|e| PresetError::Parse(e.to_string()))?;
        preset.validate()?;
        Ok(preset)
    }
}

/// Preset errors (all GUI-presentable strings).
#[derive(Debug, thiserror::Error)]
pub enum PresetError {
    /// Filesystem failure with the path.
    #[error("preset I/O at {path}: {message}")]
    Io {
        /// Path involved.
        path: String,
        /// OS error text.
        message: String,
    },
    /// YAML parse/emit failure.
    #[error("preset parse: {0}")]
    Parse(String),
    /// Name cannot be turned into a file slug.
    #[error("invalid preset name: {0}")]
    InvalidName(String),
    /// Behavior values violate the schema.
    #[error(transparent)]
    Invalid(#[from] crate::validate::ValidationError),
}

impl From<PresetError> for ConfigError {
    fn from(value: PresetError) -> Self {
        match value {
            PresetError::InvalidName(message) => ConfigError::Parse {
                path: "<preset>".into(),
                message,
            },
            PresetError::Invalid(issues) => ConfigError::Invalid(issues),
            PresetError::Parse(message) => ConfigError::Parse {
                path: "<preset>".into(),
                message,
            },
            PresetError::Io { path, message } => ConfigError::Io { path, message },
        }
    }
}

/// Both built-in presets in display order.
pub fn builtins() -> Vec<Preset> {
    vec![Preset::standard(), Preset::quick_select_free_mode()]
}

/// Look up a built-in by slug (`standard`, `quick-select-free-mode`).
pub fn builtin_preset(id: &str) -> Option<Preset> {
    builtins()
        .into_iter()
        .find(|preset| preset.slug() == id.to_ascii_lowercase())
}

/// Return `base` with the preset's behavior applied; every other section is
/// preserved.
pub fn merge_preset(base: &Config, preset: &Preset) -> Config {
    let mut config = base.clone();
    apply_preset(&mut config, preset);
    config
}

/// Apply a preset in place (replaces only `behavior`).
pub fn apply_preset(config: &mut Config, preset: &Preset) {
    config.behavior = preset.behavior.clone();
}

/// Load every `*.yaml` / `*.yml` preset from `dir` (sorted by name).
///
/// A missing directory is an empty preset list, not an error: first run has no
/// user presets.
pub fn load_user_presets(dir: &Path) -> Result<Vec<Preset>, PresetError> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => {
            return Err(PresetError::Io {
                path: dir.display().to_string(),
                message: e.to_string(),
            })
        }
    };
    let mut presets = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| PresetError::Io {
            path: dir.display().to_string(),
            message: e.to_string(),
        })?;
        let path = entry.path();
        let is_yaml = path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("yaml") || ext.eq_ignore_ascii_case("yml"));
        if !is_yaml {
            continue;
        }
        let text = std::fs::read_to_string(&path).map_err(|e| PresetError::Io {
            path: path.display().to_string(),
            message: e.to_string(),
        })?;
        let preset = Preset::from_yaml_str(&text).map_err(|e| match e {
            PresetError::Io { .. } | PresetError::InvalidName(_) => e,
            other => PresetError::Io {
                path: path.display().to_string(),
                message: other.to_string(),
            },
        })?;
        presets.push(preset);
    }
    presets.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(presets)
}

/// Save one user preset under `dir/<slug>.yaml`, atomically.
pub fn save_preset(dir: &Path, preset: &Preset) -> Result<PathBuf, PresetError> {
    let yaml = preset.to_yaml_string()?;
    let path = dir.join(format!("{}.yaml", preset.slug()));
    crate::util::write_atomic(&path, yaml.as_bytes()).map_err(|message| PresetError::Io {
        path: path.display().to_string(),
        message,
    })?;
    Ok(path)
}

/// Import a preset file from anywhere (read + validate only).
pub fn import_preset(path: &Path) -> Result<Preset, PresetError> {
    let text = std::fs::read_to_string(path).map_err(|e| PresetError::Io {
        path: path.display().to_string(),
        message: e.to_string(),
    })?;
    Preset::from_yaml_str(&text)
}

/// Export a preset to an arbitrary path (atomic).
pub fn export_preset(path: &Path, preset: &Preset) -> Result<(), PresetError> {
    let yaml = preset.to_yaml_string()?;
    crate::util::write_atomic(path, yaml.as_bytes()).map_err(|message| PresetError::Io {
        path: path.display().to_string(),
        message,
    })
}

/// Lowercase ASCII slug: letters/digits kept, runs of other characters become
/// one hyphen, leading/trailing hyphens removed.
pub fn slugify(name: &str) -> String {
    let mut slug = String::new();
    let mut pending_dash = false;
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            if pending_dash && !slug.is_empty() {
                slug.push('-');
            }
            pending_dash = false;
            slug.push(c.to_ascii_lowercase());
        } else {
            pending_dash = true;
        }
    }
    slug
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "mouseless-presets-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn builtins_pin_the_documented_values() {
        let standard = Preset::standard();
        assert_eq!(
            standard.behavior.grid_action_level,
            GridActionLevel::Subgrid
        );
        assert_eq!(standard.behavior.default_action, DefaultAction::Click);
        assert!(!standard.behavior.free_mode_after_move);

        let quick = Preset::quick_select_free_mode();
        assert_eq!(quick.behavior.grid_action_level, GridActionLevel::Two);
        assert_eq!(quick.behavior.default_action, DefaultAction::Move);
        assert!(quick.behavior.free_mode_after_move);
        assert_eq!(quick.slug(), QUICK_SELECT_FREE_MODE);
        assert_eq!(standard.slug(), STANDARD);
        assert!(builtin_preset("STANDARD").is_some());
    }

    #[test]
    fn applying_a_preset_preserves_other_sections() {
        let mut base = Config::default();
        base.grid.columns = "asdf".into();
        base.appearance.grid_color = "#112233".into();
        let merged = merge_preset(&base, &Preset::quick_select_free_mode());
        assert_eq!(merged.grid.columns, "asdf");
        assert_eq!(merged.appearance.grid_color, "#112233");
        assert_eq!(merged.behavior.default_action, DefaultAction::Move);
    }

    #[test]
    fn preset_yaml_round_trips_and_rejects_unknown_fields() {
        let preset = Preset::quick_select_free_mode();
        let yaml = preset.to_yaml_string().unwrap();
        let parsed = Preset::from_yaml_str(&yaml).unwrap();
        assert_eq!(parsed, preset);
        assert!(Preset::from_yaml_str("name: x\nbehavior: {}\nextra: 1\n").is_err());
        assert!(Preset::from_yaml_str("name: ''\nbehavior: {}\n").is_err());
        assert!(Preset::from_yaml_str("name: x\nbehavior:\n  tap_threshold_ms: 1\n").is_err());
    }

    #[test]
    fn user_presets_load_sorted_and_skip_non_yaml() {
        let dir = temp_dir("load");
        save_preset(&dir, &Preset::standard()).unwrap();
        save_preset(&dir, &Preset::quick_select_free_mode()).unwrap();
        std::fs::write(dir.join("notes.txt"), "ignored").unwrap();
        let loaded = load_user_presets(&dir).unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].name, "Quick-Select + Free-Mode");
        assert_eq!(loaded[1].name, "Standard");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn missing_preset_dir_is_empty_not_an_error() {
        let dir = temp_dir("missing").join("nope");
        assert!(load_user_presets(&dir).unwrap().is_empty());
        std::fs::remove_dir_all(dir.parent().unwrap()).unwrap();
    }

    #[test]
    fn import_export_round_trip() {
        let dir = temp_dir("roundtrip");
        let path = dir.join("custom.yaml");
        let mut preset = Preset::standard();
        preset.name = "  My   Preset!! ".into();
        export_preset(&path, &preset).unwrap();
        let imported = import_preset(&path).unwrap();
        assert_eq!(imported, preset);
        assert_eq!(imported.slug(), "my-preset");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn slugify_is_stable() {
        assert_eq!(
            slugify("Quick-Select + Free-Mode"),
            "quick-select-free-mode"
        );
        assert_eq!(slugify("  Standard  "), "standard");
        assert_eq!(slugify("!!!"), "");
        assert_eq!(slugify("A/B_C"), "a-b-c");
    }
}
