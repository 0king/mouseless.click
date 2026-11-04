//! Application state and the validate-then-apply lifecycle.
//!
//! The GUI never persists a shadow config: every apply is validated locally,
//! optionally checked against kanata, pushed as `Update` to the daemon, and
//! only then becomes the new baseline. A failure keeps the previous baseline
//! and the draft dirty so nothing is silently lost.

use crate::form::Form;
use crate::model;
use crate::pages::{self, StatusWidgets};
use adw::prelude::*;
use gtk::glib;
use gtk::prelude::*;
use libadwaita as adw;
use mouseless_cli::SettingsPage;
use mouseless_config::{validate, Config, ConfigError, ConfigStore, Preset};
use mouseless_ipc::{request, ClientToDaemon, DaemonToClient, CLIENT_TIMEOUT};
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

/// Result of one background apply attempt.
enum ApplyOutcome {
    Applied {
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
    /// Last applied / loaded baseline.
    pub loaded: RefCell<Config>,
    /// Whether the form differs from `loaded`.
    pub dirty: Cell<bool>,
    /// Suppress change handlers while loading widgets.
    pub loading: Cell<bool>,
    /// An apply is in flight.
    pub busy: Cell<bool>,
    /// User preset directory (`~/.config/mouseless/presets`).
    pub presets_dir: PathBuf,
    /// Preset rows container (installed by the Presets page).
    pub presets_group: RefCell<Option<adw::PreferencesGroup>>,
    /// Rows currently shown in [`Self::presets_group`], so a refresh removes
    /// only the rows we added instead of the group's internal container.
    pub preset_rows: RefCell<Vec<adw::ActionRow>>,
    /// "Unsaved changes" labels on each page's action group.
    pub action_labels: RefCell<Vec<gtk::Label>>,
    /// Apply buttons on each page.
    pub apply_buttons: RefCell<Vec<gtk::Button>>,
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
            presets_dir,
            presets_group: RefCell::new(None),
            preset_rows: RefCell::new(Vec::new()),
            action_labels: RefCell::new(Vec::new()),
            apply_buttons: RefCell::new(Vec::new()),
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

        // Ctrl+Enter applies from any page; the buttons remain the discoverable
        // path. A single global controller avoids one handler per page.
        let controller = gtk::ShortcutController::new();
        controller.set_scope(gtk::ShortcutScope::Global);
        let action = gtk::CallbackAction::new({
            let weak = state.self_ref.clone();
            move |_, _| {
                if let Some(state) = weak.upgrade() {
                    state.apply();
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
        self.update_actions();
    }

    /// Mark dirty after a programmatic draft change (preset apply).
    pub fn mark_dirty(&self) {
        self.dirty.set(true);
        self.update_actions();
    }

    /// Update each page's action buttons and labels.
    pub fn update_actions(&self) {
        let label = if self.busy.get() {
            "Applying…"
        } else if self.dirty.get() {
            "Unsaved changes"
        } else {
            "All changes applied"
        };
        for label_widget in self.action_labels.borrow().iter() {
            label_widget.set_text(label);
        }
        for button in self.apply_buttons.borrow().iter() {
            button.set_sensitive(!self.busy.get() && self.dirty.get());
        }
        for button in self.revert_buttons.borrow().iter() {
            button.set_sensitive(!self.busy.get() && self.dirty.get());
        }
    }

    /// Show a transient message on the window.
    pub fn toast(&self, message: &str) {
        self.window.add_toast(adw::Toast::new(message));
    }

    /// Validate the draft and push it to the daemon.
    pub fn apply(&self) {
        if self.busy.get() {
            return;
        }
        if !self.dirty.get() {
            self.toast("No changes to apply");
            return;
        }
        let candidate = self.form.to_config(&self.loaded.borrow());
        if let Err(error) = validate(&candidate) {
            show_dialog(
                &self.window,
                "Configuration not applied",
                &format!("Fix these fields first:\n\n{}", format_issues(&error)),
            );
            return;
        }

        let previous = self.loaded.borrow().clone();
        let socket = model::socket_path();
        self.busy.set(true);
        self.update_actions();
        self.toast("Applying configuration…");

        let weak = self.self_ref.clone();
        spawn_blocking(
            move || attempt_apply(&socket, &previous, &candidate),
            move |outcome| {
                if let Some(state) = weak.upgrade() {
                    state.finish_apply(outcome);
                }
            },
        );
    }

    fn finish_apply(&self, outcome: ApplyOutcome) {
        self.busy.set(false);
        match outcome {
            ApplyOutcome::Applied {
                config,
                message,
                warnings,
            } => {
                *self.loaded.borrow_mut() = *config;
                self.dirty.set(false);
                self.config_source.replace("live daemon".into());
                self.toast(&message);
                for warning in warnings {
                    self.toast(&warning);
                }
                self.config_warning.replace(None);
                self.refresh_status();
            }
            ApplyOutcome::CheckRejected(message) => {
                show_dialog(
                    &self.window,
                    "kanata rejected the generated include",
                    &format!(
                        "{message}\n\nThe daemon still has the previous configuration. \
                         Fix the layer names or tap timing, then apply again."
                    ),
                );
            }
            ApplyOutcome::DaemonError { code, message } => {
                show_dialog(
                    &self.window,
                    "The daemon rejected the configuration",
                    &format!("{code}: {message}\n\nThe previous configuration is still live."),
                );
            }
            ApplyOutcome::Unreachable(message) => {
                show_dialog(
                    &self.window,
                    "The daemon is not reachable",
                    &format!(
                        "{message}\n\nStart the daemon with \
                         `sudo systemctl start mouselessd`, then apply again. \
                         The previous configuration was not changed."
                    ),
                );
            }
        }
        self.update_actions();
    }

    /// Discard the draft and reload the daemon-owned document.
    pub fn revert(&self) {
        if self.busy.get() {
            return;
        }
        let live = request(
            model::socket_path(),
            &ClientToDaemon::GetConfig,
            CLIENT_TIMEOUT,
        )
        .ok()
        .and_then(|reply| model::config_from_reply(reply).ok());
        match live {
            Some(config) => {
                self.with_loading(|state| {
                    state.form.load(&config);
                    *state.loaded.borrow_mut() = config;
                });
                self.dirty.set(false);
                self.toast("Reverted to the live configuration");
            }
            None => {
                self.with_loading(|state| {
                    let loaded = state.loaded.borrow().clone();
                    state.form.load(&loaded);
                });
                self.dirty.set(false);
                self.toast("Reverted unsaved changes");
            }
        }
        self.update_actions();
    }

    /// Ask the daemon to re-read its persisted file, then refresh the form.
    pub fn reload_from_daemon(&self) {
        let socket = model::socket_path();
        match request(&socket, &ClientToDaemon::Reload, CLIENT_TIMEOUT) {
            Ok(DaemonToClient::Ack { message }) => {
                if let Ok(DaemonToClient::Config { config }) =
                    request(&socket, &ClientToDaemon::GetConfig, CLIENT_TIMEOUT)
                {
                    self.with_loading(|state| {
                        state.form.load(&config);
                        *state.loaded.borrow_mut() = *config;
                    });
                    self.dirty.set(false);
                }
                self.toast(&message);
                self.refresh_status();
            }
            Ok(DaemonToClient::Error { code, message }) => show_dialog(
                &self.window,
                "Reload failed",
                &format!("{code}: {message}\n\nThe previous configuration is still live."),
            ),
            Ok(_) => show_dialog(
                &self.window,
                "Reload failed",
                "The daemon sent an unexpected reply.",
            ),
            Err(error) => show_dialog(
                &self.window,
                "The daemon is not reachable",
                &error.to_string(),
            ),
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
                    .set_subtitle(&model::state_dir().join("config.yaml").display().to_string());
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

    /// Merge a preset into the current draft (behavior only) without applying.
    pub fn apply_preset(&self, preset: &Preset) {
        // Merge against the current draft so unsaved grid/appearance edits in
        // other sections are kept; a preset only replaces `behavior`.
        let draft = self.form.to_config(&self.loaded.borrow());
        let merged = mouseless_config::merge_preset(&draft, preset);
        self.with_loading(|state| state.form.load(&merged));
        self.mark_dirty();
        self.toast(&format!(
            "{} applied to the draft — press Apply to make it live",
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

/// One-line-per-issue rendering shared by apply and include export.
fn format_issues(error: &mouseless_config::ValidationError) -> String {
    error
        .issues()
        .iter()
        .map(|issue| format!("{}: {}", issue.field, issue.message))
        .collect::<Vec<_>>()
        .join("\n")
}

fn attempt_apply(socket: &std::path::Path, previous: &Config, candidate: &Config) -> ApplyOutcome {
    let warnings = match model::check_kanata_candidate(previous, candidate) {
        Ok(warnings) => warnings,
        Err(message) => return ApplyOutcome::CheckRejected(message),
    };
    match request(
        socket,
        &ClientToDaemon::Update {
            config: Box::new(candidate.clone()),
        },
        CLIENT_TIMEOUT,
    ) {
        Ok(DaemonToClient::Ack { message }) => ApplyOutcome::Applied {
            config: Box::new(candidate.clone()),
            message,
            warnings,
        },
        Ok(DaemonToClient::Error { code, message }) => ApplyOutcome::DaemonError { code, message },
        Ok(other) => ApplyOutcome::DaemonError {
            code: "unexpected_reply".into(),
            message: format!("{other:?}"),
        },
        Err(error) => ApplyOutcome::Unreachable(error.to_string()),
    }
}

fn load_config() -> Loaded {
    let socket = model::socket_path();
    match request(&socket, &ClientToDaemon::GetConfig, CLIENT_TIMEOUT) {
        Ok(reply) => match model::config_from_reply(reply) {
            Ok(config) => Loaded {
                config,
                source: "live daemon".into(),
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
    let store = ConfigStore::for_daemon(model::state_dir());
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

    /// A one-shot daemon that asserts the GUI sent `Update` and replies once.
    fn fake_daemon(dir: &std::path::Path, reply: DaemonToClient) -> PathBuf {
        let socket = dir.join("daemon.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let envelope = decode(&line).unwrap();
            assert_eq!(envelope.kind, KIND_CLIENT);
            assert_eq!(envelope.payload["type"], "Update");
            let line = encode(KIND_DAEMON, &reply).unwrap();
            stream.write_all(format!("{line}\n").as_bytes()).unwrap();
        });
        socket
    }

    #[test]
    fn apply_pushes_update_and_returns_the_ack() {
        let dir = socket_dir("apply-ack");
        let socket = fake_daemon(
            &dir,
            DaemonToClient::Ack {
                message: "configuration saved and applied".into(),
            },
        );
        let config = Config::default();
        let outcome = attempt_apply(&socket, &config, &config);
        match outcome {
            ApplyOutcome::Applied {
                config: applied,
                message,
                warnings,
            } => {
                assert_eq!(*applied, config);
                assert_eq!(message, "configuration saved and applied");
                assert!(warnings.is_empty());
            }
            _ => panic!("expected an applied outcome"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn apply_surfaces_a_daemon_rejection() {
        let dir = socket_dir("apply-error");
        let socket = fake_daemon(
            &dir,
            DaemonToClient::Error {
                code: "invalid_config".into(),
                message: "behavior.tap_threshold_ms: bad".into(),
            },
        );
        let config = Config::default();
        match attempt_apply(&socket, &config, &config) {
            ApplyOutcome::DaemonError { code, message } => {
                assert_eq!(code, "invalid_config");
                assert!(message.contains("tap_threshold_ms"));
            }
            _ => panic!("expected a daemon error"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn apply_reports_an_unreachable_daemon() {
        let dir = socket_dir("apply-missing");
        let config = Config::default();
        match attempt_apply(&dir.join("missing.sock"), &config, &config) {
            ApplyOutcome::Unreachable(message) => assert!(!message.is_empty()),
            _ => panic!("expected an unreachable outcome"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
