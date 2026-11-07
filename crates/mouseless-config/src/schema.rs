//! Frozen `config.yaml` schema v1 (SPEC-settings).
//!
//! Every section is optional in the file and falls back to the documented
//! defaults, so `config_version: 1` alone is a valid Standard config. Unknown
//! keys and unknown enum values are rejected by serde (`deny_unknown_fields`),
//! then [`crate::validate`] enforces semantics.

use serde::{Deserialize, Serialize};

/// Config schema version implemented by this build.
pub const CONFIG_VERSION: u32 = 1;

/// Level-1 key pool (home row), left-to-right — legacy default, no longer enforced.
pub const COLUMN_KEY_POOL: &str = "asdfghjkl;";
/// Level-2 key pool (the full QWERTY letter block), top-to-bottom.
pub const ROW_KEY_POOL: &str = "qwertyuiopasdfghjkl;zxcvbnm,./";
/// Default subgrid keys: the QWERTY block laid out 10 × 3.
pub const SUBGRID_KEY_POOL: &str = ROW_KEY_POOL;

/// Root document of `config.yaml` (schema v1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Must equal [`CONFIG_VERSION`]; required (no silent default).
    pub config_version: u32,
    /// Grid keys, monitor assignment.
    #[serde(default)]
    pub grid: GridConfig,
    /// Overlay behavior and action semantics.
    #[serde(default)]
    pub behavior: BehaviorConfig,
    /// Custom free-mode physics / engine selection.
    #[serde(default)]
    pub free_mode: FreeModeConfig,
    /// Colors and cursor hiding.
    #[serde(default)]
    pub appearance: AppearanceConfig,
    /// Layer names and key roles.
    #[serde(default)]
    pub keybindings: KeybindingConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            config_version: CONFIG_VERSION,
            grid: GridConfig::default(),
            behavior: BehaviorConfig::default(),
            free_mode: FreeModeConfig::default(),
            appearance: AppearanceConfig::default(),
            keybindings: KeybindingConfig::default(),
        }
    }
}

impl Config {
    /// Parse YAML and validate (does not implement schema-version policy).
    pub fn from_yaml_str(text: &str) -> Result<Self, crate::ConfigError> {
        let config: Self =
            serde_yaml_ng::from_str(text).map_err(|e| crate::ConfigError::Parse {
                path: "<memory>".into(),
                message: e.to_string(),
            })?;
        if config.config_version != CONFIG_VERSION {
            return Err(crate::ConfigError::UnsupportedVersion {
                got: config.config_version,
                expected: CONFIG_VERSION,
            });
        }
        crate::validate(&config)?;
        Ok(config)
    }

    /// Serialize to YAML with a stable field order.
    pub fn to_yaml_string(&self) -> Result<String, crate::ConfigError> {
        serde_yaml_ng::to_string(self).map_err(|e| crate::ConfigError::Parse {
            path: "<memory>".into(),
            message: e.to_string(),
        })
    }
}

/// Grid keys and monitor assignment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GridConfig {
    /// Level-1 (column) keys, left-to-right. Any 5-100 unique ASCII keys;
    /// space and the full string `esc` are reserved. [`COLUMN_KEY_POOL`] is
    /// only the default value.
    #[serde(default = "default_columns")]
    pub columns: String,
    /// Level-2 (row) keys, top-to-bottom. Subset of [`ROW_KEY_POOL`].
    #[serde(default = "default_rows")]
    pub rows: String,
    /// Subgrid shown inside the selected primary cell.
    #[serde(default)]
    pub subgrid: SubgridConfig,
    /// Which monitor the overlay opens on.
    #[serde(default)]
    pub monitor_assignment: MonitorAssignment,
    /// Monitor connector/output names for [`MonitorAssignment::Custom`].
    #[serde(default)]
    pub custom_monitors: Vec<String>,
}

impl Default for GridConfig {
    fn default() -> Self {
        Self {
            columns: default_columns(),
            rows: default_rows(),
            subgrid: SubgridConfig::default(),
            monitor_assignment: MonitorAssignment::default(),
            custom_monitors: Vec::new(),
        }
    }
}

/// Subgrid inside a selected primary cell.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubgridConfig {
    /// Number of subgrid columns.
    #[serde(default = "default_subgrid_columns")]
    pub columns: u32,
    /// Number of subgrid rows.
    #[serde(default = "default_subgrid_rows")]
    pub rows: u32,
    /// One key per sub-cell, row-major; must equal `columns * rows`.
    #[serde(default = "default_subgrid_keys")]
    pub keys: String,
}

impl Default for SubgridConfig {
    fn default() -> Self {
        Self {
            columns: default_subgrid_columns(),
            rows: default_subgrid_rows(),
            keys: default_subgrid_keys(),
        }
    }
}

/// Monitor assignment policy (upstream `Initial overlay monitor`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum MonitorAssignment {
    /// Monitor under the cursor, else primary (current daemon behavior).
    #[default]
    Auto,
    /// Reported primary monitor.
    Single,
    /// First monitor whose connector name matches `custom_monitors`.
    Custom,
}

/// Overlay behavior and action semantics.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BehaviorConfig {
    /// Where `Space` acts before a cell is selected.
    #[serde(default)]
    pub initial_action_location: InitialActionLocation,
    /// Whether selecting a cell opens the subgrid or acts immediately.
    #[serde(default)]
    pub grid_action_level: GridActionLevel,
    /// Default action for a cell/sub-cell selection.
    #[serde(default)]
    pub default_action: DefaultAction,
    /// Enter custom free mode after a `move` action.
    #[serde(default)]
    pub free_mode_after_move: bool,
    /// Multi-click cadence window; 0 disables coalescing.
    #[serde(default = "default_multi_click_ms")]
    pub multi_click_threshold_ms: u64,
    /// Tap window for the overlay/free-mode triggers (kanata include).
    #[serde(default = "default_tap_ms")]
    pub tap_threshold_ms: u64,
    /// Nudges per sub-cell edge (minimum 1).
    #[serde(default = "default_nudges")]
    pub nudges_per_cell: u32,
    /// Hold a subgrid key to nudge before releasing to click.
    #[serde(default)]
    pub hold_subgrid_for_nudge: bool,
    /// Keep the overlay visible after a click until Esc/layer exit.
    #[serde(default)]
    pub continuous_mode: bool,
    /// Modifier that toggles the default action on sub-cell selection
    /// (click→move-only, move→left-click). Default "shift".
    #[serde(default = "default_toggle_action_modifier")]
    pub toggle_action_modifier: String,
    /// Modifier that forces a right-click on sub-cell selection. Default "ctrl".
    #[serde(default = "default_right_click_modifier")]
    pub right_click_modifier: String,
    /// Alt held on the final key begins/continues a drag.
    #[serde(default = "default_true")]
    pub hold_for_drag: bool,
}

impl Default for BehaviorConfig {
    fn default() -> Self {
        Self {
            initial_action_location: InitialActionLocation::default(),
            grid_action_level: GridActionLevel::default(),
            default_action: DefaultAction::default(),
            free_mode_after_move: false,
            multi_click_threshold_ms: default_multi_click_ms(),
            tap_threshold_ms: default_tap_ms(),
            nudges_per_cell: default_nudges(),
            hold_subgrid_for_nudge: false,
            continuous_mode: false,
            toggle_action_modifier: default_toggle_action_modifier(),
            right_click_modifier: default_right_click_modifier(),
            hold_for_drag: true,
        }
    }
}

/// Where a cell-less action executes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum InitialActionLocation {
    /// Center of the active monitor (default).
    #[default]
    ScreenCenter,
    /// Current system cursor.
    SystemCursor,
}

/// Keypresses required before a mouse action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum GridActionLevel {
    /// Open the 10 × 3 subgrid; the third key executes (default).
    #[default]
    #[serde(rename = "subgrid")]
    Subgrid,
    /// Execute at the selected primary cell after two keys.
    #[serde(rename = "2")]
    Two,
}

/// Default mouse action for a selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum DefaultAction {
    /// Left-click at the target (default).
    #[default]
    Click,
    /// Move the cursor to the target.
    Move,
}

/// Free-mode motion parameters.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FreeModeConfig {
    /// Custom physics or kanata's `lyr-mouse` fallback.
    #[serde(default)]
    pub engine: Engine,
    /// Base move speed in px per tick.
    #[serde(default = "default_base_px")]
    pub base_px_per_tick: f64,
    /// Speed multiplier per held boost key.
    #[serde(default = "default_multiplier")]
    pub speed_multiplier: f64,
    /// Movement easing curve.
    #[serde(default)]
    pub easing: Easing,
    /// Auto-off after N idle seconds; 0 disables.
    #[serde(default = "default_auto_off_secs")]
    pub auto_off_secs: u64,
    /// Wheel physics.
    #[serde(default)]
    pub wheel: WheelConfig,
}

impl Default for FreeModeConfig {
    fn default() -> Self {
        Self {
            engine: Engine::default(),
            base_px_per_tick: default_base_px(),
            speed_multiplier: default_multiplier(),
            easing: Easing::default(),
            auto_off_secs: default_auto_off_secs(),
            wheel: WheelConfig::default(),
        }
    }
}

/// Free-mode engine selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Engine {
    /// Custom physics in `mouseless-state` (default).
    #[default]
    Custom,
    /// Kanata's native `lyr-mouse` layer.
    Kanata,
}

/// Movement/wheel easing curve.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Easing {
    /// Loose smoothing.
    Loose,
    /// Natural smoothing (default).
    #[default]
    Natural,
    /// Snappy smoothing.
    Snappy,
    /// Near-instant smoothing.
    Instant,
}

/// Wheel physics parameters.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WheelConfig {
    /// Base wheel speed in lines per tick.
    #[serde(default = "default_wheel_base")]
    pub base_lines_per_tick: f64,
    /// Speed multiplier for fast wheel action.
    #[serde(default = "default_multiplier")]
    pub speed_multiplier: f64,
    /// Wheel easing curve.
    #[serde(default)]
    pub easing: Easing,
    /// Discrete wheel step (lines).
    #[serde(default = "default_wheel_step")]
    pub step_size: i32,
    /// Large discrete wheel step (lines).
    #[serde(default = "default_wheel_step_large")]
    pub step_size_large: i32,
}

impl Default for WheelConfig {
    fn default() -> Self {
        Self {
            base_lines_per_tick: default_wheel_base(),
            speed_multiplier: default_multiplier(),
            easing: Easing::default(),
            step_size: default_wheel_step(),
            step_size_large: default_wheel_step_large(),
        }
    }
}

/// Active color theme: the pinned dark look, or the light variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Theme {
    /// Dark grid fill with light labels (the pinned Phase-3 look).
    #[default]
    Dark,
    /// Light grid fill with dark labels.
    Light,
}

/// Colors, opacity, and cursor hiding.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppearanceConfig {
    /// Active theme: picks which color set the daemon renders.
    #[serde(default = "default_theme")]
    pub theme: Theme,
    /// Grid fill color, `#rrggbb` (dark theme).
    #[serde(default = "default_grid_color")]
    pub grid_color: String,
    /// Grid fill opacity, 0.0..=1.0 (0.2 = 80% transparent) (dark theme).
    #[serde(default = "default_grid_opacity")]
    pub grid_opacity: f64,
    /// Cell label color, `#rrggbb` (dark theme).
    #[serde(default = "default_label_color")]
    pub label_color: String,
    /// Highlight color, `#rrggbb` (dark theme).
    #[serde(default = "default_highlight_color")]
    pub highlight_color: String,
    /// Light theme grid fill color, `#rrggbb`.
    #[serde(default = "default_light_grid_color")]
    pub light_grid_color: String,
    /// Light theme cell label color, `#rrggbb`.
    #[serde(default = "default_light_label_color")]
    pub light_label_color: String,
    /// Light theme highlight color, `#rrggbb`.
    #[serde(default = "default_light_highlight_color")]
    pub light_highlight_color: String,
    /// Light theme grid fill opacity, 0.0..=1.0.
    #[serde(default = "default_light_grid_opacity")]
    pub light_grid_opacity: f64,
    /// Move the cursor away after a click.
    #[serde(default)]
    pub hide_cursor_on_click: bool,
    /// Corner used by `hide_cursor_on_click`.
    #[serde(default)]
    pub cursor_hide_corner: CursorCorner,
}

impl Default for AppearanceConfig {
    fn default() -> Self {
        Self {
            theme: default_theme(),
            grid_color: default_grid_color(),
            grid_opacity: default_grid_opacity(),
            label_color: default_label_color(),
            highlight_color: default_highlight_color(),
            light_grid_color: default_light_grid_color(),
            light_label_color: default_light_label_color(),
            light_highlight_color: default_light_highlight_color(),
            light_grid_opacity: default_light_grid_opacity(),
            hide_cursor_on_click: false,
            cursor_hide_corner: CursorCorner::default(),
        }
    }
}

/// Screen corner for cursor hiding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum CursorCorner {
    /// Top-left.
    TopLeft,
    /// Top-right (default).
    #[default]
    TopRight,
    /// Bottom-left.
    BottomLeft,
    /// Bottom-right.
    BottomRight,
}

/// Layer names and key roles (kanata-facing).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeybindingConfig {
    /// Overlay layer (`lyr-mouseless`).
    #[serde(default = "default_overlay_layer")]
    pub overlay_layer: String,
    /// Custom free-mode layer (`lyr-free`).
    #[serde(default = "default_free_layer")]
    pub free_mode_layer: String,
    /// Kanata-native free-mode fallback (`lyr-mouse`).
    #[serde(default = "default_kanata_free_layer")]
    pub kanata_free_mode_layer: String,
    /// Exit key name (`esc`).
    #[serde(default = "default_exit_key")]
    pub exit_key: String,
    /// Action key name (`space`).
    #[serde(default = "default_action_key")]
    pub action_key: String,
    /// Monitor-switch modifier name (`shift`).
    #[serde(default = "default_monitor_key")]
    pub monitor_switch_key: String,
    /// Drag modifier name (`alt`).
    #[serde(default = "default_drag_modifier")]
    pub drag_modifier: String,
    /// Kanata key name for the overlay tap-hold trigger (default `lctl`).
    #[serde(default = "default_overlay_trigger_key")]
    pub overlay_trigger_key: String,
    /// Kanata key name for the free-mode tap-hold trigger (default empty — no
    /// dedicated trigger; free mode is entered via `free_mode_after_move`).
    #[serde(default = "default_free_mode_trigger_key")]
    pub free_mode_trigger_key: String,
}

impl Default for KeybindingConfig {
    fn default() -> Self {
        Self {
            overlay_layer: default_overlay_layer(),
            free_mode_layer: default_free_layer(),
            kanata_free_mode_layer: default_kanata_free_layer(),
            exit_key: default_exit_key(),
            action_key: default_action_key(),
            monitor_switch_key: default_monitor_key(),
            drag_modifier: default_drag_modifier(),
            overlay_trigger_key: default_overlay_trigger_key(),
            free_mode_trigger_key: default_free_mode_trigger_key(),
        }
    }
}

fn default_toggle_action_modifier() -> String {
    "shift".into()
}

fn default_right_click_modifier() -> String {
    "ctrl".into()
}

fn default_true() -> bool {
    true
}

fn default_columns() -> String {
    COLUMN_KEY_POOL.into()
}

fn default_rows() -> String {
    ROW_KEY_POOL.into()
}

fn default_subgrid_columns() -> u32 {
    10
}

fn default_subgrid_rows() -> u32 {
    3
}

fn default_subgrid_keys() -> String {
    SUBGRID_KEY_POOL.into()
}

fn default_multi_click_ms() -> u64 {
    500
}

fn default_tap_ms() -> u64 {
    200
}

fn default_nudges() -> u32 {
    4
}

fn default_base_px() -> f64 {
    10.0
}

fn default_multiplier() -> f64 {
    2.0
}

fn default_auto_off_secs() -> u64 {
    10
}

fn default_wheel_base() -> f64 {
    1.0
}

fn default_wheel_step() -> i32 {
    3
}

fn default_wheel_step_large() -> i32 {
    10
}

fn default_grid_color() -> String {
    crate::appconfig::DEFAULT_GRID_COLOR.into()
}

fn default_grid_opacity() -> f64 {
    crate::appconfig::DEFAULT_GRID_OPACITY
}

fn default_label_color() -> String {
    crate::appconfig::DEFAULT_LABEL_COLOR.into()
}

fn default_highlight_color() -> String {
    crate::appconfig::DEFAULT_HIGHLIGHT_COLOR.into()
}

fn default_theme() -> Theme {
    Theme::Dark
}

fn default_light_grid_color() -> String {
    "#c9aa03".into()
}

fn default_light_label_color() -> String {
    "#000000".into()
}

fn default_light_highlight_color() -> String {
    crate::appconfig::DEFAULT_HIGHLIGHT_COLOR.into()
}

fn default_light_grid_opacity() -> f64 {
    crate::appconfig::DEFAULT_GRID_OPACITY
}

fn default_overlay_layer() -> String {
    "lyr-mouseless".into()
}

fn default_free_layer() -> String {
    "lyr-free".into()
}

fn default_kanata_free_layer() -> String {
    "lyr-mouse".into()
}

fn default_exit_key() -> String {
    "esc".into()
}

fn default_action_key() -> String {
    "space".into()
}

fn default_monitor_key() -> String {
    "shift".into()
}

fn default_drag_modifier() -> String {
    "alt".into()
}

fn default_overlay_trigger_key() -> String {
    "lctl".into()
}

fn default_free_mode_trigger_key() -> String {
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimal_document_uses_documented_defaults() {
        let config = Config::from_yaml_str("config_version: 1\n").unwrap();
        assert_eq!(config, Config::default());
        assert_eq!(config.grid.columns, "asdfghjkl;");
        assert_eq!(config.grid.rows.chars().count(), 30);
        assert_eq!(config.grid.subgrid.columns, 10);
        assert_eq!(config.grid.subgrid.rows, 3);
        assert_eq!(config.grid.subgrid.keys.chars().count(), 30);
        assert_eq!(config.behavior.tap_threshold_ms, 200);
        assert_eq!(config.appearance.grid_color, "#d4de04");
        assert_eq!(config.appearance.grid_opacity, 0.2);
    }

    #[test]
    fn yaml_round_trip_preserves_every_section() {
        let config = Config::default();
        let yaml = config.to_yaml_string().unwrap();
        let parsed = Config::from_yaml_str(&yaml).unwrap();
        assert_eq!(parsed, config);
    }

    #[test]
    fn unknown_keys_are_rejected_at_every_level() {
        for text in [
            "config_version: 1\nextra: true\n",
            "config_version: 1\ngrid:\n  extra: true\n",
            "config_version: 1\nbehavior:\n  extra: true\n",
            "config_version: 1\nfree_mode:\n  wheel:\n    extra: true\n",
        ] {
            assert!(matches!(
                Config::from_yaml_str(text),
                Err(crate::ConfigError::Parse { .. })
            ));
        }
    }

    #[test]
    fn unknown_enum_values_are_rejected() {
        let text = "config_version: 1\nbehavior:\n  default_action: wiggle\n";
        assert!(matches!(
            Config::from_yaml_str(text),
            Err(crate::ConfigError::Parse { .. })
        ));
    }

    #[test]
    fn schema_version_is_required_and_pinned() {
        assert!(matches!(
            Config::from_yaml_str("grid:\n  columns: a\n"),
            Err(crate::ConfigError::Parse { .. })
        ));
        assert!(matches!(
            Config::from_yaml_str("config_version: 2\n"),
            Err(crate::ConfigError::UnsupportedVersion {
                got: 2,
                expected: 1
            })
        ));
    }

    #[test]
    fn grid_action_level_two_uses_the_pinned_string() {
        let text = "config_version: 1\nbehavior:\n  grid_action_level: 2\n";
        let config = Config::from_yaml_str(text).unwrap();
        assert_eq!(config.behavior.grid_action_level, GridActionLevel::Two);
        let yaml = config.to_yaml_string().unwrap();
        assert!(yaml.contains("grid_action_level: '2'") || yaml.contains("grid_action_level: 2"));
    }

    #[test]
    fn kebab_case_enums_round_trip() {
        let text = "config_version: 1\n\
                    behavior:\n  initial_action_location: system-cursor\n  default_action: move\n\
                    free_mode:\n  engine: kanata\n  easing: snappy\n\
                    appearance:\n  cursor_hide_corner: bottom-left\n\
                    grid:\n  monitor_assignment: custom\n  custom_monitors: [DP-1]\n";
        let config = Config::from_yaml_str(text).unwrap();
        assert_eq!(
            config.behavior.initial_action_location,
            InitialActionLocation::SystemCursor
        );
        assert_eq!(config.behavior.default_action, DefaultAction::Move);
        assert_eq!(config.free_mode.engine, Engine::Kanata);
        assert_eq!(config.free_mode.easing, Easing::Snappy);
        assert_eq!(
            config.appearance.cursor_hide_corner,
            CursorCorner::BottomLeft
        );
        assert_eq!(config.grid.monitor_assignment, MonitorAssignment::Custom);
        assert_eq!(config.grid.custom_monitors, vec!["DP-1"]);
    }

    #[test]
    fn theme_defaults_to_dark_and_light_fields_have_documented_defaults() {
        let config = Config::default();
        assert_eq!(config.appearance.theme, Theme::Dark);
        assert_eq!(config.appearance.light_grid_color, "#c9aa03");
        assert_eq!(config.appearance.light_label_color, "#000000");
        assert_eq!(
            config.appearance.light_highlight_color,
            crate::appconfig::DEFAULT_HIGHLIGHT_COLOR
        );
        assert_eq!(config.appearance.light_grid_opacity, 0.2);
    }

    #[test]
    fn theme_and_light_colors_round_trip_through_yaml() {
        let text = "config_version: 1\nappearance:\n  theme: light\n  light_grid_color: '#c9aa03'\n  light_label_color: '#000000'\n  light_highlight_color: '#ffb22c'\n  light_grid_opacity: 0.2\n";
        let config = Config::from_yaml_str(text).unwrap();
        assert_eq!(config.appearance.theme, Theme::Light);
        let yaml = config.to_yaml_string().unwrap();
        let parsed = Config::from_yaml_str(&yaml).unwrap();
        assert_eq!(parsed.appearance.theme, Theme::Light);
        assert_eq!(parsed.appearance.light_grid_color, "#c9aa03");
    }
}
