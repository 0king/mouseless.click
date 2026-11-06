//! Programmatic Adwaita form widgets.
//!
//! Blueprint was evaluated, but `blueprint-compiler` is a system build
//! dependency that is not available everywhere the workspace builds. The UI is
//! therefore plain typed Rust over `libadwaita`, which keeps `cargo run -p
//! mouseless-gtk` self-contained and the config engine GTK-free. See
//! `ui/README.md`.

use crate::model;
use crate::preview::Preview;
use crate::state::AppState;
use adw::prelude::*;
use gtk::prelude::*;
use libadwaita as adw;
use mouseless_config::{
    BehaviorConfig, Config, CursorCorner, DefaultAction, Easing, Engine, GridActionLevel,
    InitialActionLocation, MonitorAssignment, Theme, ROW_KEY_POOL,
};
use std::rc::Rc;

/// Monitor assignment choices (index = schema enum order).
pub const MONITOR_LABELS: [&str; 3] = [
    "Automatic (cursor monitor, then primary)",
    "Primary monitor",
    "Custom connector names",
];
/// Initial action location choices.
pub const INITIAL_LABELS: [&str; 2] = ["Screen centre", "System cursor"];
/// Grid action level choices.
pub const LEVEL_LABELS: [&str; 2] = ["Open the subgrid (third key acts)", "Act after two keys"];
/// Default action choices.
pub const ACTION_LABELS: [&str; 2] = ["Left click", "Move the cursor"];
/// Free-mode engine choices.
pub const ENGINE_LABELS: [&str; 2] = ["Custom physics", "Kanata lyr-mouse fallback"];
/// Easing choices.
pub const EASING_LABELS: [&str; 4] = ["Loose", "Natural", "Snappy", "Instant"];
/// Cursor-hide corner choices.
pub const CORNER_LABELS: [&str; 4] = ["Top left", "Top right", "Bottom left", "Bottom right"];
/// Theme choices (index matches the schema enum order).
pub const THEME_LABELS: [&str; 2] = ["Dark", "Light"];

/// Entry row with a live hint subtitle.
#[derive(Clone)]
pub struct HintEntry {
    /// Row added to a group.
    pub row: adw::ActionRow,
    /// Underlying editable.
    pub entry: gtk::Entry,
}

impl HintEntry {
    fn new(title: &str, placeholder: &str) -> Self {
        let entry = gtk::Entry::new();
        entry.set_hexpand(true);
        entry.set_placeholder_text(Some(placeholder));
        entry.set_valign(gtk::Align::Center);
        let row = adw::ActionRow::builder().title(title).build();
        row.add_suffix(&entry);
        row.set_activatable_widget(Some(&entry));
        Self { row, entry }
    }

    fn text(&self) -> String {
        self.entry.text().to_string()
    }

    fn set_text(&self, text: &str) {
        self.entry.set_text(text);
    }

    fn set_hint(&self, hint: &str) {
        self.row.set_subtitle(hint);
    }
}

/// All editable settings widgets.
///
/// The form reads values on demand (`to_config`) instead of mirroring them in
/// a second draft document; there is exactly one editable representation.
pub struct Form {
    // Grid
    /// Level-1 keys.
    pub columns: HintEntry,
    /// Level-2 keys.
    pub rows: HintEntry,
    /// Subgrid columns.
    pub subgrid_columns: adw::SpinRow,
    /// Subgrid rows.
    pub subgrid_rows: adw::SpinRow,
    /// Subgrid one-char keys.
    pub subgrid_keys: HintEntry,
    /// Monitor assignment policy.
    pub monitor_assignment: adw::ComboRow,
    /// Custom monitor connector names.
    pub custom_monitors: HintEntry,

    // Appearance
    /// Active theme: picks which color set the pickers edit.
    pub theme: adw::ComboRow,
    /// Grid fill colour picker.
    pub grid_color: gtk::ColorButton,
    /// Grid fill opacity (0–100 %).
    pub grid_opacity: gtk::Scale,
    /// Opacity row (subtitle shows the percentage).
    pub grid_opacity_row: adw::ActionRow,
    /// Label colour picker.
    pub label_color: gtk::ColorButton,
    /// Highlight colour picker.
    pub highlight_color: gtk::ColorButton,
    /// Hide the cursor after a click.
    pub hide_cursor: adw::SwitchRow,
    /// Corner used for cursor hiding.
    pub cursor_corner: adw::ComboRow,
    /// Live subgrid preview.
    pub preview: Preview,

    // Behavior
    /// Initial action location.
    pub initial_location: adw::ComboRow,
    /// Grid action level.
    pub grid_level: adw::ComboRow,
    /// Default action.
    pub default_action: adw::ComboRow,
    /// Enter free mode after a move.
    pub free_mode_after_move: adw::SwitchRow,
    /// Multi-click coalescing window.
    pub multi_click: adw::SpinRow,
    /// Tap-hold window (kanata include).
    pub tap: adw::SpinRow,
    /// Nudges per subgrid edge.
    pub nudges: adw::SpinRow,
    /// Hold a subgrid key to nudge.
    pub hold_subgrid: adw::SwitchRow,
    /// Keep the overlay visible after a click.
    pub continuous: adw::SwitchRow,
    /// Modifier that toggles the default action (default "shift").
    pub toggle_modifier: adw::EntryRow,
    /// Modifier that forces a right-click (default "ctrl").
    pub right_click_modifier: adw::EntryRow,
    /// Alt begins/continues a drag.
    pub hold_drag: adw::SwitchRow,

    // Free mode
    /// Custom physics or kanata.
    pub engine: adw::ComboRow,
    /// Base movement speed.
    pub base_speed: adw::SpinRow,
    /// Speed multiplier.
    pub speed_multiplier: adw::SpinRow,
    /// Movement easing.
    pub easing: adw::ComboRow,
    /// Idle auto-off.
    pub auto_off: adw::SpinRow,
    /// Wheel base speed.
    pub wheel_base: adw::SpinRow,
    /// Wheel speed multiplier.
    pub wheel_multiplier: adw::SpinRow,
    /// Wheel easing.
    pub wheel_easing: adw::ComboRow,
    /// Wheel step.
    pub wheel_step: adw::SpinRow,
    /// Large wheel step.
    pub wheel_step_large: adw::SpinRow,

    // Keybindings
    /// Overlay layer name.
    pub overlay_layer: adw::EntryRow,
    /// Custom free-mode layer name.
    pub free_mode_layer: adw::EntryRow,
    /// Kanata-native free-mode layer name.
    pub kanata_free_layer: adw::EntryRow,
    /// Exit key name.
    pub exit_key: adw::EntryRow,
    /// Action key name.
    pub action_key: adw::EntryRow,
    /// Monitor-switch modifier name.
    pub monitor_key: adw::EntryRow,
    /// Drag modifier name.
    pub drag_modifier: adw::EntryRow,

    // Presets
    /// Name used by "Save current behavior".
    pub preset_name: adw::EntryRow,
}

impl Form {
    /// Build the widgets from a config document.
    pub fn new(config: &Config) -> Self {
        let appearance = &crate::model::app_config().appearance;
        let opacity_percent = (appearance.grid_opacity * 100.0).clamp(0.0, 100.0);
        let form = Self {
            columns: HintEntry::new("Level-1 keys (columns)", "e.g. asdfghjkl;qwerty"),
            rows: HintEntry::new("Level-2 keys (rows)", "qwertyuiopasdfghjkl;zxcvbnm,./"),
            subgrid_columns: spin_row("Subgrid columns", 1.0, 30.0, 1.0, 0, 10.0),
            subgrid_rows: spin_row("Subgrid rows", 1.0, 10.0, 1.0, 0, 3.0),
            subgrid_keys: HintEntry::new(
                "Subgrid keys (row-major)",
                "qwertyuiopasdfghjkl;zxcvbnm,./",
            ),
            monitor_assignment: combo_row("Monitor assignment", &MONITOR_LABELS, 0),
            custom_monitors: HintEntry::new("Custom connectors", "DP-1, HDMI-1"),

            theme: combo_row("Theme", &THEME_LABELS, 0),
            grid_color: color_button(&appearance.grid_color),
            grid_opacity: gtk::Scale::new(
                gtk::Orientation::Horizontal,
                Some(&gtk::Adjustment::new(
                    opacity_percent,
                    0.0,
                    100.0,
                    1.0,
                    10.0,
                    0.0,
                )),
            ),
            grid_opacity_row: adw::ActionRow::builder()
                .title("Grid opacity")
                .subtitle(format!(
                    "{opacity_percent:.0}% opacity ({:.0}% transparent)",
                    100.0 - opacity_percent
                ))
                .build(),
            label_color: color_button(&appearance.label_color),
            highlight_color: color_button(&appearance.highlight_color),
            hide_cursor: switch_row(
                "Hide the cursor after a click",
                "Move the pointer to a screen corner after each click",
                false,
            ),
            cursor_corner: combo_row("Cursor hiding corner", &CORNER_LABELS, 1),
            preview: Preview::new(),

            initial_location: combo_row("Initial action location", &INITIAL_LABELS, 0),
            grid_level: combo_row("Grid action level", &LEVEL_LABELS, 0),
            default_action: combo_row("Default action", &ACTION_LABELS, 0),
            free_mode_after_move: switch_row(
                "Enter free mode after a move",
                "Useful for quick-select workflows",
                false,
            ),
            multi_click: spin_row("Multi-click window", 0.0, 5000.0, 50.0, 0, 500.0),
            tap: spin_row("Overlay/free-mode tap window", 50.0, 1000.0, 10.0, 0, 200.0),
            nudges: spin_row("Nudges per subgrid edge", 1.0, 16.0, 1.0, 0, 4.0),
            hold_subgrid: switch_row(
                "Hold a subgrid key to nudge",
                "Release to click; hold to nudge in that direction",
                false,
            ),
            continuous: switch_row(
                "Continuous mode",
                "Keep the overlay visible after a click until Esc",
                false,
            ),
            toggle_modifier: entry_row("Toggle default-action modifier"),
            right_click_modifier: entry_row("Right-click modifier"),
            hold_drag: switch_row(
                "Alt drags",
                "Hold Alt on the final key to begin or continue a drag",
                true,
            ),

            engine: combo_row("Free-mode engine", &ENGINE_LABELS, 0),
            base_speed: spin_row("Base speed (px/tick)", 0.5, 50.0, 0.5, 1, 10.0),
            speed_multiplier: spin_row("Boost multiplier", 1.0, 10.0, 0.25, 2, 2.0),
            easing: combo_row("Movement easing", &EASING_LABELS, 1),
            auto_off: spin_row("Auto-off after idle (s)", 0.0, 3600.0, 5.0, 0, 10.0),
            wheel_base: spin_row("Wheel base speed (lines/tick)", 0.25, 20.0, 0.25, 2, 1.0),
            wheel_multiplier: spin_row("Wheel boost multiplier", 1.0, 10.0, 0.25, 2, 2.0),
            wheel_easing: combo_row("Wheel easing", &EASING_LABELS, 1),
            wheel_step: spin_row("Wheel step (lines)", 1.0, 100.0, 1.0, 0, 3.0),
            wheel_step_large: spin_row("Large wheel step (lines)", 1.0, 1000.0, 1.0, 0, 10.0),

            overlay_layer: entry_row("Overlay layer"),
            free_mode_layer: entry_row("Free-mode layer"),
            kanata_free_layer: entry_row("Kanata free-mode layer"),
            exit_key: entry_row("Exit key"),
            action_key: entry_row("Action key"),
            monitor_key: entry_row("Monitor-switch modifier"),
            drag_modifier: entry_row("Drag modifier"),

            preset_name: entry_row("Preset name"),
        };
        form.load(config);
        form
    }

    /// Replace every widget value from `config`.
    pub fn load(&self, config: &Config) {
        self.columns.set_text(&config.grid.columns);
        self.rows.set_text(&config.grid.rows);
        self.subgrid_columns
            .set_value(f64::from(config.grid.subgrid.columns));
        self.subgrid_rows
            .set_value(f64::from(config.grid.subgrid.rows));
        self.subgrid_keys.set_text(&config.grid.subgrid.keys);
        self.monitor_assignment
            .set_selected(monitor_index(config.grid.monitor_assignment));
        self.custom_monitors
            .set_text(&model::join_monitors(&config.grid.custom_monitors));
        self.custom_monitors.row.set_visible(matches!(
            config.grid.monitor_assignment,
            MonitorAssignment::Custom
        ));

        self.load_appearance_colors(config);
        self.hide_cursor
            .set_active(config.appearance.hide_cursor_on_click);
        self.cursor_corner
            .set_selected(corner_index(config.appearance.cursor_hide_corner));

        self.initial_location
            .set_selected(match config.behavior.initial_action_location {
                InitialActionLocation::ScreenCenter => 0,
                InitialActionLocation::SystemCursor => 1,
            });
        self.grid_level
            .set_selected(match config.behavior.grid_action_level {
                GridActionLevel::Subgrid => 0,
                GridActionLevel::Two => 1,
            });
        self.default_action
            .set_selected(match config.behavior.default_action {
                DefaultAction::Click => 0,
                DefaultAction::Move => 1,
            });
        self.free_mode_after_move
            .set_active(config.behavior.free_mode_after_move);
        self.multi_click
            .set_value(config.behavior.multi_click_threshold_ms as f64);
        self.tap.set_value(config.behavior.tap_threshold_ms as f64);
        self.nudges
            .set_value(f64::from(config.behavior.nudges_per_cell));
        self.hold_subgrid
            .set_active(config.behavior.hold_subgrid_for_nudge);
        self.continuous.set_active(config.behavior.continuous_mode);
        self.toggle_modifier
            .set_text(&config.behavior.toggle_action_modifier);
        self.right_click_modifier
            .set_text(&config.behavior.right_click_modifier);
        self.hold_drag.set_active(config.behavior.hold_for_drag);

        self.engine.set_selected(match config.free_mode.engine {
            Engine::Custom => 0,
            Engine::Kanata => 1,
        });
        self.base_speed.set_value(config.free_mode.base_px_per_tick);
        self.speed_multiplier
            .set_value(config.free_mode.speed_multiplier);
        self.easing
            .set_selected(easing_index(config.free_mode.easing));
        self.auto_off
            .set_value(config.free_mode.auto_off_secs as f64);
        self.wheel_base
            .set_value(config.free_mode.wheel.base_lines_per_tick);
        self.wheel_multiplier
            .set_value(config.free_mode.wheel.speed_multiplier);
        self.wheel_easing
            .set_selected(easing_index(config.free_mode.wheel.easing));
        self.wheel_step
            .set_value(f64::from(config.free_mode.wheel.step_size));
        self.wheel_step_large
            .set_value(f64::from(config.free_mode.wheel.step_size_large));

        self.overlay_layer
            .set_text(&config.keybindings.overlay_layer);
        self.free_mode_layer
            .set_text(&config.keybindings.free_mode_layer);
        self.kanata_free_layer
            .set_text(&config.keybindings.kanata_free_mode_layer);
        self.exit_key.set_text(&config.keybindings.exit_key);
        self.action_key.set_text(&config.keybindings.action_key);
        self.monitor_key
            .set_text(&config.keybindings.monitor_switch_key);
        self.drag_modifier
            .set_text(&config.keybindings.drag_modifier);

        self.refresh_hints();
        self.refresh_preview();
        self.update_opacity_subtitle();
    }

    /// Fill the appearance pickers from the active theme's color set in
    /// `config`, so switching themes swaps the pickers to that theme's values.
    fn load_appearance_colors(&self, config: &Config) {
        self.theme.set_selected(match config.appearance.theme {
            Theme::Dark => 0,
            Theme::Light => 1,
        });
        let (grid, label, highlight, opacity) = match config.appearance.theme {
            Theme::Dark => (
                &config.appearance.grid_color,
                &config.appearance.label_color,
                &config.appearance.highlight_color,
                config.appearance.grid_opacity,
            ),
            Theme::Light => (
                &config.appearance.light_grid_color,
                &config.appearance.light_label_color,
                &config.appearance.light_highlight_color,
                config.appearance.light_grid_opacity,
            ),
        };
        set_color(&self.grid_color, grid);
        set_color(&self.label_color, label);
        set_color(&self.highlight_color, highlight);
        self.grid_opacity
            .set_value((opacity * 100.0).clamp(0.0, 100.0));
    }

    /// Build a complete candidate document, preserving every section the form
    /// does not edit (there are none today, but this keeps schema growth safe).
    pub fn to_config(&self, base: &Config) -> Config {
        let mut config = base.clone();
        config.grid.columns = self.columns.text();
        config.grid.rows = self.rows.text();
        config.grid.subgrid.columns = self.subgrid_columns.value() as u32;
        config.grid.subgrid.rows = self.subgrid_rows.value() as u32;
        config.grid.subgrid.keys = self.subgrid_keys.text();
        config.grid.monitor_assignment = match self.monitor_assignment.selected() {
            1 => MonitorAssignment::Single,
            2 => MonitorAssignment::Custom,
            _ => MonitorAssignment::Auto,
        };
        config.grid.custom_monitors = model::split_monitors(&self.custom_monitors.text());

        config.appearance.theme = match self.theme.selected() {
            1 => Theme::Light,
            _ => Theme::Dark,
        };
        let opacity = (self.grid_opacity.value() / 100.0).clamp(0.0, 1.0);
        match config.appearance.theme {
            Theme::Dark => {
                config.appearance.grid_color = color_hex(&self.grid_color);
                config.appearance.grid_opacity = opacity;
                config.appearance.label_color = color_hex(&self.label_color);
                config.appearance.highlight_color = color_hex(&self.highlight_color);
            }
            Theme::Light => {
                config.appearance.light_grid_color = color_hex(&self.grid_color);
                config.appearance.light_grid_opacity = opacity;
                config.appearance.light_label_color = color_hex(&self.label_color);
                config.appearance.light_highlight_color = color_hex(&self.highlight_color);
            }
        }
        config.appearance.hide_cursor_on_click = self.hide_cursor.is_active();
        config.appearance.cursor_hide_corner = match self.cursor_corner.selected() {
            0 => CursorCorner::TopLeft,
            2 => CursorCorner::BottomLeft,
            3 => CursorCorner::BottomRight,
            _ => CursorCorner::TopRight,
        };

        config.behavior = self.to_behavior();

        config.free_mode.engine = match self.engine.selected() {
            1 => Engine::Kanata,
            _ => Engine::Custom,
        };
        config.free_mode.base_px_per_tick = self.base_speed.value();
        config.free_mode.speed_multiplier = self.speed_multiplier.value();
        config.free_mode.easing = easing_from_index(self.easing.selected());
        config.free_mode.auto_off_secs = self.auto_off.value().max(0.0) as u64;
        config.free_mode.wheel.base_lines_per_tick = self.wheel_base.value();
        config.free_mode.wheel.speed_multiplier = self.wheel_multiplier.value();
        config.free_mode.wheel.easing = easing_from_index(self.wheel_easing.selected());
        config.free_mode.wheel.step_size = self.wheel_step.value() as i32;
        config.free_mode.wheel.step_size_large = self.wheel_step_large.value() as i32;

        config.keybindings.overlay_layer = self.overlay_layer.text().to_string();
        config.keybindings.free_mode_layer = self.free_mode_layer.text().to_string();
        config.keybindings.kanata_free_mode_layer = self.kanata_free_layer.text().to_string();
        config.keybindings.exit_key = self.exit_key.text().to_string();
        config.keybindings.action_key = self.action_key.text().to_string();
        config.keybindings.monitor_switch_key = self.monitor_key.text().to_string();
        config.keybindings.drag_modifier = self.drag_modifier.text().to_string();
        config
    }

    /// Behavior section only (used by preset save/merge).
    pub fn to_behavior(&self) -> BehaviorConfig {
        BehaviorConfig {
            initial_action_location: match self.initial_location.selected() {
                1 => InitialActionLocation::SystemCursor,
                _ => InitialActionLocation::ScreenCenter,
            },
            grid_action_level: match self.grid_level.selected() {
                1 => GridActionLevel::Two,
                _ => GridActionLevel::Subgrid,
            },
            default_action: match self.default_action.selected() {
                1 => DefaultAction::Move,
                _ => DefaultAction::Click,
            },
            free_mode_after_move: self.free_mode_after_move.is_active(),
            multi_click_threshold_ms: self.multi_click.value().max(0.0) as u64,
            tap_threshold_ms: self.tap.value().max(0.0) as u64,
            nudges_per_cell: self.nudges.value().max(1.0) as u32,
            hold_subgrid_for_nudge: self.hold_subgrid.is_active(),
            continuous_mode: self.continuous.is_active(),
            toggle_action_modifier: self.toggle_modifier.text().to_string(),
            right_click_modifier: self.right_click_modifier.text().to_string(),
            hold_for_drag: self.hold_drag.is_active(),
        }
    }

    /// Update the live key-pool hints.
    pub fn refresh_hints(&self) {
        self.columns
            .set_hint(&model::columns_hint(&self.columns.text()));
        self.rows.set_hint(&model::rows_hint(&self.rows.text()));
        self.subgrid_keys.set_hint(&model::subgrid_hint(
            &self.subgrid_keys.text(),
            self.subgrid_columns.value() as u32,
            self.subgrid_rows.value() as u32,
        ));
    }

    /// Push the current appearance/keys into the Cairo preview.
    pub fn refresh_preview(&self) {
        self.preview.update(
            &color_hex(&self.grid_color),
            self.grid_opacity.value() / 100.0,
            &color_hex(&self.label_color),
            &color_hex(&self.highlight_color),
            &self.subgrid_keys.text(),
            (
                self.subgrid_columns.value() as usize,
                self.subgrid_rows.value() as usize,
            ),
        );
    }

    fn update_opacity_subtitle(&self) {
        let value = self.grid_opacity.value().clamp(0.0, 100.0);
        self.grid_opacity_row.set_subtitle(&format!(
            "{value:.0}% opacity ({:.0}% transparent)",
            100.0 - value
        ));
    }

    /// Connect every editable widget to the shared dirty/preview state.
    pub fn connect(&self, state: &Rc<AppState>) {
        let weak = Rc::downgrade(state);
        let changed = {
            let weak = weak.clone();
            move || {
                if let Some(state) = weak.upgrade() {
                    state.changed();
                }
            }
        };

        for entry in [&self.columns.entry, &self.rows.entry] {
            let weak = weak.clone();
            entry.connect_changed(move |_| {
                if let Some(state) = weak.upgrade() {
                    state.form.refresh_hints();
                    state.changed();
                }
            });
        }
        {
            let changed = changed.clone();
            self.custom_monitors
                .entry
                .connect_changed(move |_| changed());
        }
        {
            let weak = weak.clone();
            self.subgrid_keys.entry.connect_changed(move |_| {
                if let Some(state) = weak.upgrade() {
                    state.changed();
                    state.form.refresh_hints();
                    state.form.refresh_preview();
                }
            });
        }
        for entry in [
            &self.overlay_layer,
            &self.free_mode_layer,
            &self.kanata_free_layer,
            &self.exit_key,
            &self.action_key,
            &self.monitor_key,
            &self.drag_modifier,
            &self.toggle_modifier,
            &self.right_click_modifier,
        ] {
            let changed = changed.clone();
            entry.connect_changed(move |_| changed());
        }

        for spin in [
            &self.subgrid_columns,
            &self.subgrid_rows,
            &self.multi_click,
            &self.tap,
            &self.nudges,
            &self.base_speed,
            &self.speed_multiplier,
            &self.auto_off,
            &self.wheel_base,
            &self.wheel_multiplier,
            &self.wheel_step,
            &self.wheel_step_large,
        ] {
            let weak = weak.clone();
            spin.connect_value_notify(move |_| {
                if let Some(state) = weak.upgrade() {
                    // Subgrid shape changes the expected key count and preview.
                    state.form.refresh_hints();
                    state.form.refresh_preview();
                    state.changed();
                }
            });
        }

        for switch in [
            &self.hide_cursor,
            &self.free_mode_after_move,
            &self.hold_subgrid,
            &self.continuous,
            &self.hold_drag,
        ] {
            let changed = changed.clone();
            switch.connect_active_notify(move |_| changed());
        }

        for combo in [
            &self.cursor_corner,
            &self.initial_location,
            &self.grid_level,
            &self.default_action,
            &self.engine,
            &self.easing,
            &self.wheel_easing,
        ] {
            let changed = changed.clone();
            combo.connect_selected_notify(move |_| changed());
        }

        let monitor_weak = weak.clone();
        self.monitor_assignment
            .connect_selected_notify(move |combo| {
                if let Some(state) = monitor_weak.upgrade() {
                    state
                        .form
                        .custom_monitors
                        .row
                        .set_visible(combo.selected() == 2);
                    state.changed();
                }
            });

        let theme_weak = weak.clone();
        self.theme.connect_selected_notify(move |_| {
            if let Some(state) = theme_weak.upgrade() {
                state.form.load_appearance_colors(&state.loaded.borrow());
                state.form.refresh_preview();
                state.changed();
            }
        });

        for button in [&self.grid_color, &self.label_color, &self.highlight_color] {
            let weak = weak.clone();
            button.connect_color_set(move |_| {
                if let Some(state) = weak.upgrade() {
                    state.form.refresh_preview();
                    state.changed();
                }
            });
        }

        self.grid_opacity.connect_value_changed(move |_| {
            if let Some(state) = weak.upgrade() {
                state.form.update_opacity_subtitle();
                state.form.refresh_preview();
                state.changed();
            }
        });
    }
}

/// Simple `AdwEntryRow`.
pub fn entry_row(title: &str) -> adw::EntryRow {
    adw::EntryRow::builder().title(title).build()
}

/// `AdwSwitchRow`.
pub fn switch_row(title: &str, subtitle: &str, active: bool) -> adw::SwitchRow {
    adw::SwitchRow::builder()
        .title(title)
        .subtitle(subtitle)
        .active(active)
        .build()
}

/// `AdwSpinRow` with a range and digits.
pub fn spin_row(
    title: &str,
    min: f64,
    max: f64,
    step: f64,
    digits: u32,
    value: f64,
) -> adw::SpinRow {
    let row = adw::SpinRow::with_range(min, max, step);
    row.set_title(title);
    row.set_digits(digits);
    row.set_numeric(true);
    row.set_value(value);
    row
}

/// `AdwComboRow` over display strings.
pub fn combo_row(title: &str, options: &[&str], selected: u32) -> adw::ComboRow {
    let row = adw::ComboRow::new();
    row.set_title(title);
    row.set_model(Some(&gtk::StringList::new(options)));
    row.set_selected(selected);
    row
}

/// Build a colour picker with a schema hex default.
pub fn color_button(hex: &str) -> gtk::ColorButton {
    let button = gtk::ColorButton::new();
    button.set_use_alpha(false);
    button.set_valign(gtk::Align::Center);
    set_color(&button, hex);
    button
}

/// Row containing a colour picker suffix.
pub fn color_row(title: &str, button: &gtk::ColorButton) -> adw::ActionRow {
    let row = adw::ActionRow::builder().title(title).build();
    row.add_suffix(button);
    row.set_activatable_widget(Some(button));
    row
}

fn set_color(button: &gtk::ColorButton, hex: &str) {
    let [red, green, blue] = model::rgb_channels(hex);
    button.set_rgba(&gtk::gdk::RGBA::new(
        red as f32,
        green as f32,
        blue as f32,
        1.0,
    ));
}

fn color_hex(button: &gtk::ColorButton) -> String {
    let rgba = button.rgba();
    model::hex_from_channels([
        f64::from(rgba.red()),
        f64::from(rgba.green()),
        f64::from(rgba.blue()),
    ])
}

fn monitor_index(value: MonitorAssignment) -> u32 {
    match value {
        MonitorAssignment::Auto => 0,
        MonitorAssignment::Single => 1,
        MonitorAssignment::Custom => 2,
    }
}

fn corner_index(value: CursorCorner) -> u32 {
    match value {
        CursorCorner::TopLeft => 0,
        CursorCorner::TopRight => 1,
        CursorCorner::BottomLeft => 2,
        CursorCorner::BottomRight => 3,
    }
}

fn easing_index(value: Easing) -> u32 {
    match value {
        Easing::Loose => 0,
        Easing::Natural => 1,
        Easing::Snappy => 2,
        Easing::Instant => 3,
    }
}

fn easing_from_index(value: u32) -> Easing {
    match value {
        0 => Easing::Loose,
        2 => Easing::Snappy,
        3 => Easing::Instant,
        _ => Easing::Natural,
    }
}

/// Pool documented on the Grid page.
pub fn pool_labels() -> (&'static str, &'static str) {
    ("any unique ASCII keys (5–100, no esc/space)", ROW_KEY_POOL)
}
