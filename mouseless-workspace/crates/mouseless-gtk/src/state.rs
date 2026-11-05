//! Application state and the auto-save/restart lifecycle.
//!
//! Every edit is validated and persisted automatically with the daemon's
//! `Save` message. `Save` does not touch the running app, so the user can
//! experiment safely; the “Restart App” button sends `Update`, which persists
//! and applies the current draft to the live daemon.

use crate::form::Form;
use crate::model;
use crate::pages::{self, StatusWidgets};
use adw::prelude::*;
use gtk::glib;
use gtk::prelude::*;
use libadwaita as adw;
use mouseless_cli::SettingsPage;
use mouseless_config::{validate, Config, ConfigError, ConfigStore, Preset};
use mouseless_ipc::{request, ClientToDaemon, DaemonToClient};
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::{Rc, Weak};
use std::sync::mpsc::{self, TryRecvError};
use std::time::Duration;

/// Loaded baseline and where it came from.
struct Loaded {
    config: Config,
    source: String,
    warning: Option<String>,
}

/// Result of one background auto-save attempt.
enum SaveOutcome {
    Saved { config: Box<Config> },
    CheckRejected(String),
    DaemonError { code: String, message: String },
    Unreachable(String),
}

/// Result of one background restart (persist + apply) attempt.
enum RestartOutcome {
    Restarted {
        config: Box<Config>,
        message: String,
        warnings: Vec<String>,
    },
    CheckRejected(String),
    DaemonError {
        code: String,
        message: String,
    },
    Unreachable(String),
}

/// Widgets and shared state for the whole settings window.
pub struct AppState {
    /// Preferences window.
    pub window: adw::PreferencesWindow,
    /// Editable form.
    pub form: Form,
    /// Last saved / loaded baseline.
    pub loaded: RefCell<Config>,
    /// Whether the form differs from the last persisted baseline.
    pub dirty: Cell<bool>,
    /// Suppress change handlers while loading widgets.
    pub loading: Cell<bool>,
    /// A save or restart is in flight.
    pub busy: Cell<bool>,
    /// The in-flight operation is a restart (affects the footer label).
    pub restarting: Cell<bool>,
    /// Whether the running app is known to use the saved values.
    pub live_in_sync: Cell<bool>,
    /// Debounced auto-save source.
    autosave_source: RefCell<Option<glib::SourceId>>,
    /// A change arrived while a save was in flight; save again afterwards.
    resave_pending: Cell<bool>,
    /// Last auto-save failure shown inline on every page.
    save_error: RefCell<Option<String>>,
    /// Last error toast, so repeated failures do not spam the window.
    last_error_toast: RefCell<Option<String>>,
    /// User preset directory (`~/.config/mouseless/presets`).
    pub presets_dir: PathBuf,
    /// Preset rows container (installed by the Presets page).
    pub presets_group: RefCell<Option<adw::PreferencesGroup>>,
    /// Rows currently shown in [`Self::presets_group`], so a refresh removes
    /// only the rows we added instead of the group's internal container.
    pub preset_rows: RefCell<Vec<adw::ActionRow>>,
    /// Save/restart status labels on each page's action group.
    pub action_labels: RefCell<Vec<gtk::Label>>,
    /// Restart buttons on each page.
    pub restart_buttons: RefCell<Vec<gtk::Button>>,
    /// Revert buttons on each page.
    pub revert_buttons: RefCell<Vec<gtk::Button>>,
    /// Status page widgets (installed by the Status page).
    pub status_widgets: RefCell<Option<StatusWidgets>>,
    /// Config origin shown on the Status page.
    pub config_source: RefCell<String>,
    /// Startup warning shown on the Status page.
    pub config_warning: RefCell<Option<String>>,
    /// Weak self-handle for callbacks that must not keep the window alive.
    self_ref: Weak<AppState>,
    /// Monotonic status request id (drop stale replies).
    status_generation: Cell<u64>,
}

impl AppState {
    /// Build the window, form, pages, and initial state.
    pub fn build(app: &adw::Application, page: SettingsPage) -> Rc<Self> {
        let loaded = load_config();
        let window = adw::PreferencesWindow::builder()
            .title("Mouseless")
            .application(app)
            .default_width(940)
            .default_height(760)
            .search_enabled(true)
            .build();
        let form = Form::new(&loaded.config);
        let presets_dir = preset_dir();
        let state = Rc::new_cyclic(|weak| Self {
            window,
            form,
            loaded: RefCell::new(loaded.config),
            dirty: Cell::new(false),
            loading: Cell::new(false),
            busy: Cell::new(false),
            restarting: Cell::new(false),
            live_in_sync: Cell::new(false),
            autosave_source: RefCell::new(None),
            resave_pending: Cell::new(false),
            save_error: RefCell::new(None),
            last_error_toast: RefCell::new(None),
            presets_dir,
            presets_group: RefCell::new(None),
            preset_rows: RefCell::new(Vec::new()),
            action_labels: RefCell::new(Vec::new()),
            restart_buttons: RefCell::new(Vec::new()),
            revert_buttons: RefCell::new(Vec::new()),
            status_widgets: RefCell::new(None),
            config_source: RefCell::new(loaded.source),
            config_warning: RefCell::new(loaded.warning),
            self_ref: weak.clone(),
            status_generation: Cell::new(0),
        });

        state.form.connect(&state);
        pages::build(&state);
        state.window.set_visible_page_name(page.name());
        state.update_actions();

        // Ctrl+Enter restarts the app from any page; the buttons remain the
        // discoverable path. A single global controller avoids one handler
        // per page.
        let controller = gtk::ShortcutController::new();
        controller.set_scope(gtk::ShortcutScope::Global);
        let action = gtk::CallbackAction::new({
            let weak = state.self_ref.clone();
            move |_, _| {
                if let Some(state) = weak.upgrade() {
                    state.restart_app();
                }
                glib::Propagation::Stop
            }
        });
        for accel in ["<Control>Return", "<Control>KP_Enter"] {
            if let Some(trigger) = gtk::ShortcutTrigger::parse_string(accel) {
                controller.add_shortcut(gtk::Shortcut::new(Some(trigger), Some(action.clone())));
            }
        }
        state.window.add_controller(controller);

        state.refresh_presets();

        let weak = Rc::downgrade(&state);
        state.window.connect_close_request(move |_| {
            if let Some(state) = weak.upgrade() {
                state.flush_autosave();
            }
            glib::Propagation::Proceed
        });

        let weak = Rc::downgrade(&state);
        state
            .window
            .connect_visible_page_name_notify(move |window| {
                if window.visible_page_name().as_deref() == Some("status") {
                    if let Some(state) = weak.upgrade() {
                        state.refresh_status();
                    }
                }
            });

        state.window.present();
        if let Some(warning) = state.config_warning.borrow().clone() {
            state.toast(&warning);
        }
        state.refresh_status();
        state
    }

    /// Called by every form signal; ignored while widgets are being loaded.
    pub fn changed(&self) {
        if self.loading.get() {
            return;
        }
        self.dirty.set(true);
        self.live_in_sync.set(false);
        self.save_error.replace(None);
        self.update_actions();
        self.schedule_autosave();
    }

    /// Mark dirty after a programmatic draft change (preset apply).
    pub fn mark_dirty(&self) {
        self.changed();
    }

    /// Update each page's status label and action buttons.
    pub fn update_actions(&self) {
        let label = if self.busy.get() {
            if self.restarting.get() {
                "Restarting app…".to_string()
            } else {
                "Saving…".to_string()
            }
        } else if let Some(error) = self.save_error.borrow().as_deref() {
            format!("Not saved — {error}")
        } else if self.dirty.get() {
            "Saving…".to_string()
        } else if self.live_in_sync.get() {
            "Saved and applied".to_string()
        } else {
            "Saved".to_string()
        };
        for label_widget in self.action_labels.borrow().iter() {
            label_widget.set_text(&label);
        }
        for button in self.restart_buttons.borrow().iter() {
            button.set_sensitive(!self.busy.get());
        }
        let can_revert =
            !self.busy.get() && (self.dirty.get() || self.save_error.borrow().is_some());
        for button in self.revert_buttons.borrow().iter() {
            button.set_sensitive(can_revert);
        }
    }

    /// Show a transient message on the window.
    pub fn toast(&self, message: &str) {
        self.window.add_toast(adw::Toast::new(message));
    }

    /// Cancel a pending debounced save.
    fn cancel_autosave(&self) {
        if let Some(source) = self.autosave_source.borrow_mut().take() {
            source.remove();
        }
    }

    /// Persist the latest draft synchronously when the window closes, so the
    /// debounce cannot drop the last edit. This still runs while another save
    /// is in flight; both requests carry full documents, and the newest one is
    /// sent last. Invalid drafts are left unsaved rather than blocking the
    /// close.
    fn flush_autosave(&self) {
        self.cancel_autosave();
        if !self.dirty.get() {
            return;
        }
        let candidate = self.form.to_config(&self.loaded.borrow());
        if validate(&candidate).is_err() {
            return;
        }
        let previous = self.loaded.borrow().clone();
        if let SaveOutcome::Saved { config } =
            attempt_save(&model::socket_path(), &previous, &candidate)
        {
            *self.loaded.borrow_mut() = *config;
            self.dirty.set(false);
        }
    }

    /// Debounce form changes so typing does not hit the daemon per keystroke.
    fn schedule_autosave(&self) {
        self.cancel_autosave();
        let weak = self.self_ref.clone();
        let source = glib::timeout_add_local(Duration::from_millis(350), move || {
            if let Some(state) = weak.upgrade() {
                state.autosave_source.borrow_mut().take();
                state.save_now();
            }
            glib::ControlFlow::Break
        });
        *self.autosave_source.borrow_mut() = Some(source);
    }

    /// Validate the current draft and persist it without touching the running
    /// app. Invalid drafts stay dirty and show the first issue inline.
    pub fn save_now(&self) {
        if self.loading.get() {
            return;
        }
        let candidate = self.form.to_config(&self.loaded.borrow());
        if let Err(error) = validate(&candidate) {
            self.save_error.replace(Some(summarize_issues(&error)));
            self.dirty.set(true);
            self.update_actions();
            return;
        }
        if self.busy.get() {
            self.resave_pending.set(true);
            return;
        }
        self.save_error.replace(None);
        self.start_save(candidate);
    }

    fn start_save(&self, candidate: Config) {
        let previous = self.loaded.borrow().clone();
        let socket = model::socket_path();
        self.busy.set(true);
        self.restarting.set(false);
        self.update_actions();

        let weak = self.self_ref.clone();
        spawn_blocking(
            move || attempt_save(&socket, &previous, &candidate),
            move |outcome| {
                if let Some(state) = weak.upgrade() {
                    state.finish_save(outcome);
                }
            },
        );
    }

    fn finish_save(&self, outcome: SaveOutcome) {
        self.busy.set(false);
        match outcome {
            SaveOutcome::Saved { config, .. } => {
                *self.loaded.borrow_mut() = *config;
                let current = self.form.to_config(&self.loaded.borrow());
                let unchanged = current == *self.loaded.borrow();
                self.dirty.set(!unchanged);
                if !unchanged {
                    self.resave_pending.set(true);
                }
                self.save_error.replace(None);
                self.last_error_toast.replace(None);
                self.config_source.replace("saved daemon config".into());
                self.config_warning.replace(None);
                self.refresh_status();
            }
            SaveOutcome::CheckRejected(message) => {
                self.save_error
                    .replace(Some("kanata rejected the generated include".into()));
                self.dirty.set(true);
                self.notify_error_once(&format!(
                    "Not saved: kanata rejected the generated include — {message}"
                ));
            }
            SaveOutcome::DaemonError { code, message } => {
                let error = format!("{code}: {message}");
                self.save_error.replace(Some(error.clone()));
                self.dirty.set(true);
                self.notify_error_once(&format!("Not saved: {error}"));
            }
            SaveOutcome::Unreachable(message) => {
                self.save_error.replace(Some("daemon unreachable".into()));
                self.dirty.set(true);
                self.notify_error_once(&format!("Not saved: {message}"));
            }
        }
        if self.resave_pending.replace(false) {
            self.schedule_autosave();
        }
        self.update_actions();
    }

    fn notify_error_once(&self, message: &str) {
        let mut last = self.last_error_toast.borrow_mut();
        if last.as_deref() != Some(message) {
            *last = Some(message.to_string());
            self.toast(message);
        }
    }

    /// Save the current values and apply them to the running app.
    pub fn restart_app(&self) {
        if self.busy.get() {
            return;
        }
        self.cancel_autosave();
        let candidate = self.form.to_config(&self.loaded.borrow());
        if let Err(error) = validate(&candidate) {
            show_dialog(
                &self.window,
                "The app was not restarted",
                &format!("Fix these fields first:\n\n{}", format_issues(&error)),
            );
            return;
        }

        let previous = self.loaded.borrow().clone();
        let socket = model::socket_path();
        self.busy.set(true);
        self.restarting.set(true);
        self.resave_pending.set(false);
        self.update_actions();
        self.toast("Saving and restarting the app…");

        let weak = self.self_ref.clone();
        spawn_blocking(
            move || attempt_restart(&socket, &previous, &candidate),
            move |outcome| {
                if let Some(state) = weak.upgrade() {
                    state.finish_restart(outcome);
                }
            },
        );
    }

    fn finish_restart(&self, outcome: RestartOutcome) {
        self.busy.set(false);
        self.restarting.set(false);
        match outcome {
            RestartOutcome::Restarted {
                config,
                message,
                warnings,
            } => {
                *self.loaded.borrow_mut() = *config;
                let current = self.form.to_config(&self.loaded.borrow());
                let unchanged = current == *self.loaded.borrow();
                self.dirty.set(!unchanged);
                self.live_in_sync.set(unchanged);
                if !unchanged {
                    self.resave_pending.set(true);
                }
                self.save_error.replace(None);
                self.last_error_toast.replace(None);
                self.config_source.replace("live daemon".into());
                self.toast(&message);
                for warning in warnings {
                    self.toast(&warning);
                }
                self.config_warning.replace(None);
                self.refresh_status();
            }
            RestartOutcome::CheckRejected(message) => {
                show_dialog(
                    &self.window,
                    "kanata rejected the generated include",
                    &format!(
                        "{message}\n\nThe app still runs the previous configuration. \
                         The saved values are kept; fix the layer names or tap timing, \
                         then restart again."
                    ),
                );
            }
            RestartOutcome::DaemonError { code, message } => {
                show_dialog(
                    &self.window,
                    "The daemon rejected the configuration",
                    &format!("{code}: {message}\n\nThe previous configuration is still live."),
                );
            }
            RestartOutcome::Unreachable(message) => {
                show_dialog(
                    &self.window,
                    "The daemon is not reachable",
                    &format!(
                        "{message}\n\nStart the daemon with \
                         `sudo systemctl start mouselessd`, then restart the app. \
                         The running configuration was not changed."
                    ),
                );
            }
        }
        if self.resave_pending.replace(false) {
            self.schedule_autosave();
        }
        self.update_actions();
    }

    /// Discard in-form changes and reload the daemon's saved document.
    pub fn revert(&self) {
        if self.busy.get() {
            return;
        }
        self.cancel_autosave();
        let saved = request(
            model::socket_path(),
            &ClientToDaemon::GetConfig,
            model::client_timeout(),
        )
        .ok()
        .and_then(|reply| model::config_from_reply(reply).ok());
        match saved {
            Some(config) => {
                self.with_loading(|state| {
                    state.form.load(&config);
                    *state.loaded.borrow_mut() = config;
                });
                self.dirty.set(false);
                self.save_error.replace(None);
                self.toast("Reverted to the saved configuration");
            }
            None => {
                self.with_loading(|state| {
                    let loaded = state.loaded.borrow().clone();
                    state.form.load(&loaded);
                });
                self.dirty.set(false);
                self.save_error.replace(None);
                self.toast("Reverted unsaved changes");
            }
        }
        self.update_actions();
    }

    /// Refresh daemon/kanata/uinput status off the UI thread.
    pub fn refresh_status(&self) {
        let generation = self.status_generation.get().wrapping_add(1);
        self.status_generation.set(generation);
        let socket = model::socket_path();
        let weak = self.self_ref.clone();
        spawn_blocking(
            move || model::gather_status(socket),
            move |snapshot| {
                if let Some(state) = weak.upgrade() {
                    if state.status_generation.get() == generation {
                        state.show_status(&snapshot);
                    }
                }
            },
        );
    }

    fn show_status(&self, snapshot: &model::StatusSnapshot) {
        let Some(widgets) = self.status_widgets.borrow().clone() else {
            return;
        };
        widgets.source.set_subtitle(&self.config_source.borrow());
        widgets
            .warning
            .set_visible(self.config_warning.borrow().is_some());
        if let Some(warning) = self.config_warning.borrow().as_deref() {
            widgets.warning.set_subtitle(warning);
        }
        match &snapshot.daemon {
            Ok(daemon) => {
                widgets
                    .daemon
                    .set_subtitle(&format!("reachable · version {}", daemon.version));
                widgets.overlay.set_subtitle(if daemon.overlay_connected {
                    "connected"
                } else {
                    "disconnected"
                });
                widgets.session.set_subtitle(if daemon.session_active {
                    "active"
                } else {
                    "idle"
                });
                widgets.free_mode.set_subtitle(if daemon.free_mode {
                    "running"
                } else {
                    "inactive"
                });
                widgets.config_path.set_subtitle(&daemon.config_path);
            }
            Err(error) => {
                widgets
                    .daemon
                    .set_subtitle(&format!("unreachable — {error}"));
                widgets.overlay.set_subtitle("unknown (daemon unreachable)");
                widgets.session.set_subtitle("unknown (daemon unreachable)");
                widgets
                    .free_mode
                    .set_subtitle("unknown (daemon unreachable)");
                widgets
                    .config_path
                    .set_subtitle(&model::config_path().display().to_string());
            }
        }
        widgets.kanata.set_subtitle(&match &snapshot.kanata_layer {
            Ok(layer) => format!("layer {layer}"),
            Err(error) => format!("TCP unavailable — {error}"),
        });
        widgets.uinput.set_subtitle(if snapshot.uinput_present {
            "present (the daemon owns writes; humans need no input group)"
        } else {
            "missing (/dev/uinput not found)"
        });
    }

    /// Rebuild the preset group from built-ins + user files.
    pub fn refresh_presets(&self) {
        let group = self.presets_group.borrow().clone();
        let Some(group) = group else {
            return;
        };
        let Some(this) = self.self_ref.upgrade() else {
            return;
        };
        // `AdwPreferencesGroup` is not a generic container: its first child is
        // the internal box. Track our own rows and remove exactly those.
        for row in self.preset_rows.borrow_mut().drain(..) {
            group.remove(&row);
        }
        for preset in mouseless_config::builtins() {
            let row = pages::preset_row(&this, &preset, true);
            group.add(&row);
            self.preset_rows.borrow_mut().push(row);
        }
        match mouseless_config::load_user_presets(&self.presets_dir) {
            Ok(presets) => {
                for preset in presets {
                    let row = pages::preset_row(&this, &preset, false);
                    group.add(&row);
                    self.preset_rows.borrow_mut().push(row);
                }
            }
            Err(error) => self.toast(&format!("Could not read user presets: {error}")),
        }
    }

    /// Save the current behavior as a named user preset.
    pub fn save_current_preset(&self) {
        let name = self.form.preset_name.text().trim().to_string();
        if name.is_empty() {
            self.toast("Enter a preset name first");
            return;
        }
        let preset = Preset {
            name,
            behavior: self.form.to_behavior(),
        };
        match mouseless_config::save_preset(&self.presets_dir, &preset) {
            Ok(path) => {
                self.refresh_presets();
                self.toast(&format!("Saved preset to {}", path.display()));
            }
            Err(error) => show_dialog(&self.window, "Preset not saved", &error.to_string()),
        }
    }

    /// Merge a preset into the current draft (behavior only), then auto-save.
    pub fn apply_preset(&self, preset: &Preset) {
        // Merge against the current draft so pending grid/appearance edits in
        // other sections are kept; a preset only replaces `behavior`.
        let draft = self.form.to_config(&self.loaded.borrow());
        let merged = mouseless_config::merge_preset(&draft, preset);
        self.with_loading(|state| state.form.load(&merged));
        self.mark_dirty();
        self.toast(&format!(
            "{} loaded — saved automatically; use Restart App to apply it",
            preset.name
        ));
    }

    /// Import a preset file into the user preset directory.
    pub fn import_preset(&self, path: &std::path::Path) {
        match mouseless_config::import_preset(path) {
            Ok(preset) => match mouseless_config::save_preset(&self.presets_dir, &preset) {
                Ok(_) => {
                    self.refresh_presets();
                    self.toast(&format!("Imported preset {}", preset.name));
                }
                Err(error) => show_dialog(&self.window, "Preset not imported", &error.to_string()),
            },
            Err(error) => show_dialog(&self.window, "Preset not imported", &error.to_string()),
        }
    }

    /// Export a preset to a user-chosen path.
    pub fn export_preset(&self, preset: &Preset, path: &std::path::Path) {
        match mouseless_config::export_preset(path, preset) {
            Ok(()) => self.toast(&format!("Exported preset to {}", path.display())),
            Err(error) => show_dialog(&self.window, "Preset not exported", &error.to_string()),
        }
    }

    /// Save a generated kanata include for the current draft.
    pub fn export_include(&self, path: &std::path::Path) {
        let candidate = self.form.to_config(&self.loaded.borrow());
        if let Err(error) = validate(&candidate) {
            show_dialog(
                &self.window,
                "Include not saved",
                &format!("Fix these fields first:\n\n{}", format_issues(&error)),
            );
            return;
        }
        let include = mouseless_config::render_include(&candidate);
        match std::fs::write(path, include) {
            Ok(()) => self.toast(&format!(
                "Saved include to {} — install it as /etc/kanata/mouseless.kbd with sudo",
                path.display()
            )),
            Err(error) => show_dialog(
                &self.window,
                "Include not saved",
                &format!("{}: {error}", path.display()),
            ),
        }
    }

    /// Run `f` with change handlers suppressed.
    fn with_loading<F: FnOnce(&Self)>(&self, f: F) {
        self.loading.set(true);
        f(self);
        self.loading.set(false);
    }
}

/// One-line-per-issue rendering shared by restart and include export.
fn format_issues(error: &mouseless_config::ValidationError) -> String {
    error
        .issues()
        .iter()
        .map(|issue| format!("{}: {}", issue.field, issue.message))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Compact one-line validation error for the footer while typing.
fn summarize_issues(error: &mouseless_config::ValidationError) -> String {
    match error.issues().split_first() {
        Some((first, [])) => format!("{}: {}", first.field, first.message),
        Some((first, rest)) => format!("{}: {} (+{} more)", first.field, first.message, rest.len()),
        None => error.to_string(),
    }
}

/// Persist the candidate with `Save`; the running app is left untouched.
///
/// Kanata-affecting edits run the same include check as Restart App before
/// they are persisted, so a rejected include cannot become the startup config.
fn attempt_save(socket: &std::path::Path, previous: &Config, candidate: &Config) -> SaveOutcome {
    match model::check_kanata_candidate(previous, candidate) {
        Ok(_) => {}
        Err(message) => return SaveOutcome::CheckRejected(message),
    }
    match request(
        socket,
        &ClientToDaemon::Save {
            config: Box::new(candidate.clone()),
        },
        model::client_timeout(),
    ) {
        Ok(DaemonToClient::Ack { .. }) => SaveOutcome::Saved {
            config: Box::new(candidate.clone()),
        },
        Ok(DaemonToClient::Error { code, message }) => SaveOutcome::DaemonError { code, message },
        Ok(other) => SaveOutcome::DaemonError {
            code: "unexpected_reply".into(),
            message: format!("{other:?}"),
        },
        Err(error) => SaveOutcome::Unreachable(error.to_string()),
    }
}

/// Persist and apply the candidate with `Update` (the Restart App action).
fn attempt_restart(
    socket: &std::path::Path,
    previous: &Config,
    candidate: &Config,
) -> RestartOutcome {
    let warnings = match model::check_kanata_candidate(previous, candidate) {
        Ok(warnings) => warnings,
        Err(message) => return RestartOutcome::CheckRejected(message),
    };
    match request(
        socket,
        &ClientToDaemon::Update {
            config: Box::new(candidate.clone()),
        },
        model::client_timeout(),
    ) {
        Ok(DaemonToClient::Ack { message }) => RestartOutcome::Restarted {
            config: Box::new(candidate.clone()),
            message,
            warnings,
        },
        Ok(DaemonToClient::Error { code, message }) => {
            RestartOutcome::DaemonError { code, message }
        }
        Ok(other) => RestartOutcome::DaemonError {
            code: "unexpected_reply".into(),
            message: format!("{other:?}"),
        },
        Err(error) => RestartOutcome::Unreachable(error.to_string()),
    }
}

fn load_config() -> Loaded {
    let socket = model::socket_path();
    match request(&socket, &ClientToDaemon::GetConfig, model::client_timeout()) {
        Ok(reply) => match model::config_from_reply(reply) {
            Ok(config) => Loaded {
                config,
                source: "saved daemon config".into(),
                warning: None,
            },
            Err(error) => fallback_config(Some(format!("daemon config read failed: {error}"))),
        },
        Err(error) => fallback_config(Some(format!(
            "daemon unreachable ({error}); showing the last persisted copy"
        ))),
    }
}

fn fallback_config(reason: Option<String>) -> Loaded {
    let store = ConfigStore::new(model::config_path());
    let (config, error) = store.effective();
    let source = match &error {
        None => format!("last-good file ({})", store.config_path().display()),
        Some(ConfigError::NotFound { .. }) => "schema defaults (no daemon copy yet)".into(),
        Some(_) => format!(
            "last-good file ({}) after a parse error",
            store.config_path().display()
        ),
    };
    let warning = match (reason, error) {
        (Some(reason), Some(error)) => Some(format!("{reason}; {error}")),
        (Some(reason), None) => Some(reason),
        (None, Some(error)) => Some(error.to_string()),
        (None, None) => None,
    };
    Loaded {
        config,
        source,
        warning,
    }
}

fn preset_dir() -> PathBuf {
    match ConfigStore::user_default() {
        Ok(store) => store.presets_dir().to_path_buf(),
        Err(_) => {
            let base = std::env::var_os("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .or_else(|| {
                    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config"))
                })
                .unwrap_or_else(|| PathBuf::from("."));
            base.join("mouseless").join("presets")
        }
    }
}

/// Run `work` on a worker thread and `done` on the GTK main thread.
fn spawn_blocking<T, W, D>(work: W, done: D)
where
    T: Send + 'static,
    W: FnOnce() -> T + Send + 'static,
    D: FnOnce(T) + 'static,
{
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = sender.send(work());
    });
    let mut done = Some(done);
    glib::timeout_add_local(Duration::from_millis(20), move || {
        match receiver.try_recv() {
            Ok(value) => {
                if let Some(done) = done.take() {
                    done(value);
                }
                glib::ControlFlow::Break
            }
            Err(TryRecvError::Empty) => glib::ControlFlow::Continue,
            Err(TryRecvError::Disconnected) => glib::ControlFlow::Break,
        }
    });
}

/// Show a blocking information/error dialog.
pub fn show_dialog(parent: &impl IsA<gtk::Widget>, heading: &str, body: &str) {
    let dialog = adw::AlertDialog::new(Some(heading), Some(body));
    dialog.add_response("close", "Close");
    dialog.set_default_response(Some("close"));
    dialog.set_close_response("close");
    dialog.present(Some(parent));
}

/// Open a native file chooser and call `done` with the chosen path.
pub fn choose_file<F>(
    parent: &impl IsA<gtk::Window>,
    title: &str,
    action: gtk::FileChooserAction,
    suggested_name: Option<&str>,
    done: F,
) where
    F: Fn(&std::path::Path) + 'static,
{
    let accept = match action {
        gtk::FileChooserAction::Save => "Save",
        gtk::FileChooserAction::Open => "Open",
        _ => "Select",
    };
    let dialog = gtk::FileChooserNative::new(
        Some(title),
        Some(parent),
        action,
        Some(accept),
        Some("Cancel"),
    );
    if let Some(name) = suggested_name {
        dialog.set_current_name(name);
    }
    dialog.connect_response(move |dialog, response| {
        if response == gtk::ResponseType::Accept {
            if let Some(path) = dialog.file().and_then(|file| file.path()) {
                done(&path);
            }
        }
        dialog.destroy();
    });
    dialog.show();
}

#[cfg(test)]
mod tests {
    use super::*;
    use mouseless_ipc::{decode, encode, KIND_CLIENT, KIND_DAEMON};
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixListener;

    fn socket_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "mouseless-gtk-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A one-shot daemon that asserts the GUI sent `expected` and replies once.
    fn fake_daemon(
        dir: &std::path::Path,
        expected: &'static str,
        reply: DaemonToClient,
    ) -> PathBuf {
        let socket = dir.join("daemon.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let envelope = decode(&line).unwrap();
            assert_eq!(envelope.kind, KIND_CLIENT);
            assert_eq!(envelope.payload["type"], expected);
            let line = encode(KIND_DAEMON, &reply).unwrap();
            stream.write_all(format!("{line}\n").as_bytes()).unwrap();
        });
        socket
    }

    #[test]
    fn save_persists_with_save_and_returns_the_ack() {
        let dir = socket_dir("save-ack");
        let socket = fake_daemon(
            &dir,
            "Save",
            DaemonToClient::Ack {
                message: "configuration saved; restart the app to apply it".into(),
            },
        );
        let config = Config::default();
        match attempt_save(&socket, &config, &config) {
            SaveOutcome::Saved { config: saved } => {
                assert_eq!(*saved, config);
            }
            _ => panic!("expected a saved outcome"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_surfaces_a_daemon_rejection() {
        let dir = socket_dir("save-error");
        let socket = fake_daemon(
            &dir,
            "Save",
            DaemonToClient::Error {
                code: "invalid_config".into(),
                message: "behavior.tap_threshold_ms: bad".into(),
            },
        );
        let config = Config::default();
        match attempt_save(&socket, &config, &config) {
            SaveOutcome::DaemonError { code, message } => {
                assert_eq!(code, "invalid_config");
                assert!(message.contains("tap_threshold_ms"));
            }
            _ => panic!("expected a daemon error"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_reports_an_unreachable_daemon() {
        let dir = socket_dir("save-missing");
        let config = Config::default();
        match attempt_save(&dir.join("missing.sock"), &config, &config) {
            SaveOutcome::Unreachable(message) => assert!(!message.is_empty()),
            _ => panic!("expected an unreachable outcome"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn restart_pushes_update_and_returns_the_ack() {
        let dir = socket_dir("restart-ack");
        let socket = fake_daemon(
            &dir,
            "Update",
            DaemonToClient::Ack {
                message: "configuration saved and applied".into(),
            },
        );
        let config = Config::default();
        match attempt_restart(&socket, &config, &config) {
            RestartOutcome::Restarted {
                config: applied,
                message,
                warnings,
            } => {
                assert_eq!(*applied, config);
                assert_eq!(message, "configuration saved and applied");
                assert!(warnings.is_empty());
            }
            _ => panic!("expected a restarted outcome"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn restart_surfaces_a_daemon_rejection() {
        let dir = socket_dir("restart-error");
        let socket = fake_daemon(
            &dir,
            "Update",
            DaemonToClient::Error {
                code: "invalid_config".into(),
                message: "behavior.tap_threshold_ms: bad".into(),
            },
        );
        let config = Config::default();
        match attempt_restart(&socket, &config, &config) {
            RestartOutcome::DaemonError { code, message } => {
                assert_eq!(code, "invalid_config");
                assert!(message.contains("tap_threshold_ms"));
            }
            _ => panic!("expected a daemon error"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn restart_reports_an_unreachable_daemon() {
        let dir = socket_dir("restart-missing");
        let config = Config::default();
        match attempt_restart(&dir.join("missing.sock"), &config, &config) {
            RestartOutcome::Unreachable(message) => assert!(!message.is_empty()),
            _ => panic!("expected an unreachable outcome"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
