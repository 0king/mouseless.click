//! Preferences pages.
//!
//! Each page is a thin arrangement of Adwaita rows over [`crate::form::Form`].
//! The only page that owns extra state is Status (live labels) and Presets
//! (dynamically rebuilt rows).

use crate::form;
use crate::state::{choose_file, AppState};
use adw::prelude::*;
use gtk::prelude::*;
use libadwaita as adw;
use mouseless_config::{BehaviorConfig, DefaultAction, GridActionLevel, Preset};
use std::rc::Rc;

/// Live labels on the Status page.
#[derive(Clone)]
pub struct StatusWidgets {
    /// Daemon reachability/version.
    pub daemon: adw::ActionRow,
    /// Overlay connection.
    pub overlay: adw::ActionRow,
    /// Session state.
    pub session: adw::ActionRow,
    /// Free-mode state.
    pub free_mode: adw::ActionRow,
    /// Daemon-owned config path.
    pub config_path: adw::ActionRow,
    /// Kanata layer/TCP state.
    pub kanata: adw::ActionRow,
    /// `/dev/uinput` presence.
    pub uinput: adw::ActionRow,
    /// Config origin.
    pub source: adw::ActionRow,
    /// Startup fallback warning.
    pub warning: adw::ActionRow,
}

/// Build and add every page to the window.
pub fn build(state: &Rc<AppState>) {
    state.window.add(&grid_page(state));
    state.window.add(&appearance_page(state));
    state.window.add(&behavior_page(state));
    state.window.add(&free_mode_page(state));
    state.window.add(&keybindings_page(state));
    state.window.add(&presets_page(state));
    state.window.add(&status_page(state));
}

fn page(name: &str, title: &str, icon: &str) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();
    page.set_name(Some(name));
    page.set_title(title);
    page.set_icon_name(Some(icon));
    page
}

fn group(title: &str, description: &str) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::new();
    group.set_title(title);
    if !description.is_empty() {
        group.set_description(Some(description));
    }
    group
}

fn row(title: &str, subtitle: &str) -> adw::ActionRow {
    adw::ActionRow::builder()
        .title(title)
        .subtitle(subtitle)
        .build()
}

/// Save status and actions duplicated at the bottom of every page, because
/// `AdwPreferencesWindow` has no public footer slot. Edits auto-save; the
/// suggested button deliberately applies them to the running app.
fn action_group(state: &Rc<AppState>) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::new();
    let label = gtk::Label::new(Some("Saved"));
    label.add_css_class("dim-label");
    label.set_halign(gtk::Align::Start);
    label.set_hexpand(true);
    label.set_xalign(0.0);

    let revert = gtk::Button::with_label("Revert");
    revert.set_valign(gtk::Align::Center);
    let restart = gtk::Button::with_label("Restart App");
    restart.add_css_class("suggested-action");
    restart.set_valign(gtk::Align::Center);
    restart.set_tooltip_text(Some(
        "Save the current values and apply them to the running app (Ctrl+Enter)",
    ));

    let weak = Rc::downgrade(state);
    revert.connect_clicked(move |_| {
        if let Some(state) = weak.upgrade() {
            state.revert();
        }
    });
    let weak = Rc::downgrade(state);
    restart.connect_clicked(move |_| {
        if let Some(state) = weak.upgrade() {
            state.restart_app();
        }
    });

    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    buttons.append(&label);
    buttons.append(&revert);
    buttons.append(&restart);
    buttons.set_margin_top(4);
    buttons.set_margin_bottom(4);
    group.add(&buttons);

    state.action_labels.borrow_mut().push(label);
    state.revert_buttons.borrow_mut().push(revert);
    state.restart_buttons.borrow_mut().push(restart);
    group
}

fn grid_page(state: &Rc<AppState>) -> adw::PreferencesPage {
    let page = page("grid", "Grid", "view-grid-symbolic");
    let form = &state.form;

    let keys = group(
        "Cell keys",
        &format!(
            "Level 1 picks a column from “{}”; level 2 picks a row from “{}”. \
             The selected cell opens a subgrid. Duplicate keys are rejected \
             before saving.",
            form::pool_labels().0,
            form::pool_labels().1
        ),
    );
    keys.add(&form.columns.row);
    keys.add(&form.rows.row);
    keys.add(&form.subgrid_columns);
    keys.add(&form.subgrid_rows);
    keys.add(&form.subgrid_keys.row);
    page.add(&keys);

    let monitors = group(
        "Monitor",
        "Automatic follows the cursor monitor and falls back to the primary. \
         Custom picks the first present connector from the list.",
    );
    monitors.add(&form.monitor_assignment);
    monitors.add(&form.custom_monitors.row);
    page.add(&monitors);

    page.add(&action_group(state));
    page
}

fn appearance_page(state: &Rc<AppState>) -> adw::PreferencesPage {
    let page = page("appearance", "Appearance", "applications-graphics-symbolic");
    let form = &state.form;

    let colors = group(
        "Colors",
        "Opacity is applied to the grid fill; labels and the highlight stay opaque.",
    );
    colors.add(&form.theme);
    colors.add(&form::color_row("Grid fill", &form.grid_color));
    form.grid_opacity.set_hexpand(true);
    form.grid_opacity.set_size_request(220, -1);
    form.grid_opacity.set_draw_value(true);
    form.grid_opacity.set_valign(gtk::Align::Center);
    form.grid_opacity_row.add_suffix(&form.grid_opacity);
    colors.add(&form.grid_opacity_row);
    colors.add(&form::color_row("Cell labels", &form.label_color));
    colors.add(&form::color_row("Highlight", &form.highlight_color));
    page.add(&colors);

    let preview = group(
        "Subgrid preview",
        "Live preview of the selected primary cell using the values above.",
    );
    preview.add(&form.preview.area);
    page.add(&preview);

    let cursor = group("Cursor", "");
    cursor.add(&form.hide_cursor);
    cursor.add(&form.cursor_corner);
    page.add(&cursor);

    page.add(&action_group(state));
    page
}

fn behavior_page(state: &Rc<AppState>) -> adw::PreferencesPage {
    let page = page("behavior", "Behavior", "preferences-system-symbolic");
    let form = &state.form;

    let selection = group("Selection", "");
    selection.add(&form.initial_location);
    selection.add(&form.grid_level);
    selection.add(&form.default_action);
    selection.add(&form.free_mode_after_move);
    page.add(&selection);

    let timing = group(
        "Timing and nudging",
        "The tap window is part of the kanata include. Values are saved \
         automatically; Restart App runs `kanata --check` when the binary and \
         /etc/kanata/unicfg.kbd are present.",
    );
    timing.add(&form.multi_click);
    timing.add(&form.tap);
    timing.add(&form.nudges);
    timing.add(&form.hold_subgrid);
    timing.add(&form.continuous);
    page.add(&timing);

    let modifiers = group("Modifiers", "");
    modifiers.add(&form.toggle_modifier);
    modifiers.add(&form.right_click_modifier);
    modifiers.add(&form.hold_drag);
    page.add(&modifiers);

    page.add(&action_group(state));
    page
}

fn free_mode_page(state: &Rc<AppState>) -> adw::PreferencesPage {
    let page = page("free-mode", "Free Mode", "input-mouse-symbolic");
    let form = &state.form;

    let movement = group(
        "Movement",
        "Custom physics runs in the daemon. The kanata engine hands movement to \
         `lyr-mouse`; the other values remain stored so switching back is lossless.",
    );
    movement.add(&form.engine);
    movement.add(&form.base_speed);
    movement.add(&form.speed_multiplier);
    movement.add(&form.easing);
    movement.add(&form.auto_off);
    page.add(&movement);

    let wheel = group("Wheel", "");
    wheel.add(&form.wheel_base);
    wheel.add(&form.wheel_multiplier);
    wheel.add(&form.wheel_easing);
    wheel.add(&form.wheel_step);
    wheel.add(&form.wheel_step_large);
    page.add(&wheel);

    page.add(&action_group(state));
    page
}

fn keybindings_page(state: &Rc<AppState>) -> adw::PreferencesPage {
    let page = page(
        "keybindings",
        "Keybindings",
        "preferences-desktop-keyboard-symbolic",
    );
    let form = &state.form;

    let editable = group(
        "Layers and key roles",
        "Layer names must match the generated kanata include. Changes are saved \
         automatically and applied by Restart App, but the include still has to \
         be installed by root (OQ-3); use “Save include…” below.",
    );
    editable.add(&form.overlay_layer);
    editable.add(&form.free_mode_layer);
    editable.add(&form.kanata_free_layer);
    editable.add(&form.exit_key);
    editable.add(&form.action_key);
    editable.add(&form.monitor_key);
    editable.add(&form.drag_modifier);
    page.add(&editable);

    let reference = group(
        "Reference",
        "Reserved action keys are not assignable to grid cells; cell-key conflicts \
         are duplicate or out-of-pool characters, shown live on the Grid page.",
    );
    reference.add(&row(
        "Overlay trigger",
        "Hold the overlay-trigger key; a tap keeps its normal key (kanata tap-hold).",
    ));
    reference.add(&row(
        "Free-mode trigger",
        "Hold the free-mode-trigger key; a tap keeps its normal key.",
    ));
    reference.add(&row("Exit", "Esc returns to the base layer."));
    reference.add(&row(
        "Action",
        "Space acts at the initial action location before a cell is selected.",
    ));
    reference.add(&row(
        "Monitor switch",
        "Shift moves the overlay to the next monitor.",
    ));
    reference.add(&row(
        "Drag",
        "Hold Alt on the final key to begin or continue a drag.",
    ));
    reference.add(&row(
        "Subgrid keys",
        "qwertyuiop / asdfghjkl; / zxcvbnm,./ (row-major)",
    ));
    reference.add(&row(
        "Kill switch",
        "LCtrl+Space+Esc — kanata and the daemon co-exit; systemd restarts the stack.",
    ));
    page.add(&reference);

    let include = group(
        "Kanata include",
        "The daemon cannot write /etc. Save the generated fragment, then install it \
         with: sudo install -m 0644 mouseless.kbd /etc/kanata/mouseless.kbd",
    );
    let export = gtk::Button::with_label("Save include…");
    export.add_css_class("pill");
    export.set_halign(gtk::Align::Start);
    let weak = Rc::downgrade(state);
    export.connect_clicked(move |_| {
        if let Some(state) = weak.upgrade() {
            let state_for_file = state.clone();
            choose_file(
                &state.window,
                "Save kanata include",
                gtk::FileChooserAction::Save,
                Some("mouseless.kbd"),
                move |path| state_for_file.export_include(path),
            );
        }
    });
    include.add(&export);
    page.add(&include);

    page.add(&action_group(state));
    page
}

fn presets_page(state: &Rc<AppState>) -> adw::PreferencesPage {
    let page = page("presets", "Presets", "document-save-symbolic");

    let save = group(
        "Save current behavior",
        "Presets store the complete behavior section (selection, timing, modifiers, \
         and free-mode-after-move). Grid, appearance, and free-mode physics stay as-is.",
    );
    let save_button = gtk::Button::with_label("Save preset");
    save_button.add_css_class("pill");
    save_button.set_valign(gtk::Align::Center);
    let weak = Rc::downgrade(state);
    save_button.connect_clicked(move |_| {
        if let Some(state) = weak.upgrade() {
            state.save_current_preset();
        }
    });
    state.form.preset_name.add_suffix(&save_button);
    save.add(&state.form.preset_name);
    page.add(&save);

    let available = group(
        "Available presets",
        "Built-ins are read-only. User presets are YAML files in ~/.config/mouseless/presets.",
    );
    *state.presets_group.borrow_mut() = Some(available.clone());
    page.add(&available);

    let transfer = group(
        "Import",
        "Imported presets are validated and copied into the user preset directory.",
    );
    let import = gtk::Button::with_label("Import preset…");
    import.add_css_class("pill");
    import.set_halign(gtk::Align::Start);
    let weak = Rc::downgrade(state);
    import.connect_clicked(move |_| {
        if let Some(state) = weak.upgrade() {
            let state_for_file = state.clone();
            choose_file(
                &state.window,
                "Import preset",
                gtk::FileChooserAction::Open,
                None,
                move |path| state_for_file.import_preset(path),
            );
        }
    });
    transfer.add(&import);
    page.add(&transfer);

    page.add(&action_group(state));
    page
}

fn status_page(state: &Rc<AppState>) -> adw::PreferencesPage {
    let page = page("status", "Status", "dialog-information-symbolic");

    let daemon = group(
        "Daemon",
        "Read-only status from the same socket as `mouseless status`.",
    );
    let daemon_row = row("Daemon", "querying…");
    let overlay = row("Overlay", "querying…");
    let session = row("Session", "querying…");
    let free_mode = row("Free mode", "querying…");
    let config_path = row("Config file", "querying…");
    daemon.add(&daemon_row);
    daemon.add(&overlay);
    daemon.add(&session);
    daemon.add(&free_mode);
    daemon.add(&config_path);
    page.add(&daemon);

    let input = group("Input path", "");
    let kanata = row("Kanata", "querying…");
    let uinput = row("uinput", "querying…");
    input.add(&kanata);
    input.add(&uinput);
    page.add(&input);

    let source = group(
        "Configuration source",
        "Edits are saved automatically to the daemon-owned document and do not \
         change the running app until you press Restart App. When the daemon is \
         down the app falls back to the last persisted copy and cannot save.",
    );
    let source_row = row("Loaded from", &state.config_source.borrow());
    let warning = row(
        "Fallback warning",
        state.config_warning.borrow().as_deref().unwrap_or("none"),
    );
    warning.set_visible(state.config_warning.borrow().is_some());
    source.add(&source_row);
    source.add(&warning);
    page.add(&source);

    let application = group("Application", "Version of this settings front end.");
    let app_version = row(
        "Mouseless",
        &format!("version {}", env!("CARGO_PKG_VERSION")),
    );
    application.add(&app_version);
    let about = gtk::Button::with_label("About Mouseless");
    about.add_css_class("pill");
    about.set_halign(gtk::Align::Start);
    let weak = Rc::downgrade(state);
    about.connect_clicked(move |_| {
        if let Some(state) = weak.upgrade() {
            show_about(&state.window);
        }
    });
    application.add(&about);
    page.add(&application);

    let actions = group("Actions", "");
    let refresh = gtk::Button::with_label("Refresh status");
    refresh.add_css_class("pill");
    refresh.set_halign(gtk::Align::Start);
    let weak = Rc::downgrade(state);
    refresh.connect_clicked(move |_| {
        if let Some(state) = weak.upgrade() {
            state.refresh_status();
        }
    });
    actions.add(&refresh);
    page.add(&actions);

    *state.status_widgets.borrow_mut() = Some(StatusWidgets {
        daemon: daemon_row,
        overlay,
        session,
        free_mode,
        config_path,
        kanata,
        uinput,
        source: source_row,
        warning,
    });

    page.add(&action_group(state));
    page
}

/// One row in the Available presets group.
pub fn preset_row(state: &Rc<AppState>, preset: &Preset, builtin: bool) -> adw::ActionRow {
    let subtitle = format!(
        "{} · {}",
        if builtin { "Built-in" } else { "User preset" },
        behavior_summary(&preset.behavior)
    );
    let row = row(&preset.name, &subtitle);

    let apply = gtk::Button::with_label("Use");
    apply.set_valign(gtk::Align::Center);
    apply.add_css_class("flat");
    let weak = Rc::downgrade(state);
    let preset_for_apply = preset.clone();
    apply.connect_clicked(move |_| {
        if let Some(state) = weak.upgrade() {
            state.apply_preset(&preset_for_apply);
        }
    });

    let export = gtk::Button::with_label("Export…");
    export.set_valign(gtk::Align::Center);
    export.add_css_class("flat");
    let weak = Rc::downgrade(state);
    let preset_for_export = preset.clone();
    let filename = format!("{}.yaml", preset.slug());
    export.connect_clicked(move |_| {
        if let Some(state) = weak.upgrade() {
            let preset = preset_for_export.clone();
            let state_for_file = state.clone();
            choose_file(
                &state.window,
                "Export preset",
                gtk::FileChooserAction::Save,
                Some(&filename),
                move |path| state_for_file.export_preset(&preset, path),
            );
        }
    });

    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    buttons.append(&apply);
    buttons.append(&export);
    row.add_suffix(&buttons);
    row
}

/// Standard Adwaita About dialog with the project version.
fn show_about(parent: &impl IsA<gtk::Widget>) {
    let dialog = adw::AboutDialog::builder()
        .application_name("Mouseless")
        .application_icon("input-mouse-symbolic")
        .version(env!("CARGO_PKG_VERSION"))
        .developer_name("0king")
        .developers(vec!["0king"])
        .website("https://mouseless.click")
        .copyright("© 2026 0king")
        .license_type(gtk::License::MitX11)
        .build();
    dialog.present(Some(parent));
}

fn behavior_summary(behavior: &BehaviorConfig) -> String {
    let level = match behavior.grid_action_level {
        GridActionLevel::Subgrid => "subgrid",
        GridActionLevel::Two => "2-key",
    };
    let action = match behavior.default_action {
        DefaultAction::Click => "click",
        DefaultAction::Move => "move",
    };
    format!(
        "{level} / {action} / free mode after move: {}",
        if behavior.free_mode_after_move {
            "on"
        } else {
            "off"
        }
    )
}
