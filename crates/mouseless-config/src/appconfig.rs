//! Application-level TOML configuration (`mouseless.toml`).
//!
//! This file owns values that used to be literals in the daemon, CLI, settings
//! app, and platform files: paths, the kanata endpoint, device names, timeouts,
//! the deep-link scheme, the UI application id, and appearance fallbacks. It is
//! deliberately separate from [`crate::Config`] (`config.yaml`), which remains
//! the user-editable settings document owned by the daemon.
//!
//! Installation path and precedence:
//!
//! ```text
//! --config PATH  >  MOUSELESS_CONFIG  >  /etc/mouseless/mouseless.toml  >  built-in defaults
//! ```
//!
//! Every section and key is optional; a missing file degrades to the built-in
//! defaults. A file explicitly requested through `--config`/`MOUSELESS_CONFIG`
//! is required and invalid content is reported instead of silently ignored.

use crate::validate::parse_hex_color;
use serde::{Deserialize, Serialize};
use std::net::IpAddr;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

/// Installed system copy read by every Mouseless binary.
pub const APP_CONFIG_PATH: &str = "/etc/mouseless/mouseless.toml";
/// Development override for [`APP_CONFIG_PATH`].
pub const APP_CONFIG_ENV: &str = "MOUSELESS_CONFIG";

/// Default grid fill colour (`config.yaml` fallback).
pub const DEFAULT_GRID_COLOR: &str = "#d4de04";
/// Default grid fill opacity (0.2 = 80% transparent).
pub const DEFAULT_GRID_OPACITY: f64 = 0.2;
/// Default cell label colour.
pub const DEFAULT_LABEL_COLOR: &str = "#ebe9e8";
/// Default highlight colour.
pub const DEFAULT_HIGHLIGHT_COLOR: &str = "#ffb22c";

/// Error raised while locating, reading, parsing, or validating `mouseless.toml`.
#[derive(Debug, Clone, thiserror::Error)]
pub enum AppConfigError {
    /// The file does not exist.
    #[error("app config not found at {path}")]
    NotFound {
        /// Missing path.
        path: String,
    },
    /// The file exists but could not be read.
    #[error("app config I/O at {path}: {message}")]
    Io {
        /// Path involved.
        path: String,
        /// OS error text.
        message: String,
    },
    /// TOML parsing failed.
    #[error("app config parse at {path}: {message}")]
    Parse {
        /// Path involved.
        path: String,
        /// Parser error text.
        message: String,
    },
    /// Parsed values are semantically invalid.
    #[error("app config invalid: {0}")]
    Invalid(String),
}

/// Root of `mouseless.toml`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AppConfig {
    /// Filesystem locations.
    pub paths: PathsConfig,
    /// Kanata TCP endpoint and include names.
    pub kanata: KanataSettings,
    /// Daemon loop timeouts and closed-loop movement tuning.
    pub daemon: DaemonSettings,
    /// Virtual device identity.
    pub devices: DevicesSettings,
    /// Fixed kanata layer names not covered by `config.yaml`.
    pub layers: LayerSettings,
    /// `mouseless://` deep-link routing.
    pub deep_link: DeepLinkSettings,
    /// Settings-window identity.
    pub ui: UiSettings,
    /// Appearance used when `config.yaml` has no appearance section.
    pub appearance: AppearanceDefaults,
    /// IPC client timeout.
    pub ipc: IpcSettings,
}

impl AppConfig {
    /// Parse and validate a TOML document.
    pub fn from_toml_str(text: &str) -> Result<Self, AppConfigError> {
        Self::parse(text, Path::new("<memory>"))
    }

    /// Read, parse, and validate one `mouseless.toml`.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, AppConfigError> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path).map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => AppConfigError::NotFound {
                path: path.display().to_string(),
            },
            _ => AppConfigError::Io {
                path: path.display().to_string(),
                message: error.to_string(),
            },
        })?;
        Self::parse(&text, path)
    }

    /// The daemon's last-applied config path for a resolved state directory.
    pub fn state_config_path(&self, state_dir: &Path) -> PathBuf {
        state_dir.join(&self.paths.config_file)
    }

    fn parse(text: &str, path: &Path) -> Result<Self, AppConfigError> {
        let config: Self = toml::from_str(text).map_err(|error| AppConfigError::Parse {
            path: path.display().to_string(),
            message: error.to_string(),
        })?;
        config.validate()?;
        Ok(config)
    }

    /// Check every value this build cannot use safely.
    pub fn validate(&self) -> Result<(), AppConfigError> {
        let mut issues: Vec<String> = Vec::new();

        if self.paths.state_dir.is_empty() {
            issues.push("paths.state_dir must not be empty".into());
        }
        if self.paths.socket.is_empty() {
            issues.push("paths.socket must not be empty".into());
        }
        if self.paths.kanata_main_config.is_empty() {
            issues.push("paths.kanata_main_config must not be empty".into());
        }
        if self.paths.uinput.is_empty() {
            issues.push("paths.uinput must not be empty".into());
        }
        if self.paths.input_devices.is_empty() {
            issues.push("paths.input_devices must not be empty".into());
        }
        let config_file = Path::new(&self.paths.config_file);
        if self.paths.config_file.is_empty()
            || config_file.is_absolute()
            || !matches!(config_file.components().next(), Some(Component::Normal(_)))
            || config_file
                .components()
                .any(|component| matches!(component, Component::ParentDir))
        {
            issues.push("paths.config_file must be a relative file inside state_dir".into());
        }

        if !valid_endpoint(&self.kanata.address) {
            issues.push(format!(
                "kanata.address must be a loopback HOST:PORT with a non-zero port, got {:?}",
                self.kanata.address
            ));
        }
        if self.kanata.connect_timeout_ms == 0 {
            issues.push("kanata.connect_timeout_ms must be greater than zero".into());
        }
        if self.kanata.query_timeout_ms == 0 {
            issues.push("kanata.query_timeout_ms must be greater than zero".into());
        }
        if self.kanata.reconnect_backoff_ms == 0 {
            issues.push("kanata.reconnect_backoff_ms must be greater than zero".into());
        }
        if !valid_include_name(&self.kanata.include_name) {
            issues.push("kanata.include_name must be a plain file name (no / or \")".into());
        }

        let daemon = &self.daemon;
        if daemon.idle_read_timeout_ms == 0 {
            issues.push("daemon.idle_read_timeout_ms must be greater than zero".into());
        }
        if daemon.session_read_timeout_ms == 0 {
            issues.push("daemon.session_read_timeout_ms must be greater than zero".into());
        }
        if daemon.grab_retry_attempts == 0 {
            issues.push("daemon.grab_retry_attempts must be greater than zero".into());
        }
        if daemon.grab_retry_delay_ms == 0 {
            issues.push("daemon.grab_retry_delay_ms must be greater than zero".into());
        }
        if daemon.cursor_query_timeout_ms == 0 {
            issues.push("daemon.cursor_query_timeout_ms must be greater than zero".into());
        }
        if daemon.tick_ms == 0 {
            issues.push("daemon.tick_ms must be greater than zero".into());
        }
        if daemon.move_max_step_px <= 0
            || daemon.move_min_step_px <= 0
            || daemon.move_tolerance_px < 0
            || daemon.move_damping_divisor < 1
            || daemon.move_max_iterations == 0
            || daemon.move_max_stalled_steps == 0
            || daemon.move_settle_timeout_ms == 0
            || daemon.move_settle_poll_ms == 0
        {
            issues.push(
                "daemon movement tuning needs positive steps/limits and a damping divisor >= 1"
                    .into(),
            );
        }

        if !valid_device_name(&self.devices.kanata_output_name) {
            issues.push("devices.kanata_output_name must use only letters, digits, - _ .".into());
        }
        if !valid_device_name(&self.devices.mouse_name) {
            issues.push("devices.mouse_name must use only letters, digits, - _ .".into());
        }
        if self.layers.base.is_empty() {
            issues.push("layers.base must not be empty".into());
        }

        if !valid_scheme(&self.deep_link.scheme) {
            issues.push(format!(
                "deep_link.scheme must start with a letter and use only letters, digits, +, -, ., got {:?}",
                self.deep_link.scheme
            ));
        }
        if self.deep_link.settings_binary.is_empty() {
            issues.push("deep_link.settings_binary must not be empty".into());
        }
        if self.ui.app_id.is_empty() {
            issues.push("ui.app_id must not be empty".into());
        }
        if self.ipc.client_timeout_ms == 0 {
            issues.push("ipc.client_timeout_ms must be greater than zero".into());
        }

        if parse_hex_color(&self.appearance.grid_color).is_none() {
            issues.push("appearance.grid_color must be #rrggbb".into());
        }
        if parse_hex_color(&self.appearance.label_color).is_none() {
            issues.push("appearance.label_color must be #rrggbb".into());
        }
        if parse_hex_color(&self.appearance.highlight_color).is_none() {
            issues.push("appearance.highlight_color must be #rrggbb".into());
        }
        if !self.appearance.grid_opacity.is_finite()
            || !(0.0..=1.0).contains(&self.appearance.grid_opacity)
        {
            issues.push("appearance.grid_opacity must be between 0.0 and 1.0".into());
        }

        if issues.is_empty() {
            Ok(())
        } else {
            Err(AppConfigError::Invalid(issues.join("; ")))
        }
    }
}

/// Filesystem locations used across the workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PathsConfig {
    /// Daemon state directory (`StateDirectory=mouseless`).
    pub state_dir: String,
    /// Last-applied settings document inside [`Self::state_dir`].
    pub config_file: String,
    /// Overlay Unix socket.
    pub socket: String,
    /// Kanata main config that includes [`KanataSettings::include_name`].
    pub kanata_main_config: String,
    /// uinput device node.
    pub uinput: String,
    /// Kernel input-device listing used by `--list-input-devices`.
    pub input_devices: String,
}

impl Default for PathsConfig {
    fn default() -> Self {
        Self {
            state_dir: "/var/lib/mouseless".into(),
            config_file: "config.yaml".into(),
            socket: "/run/mouseless/mouselessd.sock".into(),
            kanata_main_config: "/etc/kanata/unicfg.kbd".into(),
            uinput: "/dev/uinput".into(),
            input_devices: "/proc/bus/input/devices".into(),
        }
    }
}

/// Kanata TCP and include settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct KanataSettings {
    /// Localhost endpoint (`HOST:PORT`).
    pub address: String,
    /// Connect timeout for daemon/settings clients.
    pub connect_timeout_ms: u64,
    /// Query timeout for CLI layer operations.
    pub query_timeout_ms: u64,
    /// Delay before retrying an unavailable kanata TCP connection.
    pub reconnect_backoff_ms: u64,
    /// Include file name generated by the config engine.
    pub include_name: String,
}

impl Default for KanataSettings {
    fn default() -> Self {
        Self {
            address: "127.0.0.1:7070".into(),
            connect_timeout_ms: 2000,
            query_timeout_ms: 3000,
            reconnect_backoff_ms: 1000,
            include_name: "mouseless.kbd".into(),
        }
    }
}

impl KanataSettings {
    /// Split [`Self::address`] into host and port (validated on load).
    pub fn endpoint(&self) -> (String, u16) {
        let (host, port) = self
            .address
            .rsplit_once(':')
            .unwrap_or(("127.0.0.1", "7070"));
        (host.to_string(), port.parse().unwrap_or(7070))
    }

    /// Daemon/settings connect timeout.
    pub fn connect_timeout(&self) -> Duration {
        Duration::from_millis(self.connect_timeout_ms)
    }

    /// CLI layer-query timeout.
    pub fn query_timeout(&self) -> Duration {
        Duration::from_millis(self.query_timeout_ms)
    }

    /// Delay before retrying an unavailable kanata connection.
    pub fn reconnect_backoff(&self) -> Duration {
        Duration::from_millis(self.reconnect_backoff_ms)
    }
}

/// Daemon loop timeouts and movement tuning.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DaemonSettings {
    /// Idle TCP read timeout (also polls the overlay socket).
    pub idle_read_timeout_ms: u64,
    /// Session TCP read timeout (grabbed keys must be polled often).
    pub session_read_timeout_ms: u64,
    /// `EBUSY` grab retry attempts.
    pub grab_retry_attempts: u32,
    /// Delay between grab retries.
    pub grab_retry_delay_ms: u64,
    /// Cursor refresh budget before a click.
    pub cursor_query_timeout_ms: u64,
    /// Shift tap window for monitor switching.
    pub monitor_switch_tap_ms: u64,
    /// Fixed tick for the kanata precision layer.
    #[serde(alias = "free_mode_tick_ms")]
    pub tick_ms: u64,
    /// Largest single REL step.
    pub move_max_step_px: i32,
    /// Divisor applied to the remaining error (compositor acceleration).
    pub move_damping_divisor: i32,
    /// Smallest REL step for a remaining error above tolerance.
    pub move_min_step_px: i32,
    /// Acceptable final cursor error.
    pub move_tolerance_px: i32,
    /// Maximum closed-loop iterations.
    pub move_max_iterations: u32,
    /// Consecutive no-progress queries before a move is declared stalled.
    pub move_max_stalled_steps: u32,
    /// Maximum wait for a fresh cursor report after a step.
    pub move_settle_timeout_ms: u64,
    /// Poll interval while waiting for a fresh cursor report.
    pub move_settle_poll_ms: u64,
    /// Log overlay geometry/cursor reports (`MOUSELESS_DEBUG_LAYOUT` also works).
    pub debug_layout: bool,
    /// Log every closed-loop movement step (`MOUSELESS_DEBUG_MOVE` also works).
    pub debug_move: bool,
}

impl Default for DaemonSettings {
    fn default() -> Self {
        Self {
            idle_read_timeout_ms: 500,
            session_read_timeout_ms: 2,
            grab_retry_attempts: 40,
            grab_retry_delay_ms: 50,
            cursor_query_timeout_ms: 100,
            monitor_switch_tap_ms: 200,
            tick_ms: 8,
            move_max_step_px: 64,
            move_damping_divisor: 2,
            move_min_step_px: 2,
            move_tolerance_px: 2,
            move_max_iterations: 256,
            move_max_stalled_steps: 8,
            move_settle_timeout_ms: 20,
            move_settle_poll_ms: 2,
            debug_layout: false,
            debug_move: false,
        }
    }
}

impl DaemonSettings {
    /// Idle read timeout.
    pub fn idle_read_timeout(&self) -> Duration {
        Duration::from_millis(self.idle_read_timeout_ms)
    }

    /// Session read timeout.
    pub fn session_read_timeout(&self) -> Duration {
        Duration::from_millis(self.session_read_timeout_ms)
    }

    /// Grab retry delay.
    pub fn grab_retry_delay(&self) -> Duration {
        Duration::from_millis(self.grab_retry_delay_ms)
    }

    /// Cursor query timeout.
    pub fn cursor_query_timeout(&self) -> Duration {
        Duration::from_millis(self.cursor_query_timeout_ms)
    }

    /// Precision tick.
    pub fn tick(&self) -> Duration {
        Duration::from_millis(self.tick_ms)
    }

    /// Stale-reply settle timeout.
    pub fn move_settle_timeout(&self) -> Duration {
        Duration::from_millis(self.move_settle_timeout_ms)
    }

    /// Stale-reply poll interval.
    pub fn move_settle_poll(&self) -> Duration {
        Duration::from_millis(self.move_settle_poll_ms)
    }
}

/// Virtual device identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DevicesSettings {
    /// Kanata virtual output name (`linux-output-device-name`).
    pub kanata_output_name: String,
    /// uinput mouse name (must be in kanata `linux-dev-names-exclude`).
    pub mouse_name: String,
    /// uinput vendor id.
    pub vendor_id: u16,
}

impl Default for DevicesSettings {
    fn default() -> Self {
        Self {
            kanata_output_name: "kanata-mouseless".into(),
            mouse_name: "mouseless-mouse".into(),
            vendor_id: 0x736e,
        }
    }
}

/// Fixed kanata layer names not covered by `config.yaml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LayerSettings {
    /// Neutral base layer restored on exit.
    pub base: String,
}

impl Default for LayerSettings {
    fn default() -> Self {
        Self {
            base: "lyr-base".into(),
        }
    }
}

/// Deep-link routing settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DeepLinkSettings {
    /// URL scheme used by `mouseless://` links.
    pub scheme: String,
    /// Settings binary spawned by the CLI.
    pub settings_binary: String,
}

impl Default for DeepLinkSettings {
    fn default() -> Self {
        Self {
            scheme: "mouseless".into(),
            settings_binary: "mouseless-settings".into(),
        }
    }
}

/// Settings-window identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct UiSettings {
    /// GTK application id (also the single-instance key).
    pub app_id: String,
}

impl Default for UiSettings {
    fn default() -> Self {
        Self {
            app_id: "click.mouseless.Settings".into(),
        }
    }
}

/// Appearance defaults used when `config.yaml` has no `appearance` section.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AppearanceDefaults {
    /// Grid fill colour, `#rrggbb`.
    pub grid_color: String,
    /// Grid fill opacity, `0.0..=1.0`.
    pub grid_opacity: f64,
    /// Cell label colour, `#rrggbb`.
    pub label_color: String,
    /// Highlight colour, `#rrggbb`.
    pub highlight_color: String,
}

impl Default for AppearanceDefaults {
    fn default() -> Self {
        Self {
            grid_color: DEFAULT_GRID_COLOR.into(),
            grid_opacity: DEFAULT_GRID_OPACITY,
            label_color: DEFAULT_LABEL_COLOR.into(),
            highlight_color: DEFAULT_HIGHLIGHT_COLOR.into(),
        }
    }
}

/// IPC client timeout.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct IpcSettings {
    /// One-shot client request timeout.
    pub client_timeout_ms: u64,
}

impl Default for IpcSettings {
    fn default() -> Self {
        Self {
            client_timeout_ms: 1000,
        }
    }
}

impl IpcSettings {
    /// Client request timeout.
    pub fn client_timeout(&self) -> Duration {
        Duration::from_millis(self.client_timeout_ms)
    }
}

/// Where the effective config came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigSource {
    /// `--config PATH` (required).
    Explicit(PathBuf),
    /// `MOUSELESS_CONFIG` (required).
    Env(PathBuf),
    /// Installed default path (optional).
    Default(PathBuf),
}

impl ConfigSource {
    /// Path backing this source.
    pub fn path(&self) -> &Path {
        match self {
            Self::Explicit(path) | Self::Env(path) | Self::Default(path) => path,
        }
    }

    /// Whether a missing/unreadable file must be reported to the caller.
    pub fn is_required(&self) -> bool {
        matches!(self, Self::Explicit(_) | Self::Env(_))
    }

    /// Human-readable label for logs and status output.
    pub fn label(&self) -> String {
        match self {
            Self::Explicit(path) => format!("--config {}", path.display()),
            Self::Env(path) => format!("{APP_CONFIG_ENV} {}", path.display()),
            Self::Default(path) => path.display().to_string(),
        }
    }
}

/// Effective config plus its provenance and any load error.
#[derive(Debug, Clone)]
pub struct LoadedAppConfig {
    /// Parsed config, or built-in defaults on failure.
    pub config: AppConfig,
    /// Where the config was looked up.
    pub source: ConfigSource,
    /// Load error, if the file was required or existed but was invalid.
    pub error: Option<AppConfigError>,
}

/// Resolve and load the effective application config.
///
/// A missing installed copy silently falls back to defaults. A missing or
/// invalid explicitly requested file keeps the error in
/// [`LoadedAppConfig::error`] so the binary can fail loudly.
pub fn load_effective(explicit: Option<&Path>) -> LoadedAppConfig {
    let source = if let Some(path) = explicit {
        ConfigSource::Explicit(path.to_path_buf())
    } else if let Some(value) = std::env::var_os(APP_CONFIG_ENV).filter(|value| !value.is_empty()) {
        ConfigSource::Env(PathBuf::from(value))
    } else {
        ConfigSource::Default(PathBuf::from(APP_CONFIG_PATH))
    };

    match AppConfig::load(source.path()) {
        Ok(config) => LoadedAppConfig {
            config,
            source,
            error: None,
        },
        Err(error) => {
            let missing_optional =
                matches!(error, AppConfigError::NotFound { .. }) && !source.is_required();
            LoadedAppConfig {
                config: AppConfig::default(),
                source,
                error: if missing_optional { None } else { Some(error) },
            }
        }
    }
}

fn valid_endpoint(address: &str) -> bool {
    let Some((host, port)) = address.rsplit_once(':') else {
        return false;
    };
    !host.is_empty()
        && !port.is_empty()
        && port != "0"
        && port.parse::<u16>().is_ok()
        && is_loopback_host(host)
}

fn is_loopback_host(host: &str) -> bool {
    let bare = host
        .strip_prefix('[')
        .and_then(|host| host.strip_suffix(']'))
        .unwrap_or(host);
    bare == "localhost"
        || bare
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

fn valid_include_name(name: &str) -> bool {
    !name.is_empty() && !name.contains('/') && !name.contains('"')
}

fn valid_device_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

fn valid_scheme(scheme: &str) -> bool {
    let mut chars = scheme.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checked_in_toml_matches_built_in_defaults() {
        let text = include_str!("../../../mouseless.toml");
        let config = AppConfig::from_toml_str(text).unwrap();
        assert_eq!(config, AppConfig::default());
    }

    #[test]
    fn empty_document_uses_defaults() {
        assert_eq!(AppConfig::from_toml_str("").unwrap(), AppConfig::default());
    }

    #[test]
    fn unknown_keys_are_rejected() {
        for text in [
            "[paths]\nbogus = 1\n",
            "[kanata]\nbogus = \"x\"\n",
            "[daemon]\nbogus = true\n",
            "[appearance]\nbogus = \"#ffffff\"\n",
        ] {
            assert!(matches!(
                AppConfig::from_toml_str(text),
                Err(AppConfigError::Parse { .. })
            ));
        }
    }

    #[test]
    fn endpoint_and_scheme_validation() {
        let mut config = AppConfig::default();
        config.kanata.address = "127.0.0.1".into();
        assert!(config.validate().is_err());
        config.kanata.address = "127.0.0.1:0".into();
        assert!(config.validate().is_err());
        config.kanata.address = "192.0.2.1:7070".into();
        assert!(config.validate().is_err());
        config.kanata.address = "[::1]:7070".into();
        assert!(config.validate().is_ok());
        config.kanata.address = "127.0.0.1:7070".into();
        config.deep_link.scheme = "1bad".into();
        assert!(config.validate().is_err());
        config.deep_link.scheme = "mouseless".into();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn config_file_must_stay_inside_state_dir() {
        let mut config = AppConfig::default();
        config.paths.config_file = "../config.yaml".into();
        assert!(config.validate().is_err());
        config.paths.config_file = "/etc/config.yaml".into();
        assert!(config.validate().is_err());
        config.paths.config_file = "config.yaml".into();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn colours_and_opacity_are_validated() {
        let mut config = AppConfig::default();
        config.appearance.grid_color = "#d4de0".into();
        assert!(config.validate().is_err());
        config.appearance.grid_color = "#d4de04".into();
        config.appearance.grid_opacity = 1.5;
        assert!(config.validate().is_err());
        config.appearance.grid_opacity = 0.2;
        assert!(config.validate().is_ok());
    }

    #[test]
    fn device_and_include_names_are_plain() {
        let mut config = AppConfig::default();
        config.devices.kanata_output_name = "bad\"name".into();
        assert!(config.validate().is_err());
        config.devices.kanata_output_name = "kanata-mouseless".into();
        config.kanata.include_name = "sub/mouseless.kbd".into();
        assert!(config.validate().is_err());
        config.kanata.include_name = "mouseless.kbd".into();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn movement_tuning_rejects_zero_limits() {
        let mut config = AppConfig::default();
        config.daemon.move_damping_divisor = 0;
        assert!(config.validate().is_err());
        config.daemon.move_damping_divisor = 2;
        config.daemon.move_max_iterations = 0;
        assert!(config.validate().is_err());
        config.daemon.move_max_iterations = 256;
        assert!(config.validate().is_ok());
    }

    #[test]
    fn explicit_missing_file_is_an_error() {
        let loaded = load_effective(Some(Path::new("/nonexistent/mouseless.toml")));
        assert!(loaded.source.is_required());
        assert!(matches!(
            loaded.error,
            Some(AppConfigError::NotFound { .. })
        ));
        assert_eq!(loaded.config, AppConfig::default());
    }

    #[test]
    fn load_reads_a_real_file() {
        let dir = std::env::temp_dir().join(format!(
            "mouseless-appconfig-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("mouseless.toml");
        std::fs::write(&path, "[deep_link]\nscheme = \"ml\"\n").unwrap();
        let config = AppConfig::load(&path).unwrap();
        assert_eq!(config.deep_link.scheme, "ml");
        assert_eq!(config.deep_link.settings_binary, "mouseless-settings");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn endpoint_parser_splits_host_and_port() {
        let mut config = AppConfig::default();
        config.kanata.address = "localhost:7777".into();
        assert_eq!(config.kanata.endpoint(), ("localhost".into(), 7777));
    }
}
