//! `mouseless` binary: show/hide/toggle, layer commands, device list.
//!
//! `status` queries live kanata TCP; `--list-input-devices` parses
//! `/proc/bus/input/devices` (no special permissions needed, unlike the
//! `EVIOCGNAME` ioctl path). Layer-changing verbs speak kanata TCP so the
//! layer stays the single source of truth (plan §1.3, SPEC-cli); the daemon
//! reacts to the resulting `LayerChange` and owns grab/session state.
//!
//! `--kanata ADDR` and `MOUSELESS_KANATA` override the localhost endpoint
//! (useful for fake-TCP tests and non-default ports).

use mouseless_cli::{toggle_target, CliError, Verb};
use mouseless_ipc::{request, ClientToDaemon, DaemonToClient, CLIENT_TIMEOUT, SOCKET_PATH};
use mouseless_kbd::{query_current_layer, send_layer_change, KanataTcpConfig};
use std::process::ExitCode;
use std::time::Duration;

fn list_input_devices() {
    let text = std::fs::read_to_string("/proc/bus/input/devices").unwrap_or_default();
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
                let tag = if name.contains("kanata") {
                    "  [kanata output]"
                } else if name.contains("mouseless-mouse") {
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

fn usage() {
    eprintln!("{}", CliError::Usage);
    eprintln!("  [--kanata HOST:PORT]   default 127.0.0.1:7070 (or MOUSELESS_KANATA)");
    eprintln!("  [--socket PATH]        default {SOCKET_PATH} (or MOUSELESS_SOCKET)");
}

/// Parse `[--kanata ADDR] [--socket PATH] <verb>`.
///
/// Returns the kanata endpoint, the daemon socket path, and the raw verb.
fn parse_args() -> Result<(KanataTcpConfig, String, String), String> {
    let mut addr =
        std::env::var("MOUSELESS_KANATA").unwrap_or_else(|_| "127.0.0.1:7070".to_string());
    let mut socket = std::env::var("MOUSELESS_SOCKET").unwrap_or_else(|_| SOCKET_PATH.to_string());
    let mut verb: Option<String> = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
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
                println!("usage: mouseless [--kanata HOST:PORT] [--socket PATH] <verb>");
                println!("verbs: show hide toggle enter-free-mode exit-free-mode status reload");
                println!("flags: --list-input-devices, --version");
                println!("exit: 0 success, 1 operational failure, 2 usage error");
                std::process::exit(0);
            }
            "--version" | "-V" => {
                println!("mouseless {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            other => {
                if verb.is_some() {
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
            timeout: Duration::from_secs(3),
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

fn main() -> ExitCode {
    let (cfg, socket, verb_arg) = match parse_args() {
        Ok(v) => v,
        Err(e) => {
            eprintln!("Error: {e}");
            usage();
            return ExitCode::from(2);
        }
    };
    let verb = match Verb::parse(&verb_arg) {
        Ok(v) => v,
        Err(_) => {
            usage();
            return ExitCode::from(2);
        }
    };
    match verb {
        Verb::Status => match query_current_layer(&cfg) {
            Ok(name) => {
                println!("kanata: {name}");
                match request(&socket, &ClientToDaemon::Status, CLIENT_TIMEOUT) {
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
        Verb::Reload => match request(&socket, &ClientToDaemon::Reload, CLIENT_TIMEOUT) {
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
            list_input_devices();
            ExitCode::SUCCESS
        }
        Verb::Toggle => match query_current_layer(&cfg) {
            Ok(current) => change_layer(&cfg, toggle_target(&current)),
            Err(e) => query_error(&e),
        },
        Verb::Show | Verb::Hide | Verb::EnterFreeMode | Verb::ExitFreeMode => {
            match verb.layer_change() {
                Some(layer) => change_layer(&cfg, layer),
                None => {
                    usage();
                    ExitCode::from(2)
                }
            }
        }
    }
}
