//! Semantic validation for schema v1.
//!
//! Validation returns every issue with a field path so the settings GUI can
//! show each one 1:1 (SPEC-settings) instead of failing on the first problem.
//! All checks are pure and allocation-bounded; no I/O.

use crate::schema::{
    Config, Engine, MonitorAssignment, COLUMN_KEY_POOL, CONFIG_VERSION, ROW_KEY_POOL,
};
use std::fmt;

/// One field-tagged validation issue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    /// Dotted path of the offending field, e.g. `grid.columns`.
    pub field: String,
    /// Human-readable explanation shown in the GUI.
    pub message: String,
}

impl Issue {
    /// Build one issue.
    pub fn new(field: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            field: field.into(),
            message: message.into(),
        }
    }
}

impl fmt::Display for Issue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.field, self.message)
    }
}

/// All semantic issues found in one document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationError {
    issues: Vec<Issue>,
}

impl ValidationError {
    /// The collected issues, in field order.
    pub fn issues(&self) -> &[Issue] {
        &self.issues
    }

    /// Consume into the issues.
    pub fn into_issues(self) -> Vec<Issue> {
        self.issues
    }

    /// True when no issue was collected (never returned by [`validate`]).
    pub fn is_empty(&self) -> bool {
        self.issues.is_empty()
    }
}

impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, issue) in self.issues.iter().enumerate() {
            if index > 0 {
                write!(f, "; ")?;
            }
            write!(f, "{issue}")?;
        }
        Ok(())
    }
}

impl std::error::Error for ValidationError {}

/// Validate a config document against every schema-v1 rule.
pub fn validate(config: &Config) -> Result<(), ValidationError> {
    let mut issues = Vec::new();

    if config.config_version != CONFIG_VERSION {
        issues.push(Issue::new(
            "config_version",
            format!(
                "unsupported version {} (this build implements {CONFIG_VERSION})",
                config.config_version
            ),
        ));
    }

    validate_keys(
        &config.grid.columns,
        COLUMN_KEY_POOL,
        "grid.columns",
        "column key",
        &mut issues,
    );
    validate_keys(
        &config.grid.rows,
        ROW_KEY_POOL,
        "grid.rows",
        "row key",
        &mut issues,
    );

    let sub = &config.grid.subgrid;
    if sub.columns == 0 || sub.rows == 0 {
        issues.push(Issue::new(
            "grid.subgrid",
            "columns and rows must both be non-zero",
        ));
    } else {
        let expected = sub.columns as usize * sub.rows as usize;
        let actual = sub.keys.chars().count();
        if actual != expected {
            issues.push(Issue::new(
                "grid.subgrid.keys",
                format!(
                    "has {actual} keys but {}x{} needs {expected}",
                    sub.columns, sub.rows
                ),
            ));
        }
    }
    validate_keys(
        &sub.keys,
        ROW_KEY_POOL,
        "grid.subgrid.keys",
        "subgrid key",
        &mut issues,
    );

    let monitors = &config.grid.custom_monitors;
    match config.grid.monitor_assignment {
        MonitorAssignment::Custom if monitors.is_empty() => issues.push(Issue::new(
            "grid.custom_monitors",
            "at least one monitor name is required for custom assignment",
        )),
        _ => {}
    }
    if !matches!(config.grid.monitor_assignment, MonitorAssignment::Custom) && !monitors.is_empty()
    {
        issues.push(Issue::new(
            "grid.custom_monitors",
            "only valid when monitor_assignment is custom",
        ));
    }
    for (index, name) in monitors.iter().enumerate() {
        if name.trim().is_empty() {
            issues.push(Issue::new(
                format!("grid.custom_monitors[{index}]"),
                "monitor name must not be empty",
            ));
        }
        if monitors[..index].contains(name) {
            issues.push(Issue::new(
                format!("grid.custom_monitors[{index}]"),
                "duplicate monitor name",
            ));
        }
    }

    let behavior = &config.behavior;
    if !(50..=1000).contains(&behavior.tap_threshold_ms) {
        issues.push(Issue::new(
            "behavior.tap_threshold_ms",
            "must be between 50 and 1000 ms",
        ));
    }
    if behavior.multi_click_threshold_ms > 5000 {
        issues.push(Issue::new(
            "behavior.multi_click_threshold_ms",
            "must be at most 5000 ms (0 disables coalescing)",
        ));
    }
    if !(1..=16).contains(&behavior.nudges_per_cell) {
        issues.push(Issue::new(
            "behavior.nudges_per_cell",
            "must be between 1 and 16",
        ));
    }

    let free = &config.free_mode;
    check_speed(
        free.base_px_per_tick,
        "free_mode.base_px_per_tick",
        &mut issues,
    );
    check_multiplier(
        free.speed_multiplier,
        "free_mode.speed_multiplier",
        &mut issues,
    );
    if free.auto_off_secs > 3600 {
        issues.push(Issue::new(
            "free_mode.auto_off_secs",
            "must be at most 3600 s (0 disables)",
        ));
    }
    check_speed(
        free.wheel.base_lines_per_tick,
        "free_mode.wheel.base_lines_per_tick",
        &mut issues,
    );
    check_multiplier(
        free.wheel.speed_multiplier,
        "free_mode.wheel.speed_multiplier",
        &mut issues,
    );
    if !(1..=100).contains(&free.wheel.step_size) {
        issues.push(Issue::new(
            "free_mode.wheel.step_size",
            "must be between 1 and 100 lines",
        ));
    }
    if !(1..=1000).contains(&free.wheel.step_size_large)
        || free.wheel.step_size_large < free.wheel.step_size
    {
        issues.push(Issue::new(
            "free_mode.wheel.step_size_large",
            "must be >= step_size and between 1 and 1000 lines",
        ));
    }
    if free.engine == Engine::Kanata && free.auto_off_secs > 0 {
        // Not an error: the value is simply unused by kanata. Keep it valid so
        // switching engines back does not surface a stale issue.
    }

    for (field, color) in [
        ("appearance.grid_color", &config.appearance.grid_color),
        ("appearance.label_color", &config.appearance.label_color),
        (
            "appearance.highlight_color",
            &config.appearance.highlight_color,
        ),
    ] {
        if !is_hex_color(color) {
            issues.push(Issue::new(
                field,
                "must be a #rrggbb hex color (for example #d4de04)",
            ));
        }
    }
    let opacity = config.appearance.grid_opacity;
    if !opacity.is_finite() || !(0.0..=1.0).contains(&opacity) {
        issues.push(Issue::new(
            "appearance.grid_opacity",
            "must be a finite value between 0.0 and 1.0",
        ));
    }
    let keys = &config.keybindings;
    for (field, value) in [
        ("keybindings.overlay_layer", &keys.overlay_layer),
        ("keybindings.free_mode_layer", &keys.free_mode_layer),
        (
            "keybindings.kanata_free_mode_layer",
            &keys.kanata_free_mode_layer,
        ),
    ] {
        if value.trim().is_empty() {
            issues.push(Issue::new(field, "layer name must not be empty"));
        } else if !value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            issues.push(Issue::new(
                field,
                "layer names may contain only ASCII letters, digits, '-' and '_'",
            ));
        }
    }
    for (left_field, left, right_field, right) in [(
        "keybindings.overlay_layer",
        &keys.overlay_layer,
        "keybindings.free_mode_layer",
        &keys.free_mode_layer,
    )] {
        if left == right {
            issues.push(Issue::new(
                right_field,
                format!("must differ from {left_field}"),
            ));
        }
    }
    if keys.free_mode_layer == keys.kanata_free_mode_layer {
        issues.push(Issue::new(
            "keybindings.kanata_free_mode_layer",
            "must differ from keybindings.free_mode_layer",
        ));
    }
    if keys.overlay_layer == keys.kanata_free_mode_layer {
        issues.push(Issue::new(
            "keybindings.kanata_free_mode_layer",
            "must differ from keybindings.overlay_layer",
        ));
    }

    // v1 supports only the pinned trigger/action roles; the kanata layer and
    // the evdev translator both hard-code them. Rebinding is Phase 4.
    for (field, value, allowed) in [
        ("keybindings.exit_key", &keys.exit_key, "esc"),
        ("keybindings.action_key", &keys.action_key, "space"),
        (
            "keybindings.monitor_switch_key",
            &keys.monitor_switch_key,
            "shift",
        ),
        ("keybindings.drag_modifier", &keys.drag_modifier, "alt"),
    ] {
        if value != allowed {
            issues.push(Issue::new(
                field,
                format!("v1 supports only \"{allowed}\" (rebinding is Phase 4)"),
            ));
        }
    }

    if issues.is_empty() {
        Ok(())
    } else {
        Err(ValidationError { issues })
    }
}

fn validate_keys(value: &str, pool: &str, field: &str, label: &str, issues: &mut Vec<Issue>) {
    let mut seen = String::new();
    for (index, c) in value.chars().enumerate() {
        if !pool.contains(c) {
            issues.push(Issue::new(
                format!("{field}[{index}]"),
                format!("{label} {c:?} is not in the supported pool {pool:?}"),
            ));
        }
        if seen.contains(c) {
            issues.push(Issue::new(
                format!("{field}[{index}]"),
                format!("duplicate {label} {c:?}"),
            ));
        }
        seen.push(c);
    }
    if value.is_empty() {
        issues.push(Issue::new(
            field,
            format!("at least one {label} is required"),
        ));
    }
}

fn check_speed(value: f64, field: &str, issues: &mut Vec<Issue>) {
    if !value.is_finite() || value <= 0.0 || value > 1000.0 {
        issues.push(Issue::new(
            field,
            "must be finite and greater than 0 (at most 1000)",
        ));
    }
}

fn check_multiplier(value: f64, field: &str, issues: &mut Vec<Issue>) {
    if !value.is_finite() || !(1.0..=16.0).contains(&value) {
        issues.push(Issue::new(field, "must be finite and between 1 and 16"));
    }
}

/// `#rrggbb` only; the GUI color picker always emits this form.
pub fn is_hex_color(value: &str) -> bool {
    parse_hex_color(value).is_some()
}

/// Parse a strict `#rrggbb` colour into RGB components.
///
/// Returns `None` for any other spelling (no shorthand, alpha, names, or
/// whitespace); [`validate`] uses the same parser, so a validated document
/// always round-trips through this function.
pub fn parse_hex_color(value: &str) -> Option<[u8; 3]> {
    let hex = value.strip_prefix('#')?;
    if hex.len() != 6 || !hex.is_ascii() {
        return None;
    }
    let channel = |range: std::ops::Range<usize>| u8::from_str_radix(&hex[range], 16).ok();
    Some([channel(0..2)?, channel(2..4)?, channel(4..6)?])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::*;

    fn issues(config: &Config) -> Vec<String> {
        validate(config)
            .unwrap_err()
            .issues()
            .iter()
            .map(|issue| issue.field.clone())
            .collect()
    }

    #[test]
    fn default_config_is_valid() {
        assert!(validate(&Config::default()).is_ok());
    }

    #[test]
    fn all_issues_are_collected_with_field_paths() {
        let mut config = Config::default();
        config.grid.columns = "aab".into(); // duplicate + outside pool? b is outside
        config.grid.subgrid.columns = 4;
        config.grid.subgrid.keys = "ab".into();
        config.behavior.tap_threshold_ms = 10;
        config.behavior.nudges_per_cell = 0;
        config.free_mode.base_px_per_tick = 0.0;
        config.free_mode.speed_multiplier = 0.5;
        config.free_mode.wheel.step_size = 0;
        config.free_mode.wheel.step_size_large = 0;
        config.appearance.grid_color = "yellow".into();
        config.appearance.grid_opacity = 2.0;
        config.keybindings.overlay_layer = String::new();
        config.keybindings.free_mode_layer = String::new();
        let found = issues(&config);
        for field in [
            "grid.columns[1]",
            "grid.subgrid.keys",
            "behavior.tap_threshold_ms",
            "behavior.nudges_per_cell",
            "free_mode.base_px_per_tick",
            "free_mode.speed_multiplier",
            "free_mode.wheel.step_size",
            "free_mode.wheel.step_size_large",
            "appearance.grid_color",
            "appearance.grid_opacity",
            "keybindings.overlay_layer",
            "keybindings.free_mode_layer",
        ] {
            assert!(
                found.iter().any(|f| f == field),
                "missing {field} in {found:?}"
            );
        }
    }

    #[test]
    fn key_pools_are_enforced() {
        let mut config = Config::default();
        config.grid.columns = "aq".into(); // q is not a column key
        config.grid.rows = "qq".into(); // duplicate
        let found = issues(&config);
        assert!(found.contains(&"grid.columns[1]".to_string()));
        assert!(found.contains(&"grid.rows[1]".to_string()));
    }

    #[test]
    fn subgrid_dimensions_must_match_key_count() {
        let mut config = Config::default();
        config.grid.subgrid.columns = 9;
        assert!(issues(&config).contains(&"grid.subgrid.keys".to_string()));
        config.grid.subgrid.columns = 10;
        config.grid.subgrid.keys.pop();
        assert!(issues(&config).contains(&"grid.subgrid.keys".to_string()));
    }

    #[test]
    fn monitor_assignment_requires_matching_names() {
        let mut config = Config::default();
        config.grid.monitor_assignment = MonitorAssignment::Custom;
        assert!(issues(&config).contains(&"grid.custom_monitors".to_string()));
        config.grid.custom_monitors = vec!["DP-1".into(), "DP-1".into()];
        assert!(issues(&config).contains(&"grid.custom_monitors[1]".to_string()));
        config.grid.monitor_assignment = MonitorAssignment::Auto;
        assert!(issues(&config).contains(&"grid.custom_monitors".to_string()));
    }

    #[test]
    fn layer_names_must_be_distinct_and_safe() {
        let mut config = Config::default();
        config.keybindings.free_mode_layer = config.keybindings.overlay_layer.clone();
        config.keybindings.kanata_free_mode_layer = "bad name".into();
        let found = issues(&config);
        assert!(found.contains(&"keybindings.free_mode_layer".to_string()));
        assert!(found.contains(&"keybindings.kanata_free_mode_layer".to_string()));
    }

    #[test]
    fn hex_color_parser_is_strict() {
        assert!(is_hex_color("#d4de04"));
        assert!(is_hex_color("#ABCDEF"));
        assert!(!is_hex_color("d4de04"));
        assert!(!is_hex_color("#d4de0"));
        assert!(!is_hex_color("#gggggg"));
        assert!(!is_hex_color("#d4de04ff"));
        assert_eq!(parse_hex_color("#d4de04"), Some([0xd4, 0xde, 0x04]));
        assert_eq!(parse_hex_color("#ABCDEF"), Some([0xab, 0xcd, 0xef]));
        // Six bytes but not six ASCII chars: must not slice a char boundary.
        assert_eq!(parse_hex_color("#ééé"), None);
    }

    #[test]
    fn pinned_trigger_roles_are_reported() {
        let mut config = Config::default();
        config.keybindings.exit_key = "q".into();
        config.keybindings.action_key = "enter".into();
        config.keybindings.monitor_switch_key = "ctrl".into();
        config.keybindings.drag_modifier = "meta".into();
        let found = issues(&config);
        for field in [
            "keybindings.exit_key",
            "keybindings.action_key",
            "keybindings.monitor_switch_key",
            "keybindings.drag_modifier",
        ] {
            assert!(found.contains(&field.to_string()), "{field} in {found:?}");
        }
    }

    // Guard against accidental removal of the golden defaults the specs pin.
    #[test]
    fn golden_defaults_are_stable() {
        let config = Config::default();
        assert_eq!(config.appearance.grid_color, "#d4de04");
        assert_eq!(config.appearance.label_color, "#ebe9e8");
        assert_eq!(config.appearance.grid_opacity, 0.2);
    }
}
