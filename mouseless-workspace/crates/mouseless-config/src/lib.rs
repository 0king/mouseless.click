//! `mouseless-config`: GTK-free configuration engine (Phase 3, matrix #9).
//!
//! Owns the frozen `config.yaml` schema v1 ([`Config`]), semantic validation,
//! built-in/user presets, atomic validate-then-apply with last-good rollback,
//! and the kanata include renderer/check gate. No GTK types are allowed here so
//! the whole crate is testable headless (`cargo test -p mouseless-config`).
//!
//! Trust boundary: this crate never writes anything outside the paths a caller
//! passes in. The GUI supplies the user config path; the daemon supplies its
//! `StateDirectory` copy.

pub mod appconfig;
pub mod kanata;
pub mod presets;
pub mod schema;
pub mod store;
mod util;
pub mod validate;

pub use appconfig::{
    load_effective, AppConfig, AppConfigError, AppearanceDefaults, ConfigSource, DaemonSettings,
    DeepLinkSettings, DevicesSettings, IpcSettings, KanataSettings, LayerSettings, LoadedAppConfig,
    PathsConfig, UiSettings, APP_CONFIG_ENV, APP_CONFIG_PATH,
};
pub use kanata::{render_include, CheckError, KanataCheck, ProcessRunner, Runner};
pub use presets::{
    apply_preset, builtin_preset, builtins, export_preset, import_preset, load_user_presets,
    merge_preset, save_preset, Preset, PresetError, QUICK_SELECT_FREE_MODE, STANDARD,
};
pub use schema::{
    AppearanceConfig, BehaviorConfig, Config, CursorCorner, DefaultAction, Easing, Engine,
    FreeModeConfig, GridActionLevel, GridConfig, InitialActionLocation, KeybindingConfig,
    MonitorAssignment, SubgridConfig, WheelConfig, COLUMN_KEY_POOL, CONFIG_VERSION, ROW_KEY_POOL,
    SUBGRID_KEY_POOL,
};
pub use store::{ConfigStore, StoreError};
pub use validate::{parse_hex_color, validate, Issue, ValidationError};

/// Errors shared by the config engine entry points.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// Input/output failure with the path that failed.
    #[error("config I/O at {path}: {message}")]
    Io {
        /// Path involved.
        path: String,
        /// OS error text.
        message: String,
    },
    /// YAML parsing failure with the path that failed.
    #[error("config parse at {path}: {message}")]
    Parse {
        /// Path involved.
        path: String,
        /// Parser error text.
        message: String,
    },
    /// The document declares a schema version this build cannot apply.
    #[error("unsupported config_version {got}; this build supports {expected}")]
    UnsupportedVersion {
        /// Version found in the document.
        got: u32,
        /// Version this build implements.
        expected: u32,
    },
    /// No config file exists yet (first run).
    #[error("config not found at {path}")]
    NotFound {
        /// Missing path.
        path: String,
    },
    /// A caller-supplied validation gate (kanata `--check`) rejected the file.
    #[error("config rejected by the check gate: {message}")]
    CheckFailed {
        /// Checker output.
        message: String,
    },
    /// One or more semantic validation issues (field-tagged for the GUI).
    #[error(transparent)]
    Invalid(#[from] ValidationError),
}
