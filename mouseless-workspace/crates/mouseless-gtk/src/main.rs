//! `mouseless-settings`: GTK4/Adwaita settings app.
//!
//! Single-instance `AdwApplication`; a second launch forwards its
//! `mouseless://settings/<page>` argument to the running window. The app is a
//! thin shell over `mouseless-config`: edits are validated locally and
//! auto-saved with `Save` (persist only), while “Restart App” sends `Update`
//! to make the saved values live. The app itself never writes the config file.

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
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// Application id (matches the launcher desktop entry).
pub const APP_ID: &str = "click.mouseless.Settings";

thread_local! {
    /// Keep the single window's state alive for the process lifetime.
    static STATE: RefCell<Option<Rc<state::AppState>>> = const { RefCell::new(None) };
}

fn main() -> glib::ExitCode {
    // Help/version are answered before GTK starts so a forwarded argument can
    // never terminate a running instance.
    for argument in std::env::args().skip(1) {
        match argument.as_str() {
            "--help" | "-h" => {
                println!("usage: mouseless-settings [mouseless://settings/<page>]");
                println!("pages: grid appearance behavior free-mode keybindings presets status");
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
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::HANDLES_COMMAND_LINE)
        .build();

    let requested = Rc::new(Cell::new(SettingsPage::Grid));
    let command_page = requested.clone();
    app.connect_command_line(
        move |app, command_line| match parse_command_line(command_line) {
            Ok(page) => {
                command_page.set(page);
                app.activate();
                glib::ExitCode::SUCCESS
            }
            Err(message) => {
                eprintln!("mouseless-settings: {message}");
                eprintln!("usage: mouseless-settings [mouseless://settings/<page>]");
                glib::ExitCode::new(2)
            }
        },
    );

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

fn parse_command_line(command_line: &gio::ApplicationCommandLine) -> Result<SettingsPage, String> {
    let mut page = SettingsPage::Grid;
    let arguments = command_line.arguments();
    // gio includes argv[0]; only the forwarded page arguments matter here.
    for argument in arguments.iter().skip(1) {
        let Some(text) = argument.to_str() else {
            return Err("arguments must be valid UTF-8".into());
        };
        if let Some(parsed) = SettingsPage::parse(text) {
            page = parsed;
        } else if let Ok(Verb::Settings(parsed)) = Verb::parse(text) {
            page = parsed;
        } else {
            return Err(format!("unknown argument {text:?}"));
        }
    }
    Ok(page)
}
