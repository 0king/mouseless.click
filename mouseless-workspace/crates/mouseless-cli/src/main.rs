//! `mouseless` binary: show/hide/toggle, layer commands, device list.
//!
//! `status` queries live kanata TCP; `--list-input-devices` parses the kernel
//! input listing (no special permissions needed, unlike the `EVIOCGNAME` ioctl
//! path). Layer-changing verbs speak kanata TCP so the layer stays the single
//! source of truth (plan §1.3, SPEC-cli); the daemon reacts to the resulting
//! `LayerChange` and owns grab/session state.
//!
//! Defaults for the endpoint, socket, scheme, settings binary, and device-list
//! path come from `mouseless.toml` (`--config` or `MOUSELESS_CONFIG`). The
//! historical `--kanata`/`--socket` flags and `MOUSELESS_KANATA`/
//! `MOUSELESS_SOCKET`/`MOUSELESS_SETTINGS` environment variables still
//! override the file for development and tests.

use mouseless_cli::{toggle_target_with, CliError, LayerNames, SettingsPage, Verb};
use mouseless_config::{load_effective, AppConfig};
use mouseless_ipc::{request, ClientToDaemon, DaemonToClient};
use mouseless_kbd::{
    query_current_layer, send_layer_change, KanataTcpConfig, FREE_MODE_LAYER, OVERLAY_LAYER,
};
use std::path::PathBuf;
use std::process::{Command, ExitCode, Stdio};

/// Find `--config PATH` / `--config=PATH` before the main argument pass.
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

fn list_input_devices(app: &AppConfig) {
    let text = std::fs::read_to_string(&app.paths.input_devices).unwrap_or_default();
    let mut name = String::new();
    let mut sysfs = String::new();
    let mut out = 0;
    for line in text.lines().chain([""]) {
        if let Some(n) = line.strip_prefix("N: Name=") {
            name = n.trim_matches('"').to_string();
        } else if let Some(s) = line.strip_prefix("S: Sysfs=") {
            sysfs = s.to_string();
        } else if line.is_empty() {
            if !name.is_empty() {
                let tag = if name.contains(&app.devices.kanata_output_name) {
                    "  [kanata output]"
                } else if name.contains(&app.devices.mouse_name) {
                    "  [ours: kanata-excluded]"
                } else {
                    ""
                };
                println!("{name}  ({sysfs}){tag}");
                out += 1;
            }
            name.clear();
            sysfs.clear();
        }
    }
    eprintln!("{out} devices");
}

fn usage(app: &AppConfig) {
    eprintln!("{}", CliError::Usage);
    eprintln!(
        "  [--config PATH]        default {} (or MOUSELESS_CONFIG)",
        mouseless_config::APP_CONFIG_PATH
    );
    eprintln!(
        "  [--kanata HOST:PORT]   default {} (or MOUSELESS_KANATA)",
        app.kanata.address
    );
    eprintln!(
        "  [--socket PATH]        default {} (or MOUSELESS_SOCKET)",
        app.paths.socket
    );
}

/// Parse `[--config PATH] [--kanata ADDR] [--socket PATH] <verb>`.
///
/// Returns the kanata endpoint, the daemon socket path, and the raw verb.
/// CLI flags win over environment variables, which win over `mouseless.toml`.
fn parse_args(app: &AppConfig) -> Result<(KanataTcpConfig, String, String), String> {
    let mut addr = std::env::var("MOUSELESS_KANATA").unwrap_or_else(|_| app.kanata.address.clone());
    let mut socket = std::env::var("MOUSELESS_SOCKET").unwrap_or_else(|_| app.paths.socket.clone());
    let mut verb: Option<String> = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--config" => {
                // Already applied by `config_path_arg`; consume its value.
                let _ = args.next();
            }
            _ if a.starts_with("--config=") => {}
            "--kanata" => {
                addr = args
                    .next()
                    .ok_or_else(|| "--kanata requires HOST:PORT".to_string())?;
            }
            "--socket" => {
                socket = args
                    .next()
                    .ok_or_else(|| "--socket requires PATH".to_string())?;
            }
            "--help" | "-h" => {
                println!(
                    "usage: mouseless [--config PATH] [--kanata HOST:PORT] [--socket PATH] <verb>"
                );
                println!(
                    "verbs: show hide toggle enter-free-mode exit-free-mode status reload settings [page]"
                );
                println!(
                    "settings pages: grid appearance behavior free-mode keybindings presets status"
                );
                println!("flags: --list-input-devices, --version");
                println!("exit: 0 success, 1 operational failure, 2 usage error");
                std::process::exit(0);
            }
            "--version" | "-V" => {
                println!("mouseless {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            other => {
                if let Some(existing) = verb.as_deref() {
                    // `mouseless settings <page>` is sugar for
                    // `mouseless settings/<page>` (the form forwarded by the
                    // scheme handler). Page names are validated here so a
                    // typo fails before any daemon I/O.
                    if existing == "settings" {
                        if let Some(page) =
                            SettingsPage::parse_with_scheme(other, &app.deep_link.scheme)
                        {
                            verb = Some(format!("settings/{}", page.name()));
                            continue;
                        }
                    }
                    return Err(format!("unexpected argument {other}"));
                }
                verb = Some(other.to_string());
            }
        }
    }
    let verb = verb.ok_or_else(|| "missing verb".to_string())?;
    let (host, port) = addr
        .rsplit_once(':')
        .ok_or_else(|| format!("invalid kanata address {addr}"))?;
    let port = port
        .parse()
        .map_err(|_| format!("invalid port in {addr}"))?;
    Ok((
        KanataTcpConfig {
            host: host.to_string(),
            port,
            timeout: app.kanata.query_timeout(),
        },
        socket,
        verb,
    ))
}

fn change_layer(cfg: &KanataTcpConfig, layer: &str) -> ExitCode {
    match send_layer_change(cfg, layer) {
        Ok(()) => {
            println!("mouseless: layer {layer}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("kanata TCP unavailable: {e}");
            ExitCode::FAILURE
        }
    }
}

fn query_error(e: &mouseless_kbd::KbdError) -> ExitCode {
    eprintln!("kanata TCP unavailable: {e}");
    ExitCode::FAILURE
}

/// Hand a settings deep link to the settings app (single instance).
///
/// The URL handler always points at this CLI so overlay verbs keep working
/// without the GTK app; settings links are the only ones forwarded.
fn open_settings(page: SettingsPage, app: &AppConfig) -> ExitCode {
    let program = std::env::var("MOUSELESS_SETTINGS")
        .unwrap_or_else(|_| app.deep_link.settings_binary.clone());
    let link = page.deep_link_with_scheme(&app.deep_link.scheme);
    let mut command = Command::new(&program);
    command
        .arg(&link)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    match command.spawn() {
        Ok(_) => {
            println!("mouseless: opening settings ({})", page.title());
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("mouseless: cannot open settings with {program}: {error}");
            eprintln!("install mouseless-settings or set MOUSELESS_SETTINGS to its path");
            ExitCode::FAILURE
        }
    }
}

fn main() -> ExitCode {
    let app = match load_app() {
        Ok(app) => app,
        Err(message) => {
            eprintln!("mouseless: {message}");
            return ExitCode::from(2);
        }
    };
    let (cfg, socket, verb_arg) = match parse_args(&app) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("Error: {e}");
            usage(&app);
            return ExitCode::from(2);
        }
    };
    let verb = match Verb::parse_with_scheme(&verb_arg, &app.deep_link.scheme) {
        Ok(v) => v,
        Err(_) => {
            usage(&app);
            return ExitCode::from(2);
        }
    };
    let layers = LayerNames {
        base: &app.layers.base,
        overlay: OVERLAY_LAYER,
        free_mode: FREE_MODE_LAYER,
    };
    let client_timeout = app.ipc.client_timeout();
    match verb {
        Verb::Status => match query_current_layer(&cfg) {
            Ok(name) => {
                println!("kanata: {name}");
                match request(&socket, &ClientToDaemon::Status, client_timeout) {
                    Ok(DaemonToClient::Status {
                        daemon_version,
                        overlay_connected,
                        session_active,
                        free_mode,
                        config_path,
                    }) => {
                        println!("daemon: ok ({daemon_version})");
                        println!(
                            "overlay: {}",
                            if overlay_connected {
                                "connected"
                            } else {
                                "disconnected"
                            }
                        );
                        println!(
                            "session: {}",
                            if session_active { "active" } else { "idle" }
                        );
                        println!(
                            "free-mode: {}",
                            if free_mode { "active" } else { "inactive" }
                        );
                        println!("config: {config_path}");
                    }
                    Ok(DaemonToClient::Error { code, message }) => {
                        eprintln!("daemon: error ({code}): {message}");
                    }
                    Ok(_) => eprintln!("daemon: unexpected reply"),
                    Err(e) => eprintln!("daemon: unavailable ({e})"),
                }
                ExitCode::SUCCESS
            }
            Err(e) => query_error(&e),
        },
        Verb::Reload => match request(&socket, &ClientToDaemon::Reload, client_timeout) {
            Ok(DaemonToClient::Ack { message }) => {
                println!("mouseless: {message}");
                ExitCode::SUCCESS
            }
            Ok(DaemonToClient::Error { code, message }) => {
                eprintln!("mouseless: reload failed ({code}): {message}");
                ExitCode::FAILURE
            }
            Ok(_) => {
                eprintln!("mouseless: reload failed: unexpected daemon reply");
                ExitCode::FAILURE
            }
            Err(e) => {
                eprintln!("mouseless: reload failed: {e}");
                ExitCode::FAILURE
            }
        },
        Verb::ListInputDevices => {
            list_input_devices(&app);
            ExitCode::SUCCESS
        }
        Verb::Settings(page) => open_settings(page, &app),
        Verb::Toggle => match query_current_layer(&cfg) {
            Ok(current) => change_layer(&cfg, toggle_target_with(&current, &layers)),
            Err(e) => query_error(&e),
        },
        Verb::Show | Verb::Hide | Verb::EnterFreeMode | Verb::ExitFreeMode => {
            match verb.layer_change_with(&layers) {
                Some(layer) => change_layer(&cfg, layer),
                None => {
                    usage(&app);
                    ExitCode::from(2)
                }
            }
        }
    }
}
