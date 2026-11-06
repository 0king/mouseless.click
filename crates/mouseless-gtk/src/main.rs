//! `mouseless-settings`: GTK4/Adwaita settings app.
//!
//! Single-instance `AdwApplication`; a second launch forwards its
//! `mouseless://settings/<page>` argument to the running window. The app is a
//! thin shell over `mouseless-config`: edits are validated locally and
//! auto-saved with `Save` (persist only), while “Restart App” sends `Update`
//! to make the saved values live. The app itself never writes the config file.
//!
//! Application-level constants (application id, daemon socket, config paths,
//! kanata endpoint, deep-link scheme, appearance fallbacks) come from
//! `mouseless.toml`; see [`mouseless_config::appconfig`].

mod form;
mod model;
mod pages;
mod preview;
mod state;

use adw::prelude::*;
use gtk::gio::prelude::*;
use gtk::prelude::*;
use gtk::{gio, glib};
use libadwaita as adw;
use mouseless_cli::{SettingsPage, Verb};
use mouseless_config::{load_effective, AppConfig};
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;

thread_local! {
    /// Keep the single window's state alive for the process lifetime.
    static STATE: RefCell<Option<Rc<state::AppState>>> = const { RefCell::new(None) };
}

/// Find `--config PATH` / `--config=PATH` before GTK starts.
fn config_path_arg() -> Option<PathBuf> {
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if let Some(path) = arg.strip_prefix("--config=") {
            return Some(PathBuf::from(path));
        }
        if arg == "--config" {
            return args.next().map(PathBuf::from);
        }
    }
    None
}

/// Load `mouseless.toml`; a required file that is missing/invalid is fatal.
fn load_app() -> Result<AppConfig, String> {
    let explicit = config_path_arg();
    let loaded = load_effective(explicit.as_deref());
    match loaded.error {
        Some(error) => Err(format!("{}: {error}", loaded.source.label())),
        None => Ok(loaded.config),
    }
}

fn main() -> glib::ExitCode {
    let app_config = match load_app() {
        Ok(config) => config,
        Err(message) => {
            eprintln!("mouseless-settings: {message}");
            return glib::ExitCode::new(2);
        }
    };
    model::install_app_config(app_config.clone());

    // Help/version are answered before GTK starts so a forwarded argument can
    // never terminate a running instance.
    for argument in std::env::args().skip(1) {
        match argument.as_str() {
            "--help" | "-h" => {
                println!(
                    "usage: mouseless-settings [--config PATH] [{}://settings/<page>]",
                    app_config.deep_link.scheme
                );
                println!("pages: grid appearance behavior free-mode keybindings presets status");
                println!(
                    "config: {} (or MOUSELESS_CONFIG)",
                    mouseless_config::APP_CONFIG_PATH
                );
                return glib::ExitCode::SUCCESS;
            }
            "--version" | "-V" => {
                println!("mouseless-settings {}", env!("CARGO_PKG_VERSION"));
                return glib::ExitCode::SUCCESS;
            }
            _ => {}
        }
    }

    let app = adw::Application::builder()
        .application_id(app_config.ui.app_id.clone())
        .flags(gio::ApplicationFlags::HANDLES_COMMAND_LINE)
        .build();

    let requested = Rc::new(Cell::new(SettingsPage::Grid));
    let command_page = requested.clone();
    let scheme = app_config.deep_link.scheme.clone();
    app.connect_command_line(move |app, command_line| {
        match parse_command_line(command_line, &scheme) {
            Ok(page) => {
                command_page.set(page);
                app.activate();
                glib::ExitCode::SUCCESS
            }
            Err(message) => {
                eprintln!("mouseless-settings: {message}");
                eprintln!("usage: mouseless-settings [{}://settings/<page>]", scheme);
                glib::ExitCode::new(2)
            }
        }
    });

    let activate_page = requested.clone();
    app.connect_activate(move |app| {
        let page = activate_page.get();
        if let Some(window) = app.active_window() {
            if let Some(window) = window.downcast_ref::<adw::PreferencesWindow>() {
                window.set_visible_page_name(page.name());
                window.present();
                return;
            }
        }
        let state = state::AppState::build(app, page);
        STATE.with(|slot| {
            *slot.borrow_mut() = Some(state);
        });
    });

    app.run()
}

fn parse_command_line(
    command_line: &gio::ApplicationCommandLine,
    scheme: &str,
) -> Result<SettingsPage, String> {
    let mut page = SettingsPage::Grid;
    let arguments = command_line.arguments();
    // gio includes argv[0]; only the forwarded page arguments matter here.
    let mut arguments = arguments.iter().skip(1);
    while let Some(argument) = arguments.next() {
        let Some(text) = argument.to_str() else {
            return Err("arguments must be valid UTF-8".into());
        };
        // `--config` is consumed before the GTK process starts; a running
        // instance must ignore it rather than treat it as a page name.
        if text == "--config" {
            let _ = arguments.next();
            continue;
        }
        if text.starts_with("--config=") {
            continue;
        }
        if let Some(parsed) = SettingsPage::parse_with_scheme(text, scheme) {
            page = parsed;
        } else if let Ok(Verb::Settings(parsed)) = Verb::parse_with_scheme(text, scheme) {
            page = parsed;
        } else {
            return Err(format!("unknown argument {text:?}"));
        }
    }
    Ok(page)
}
