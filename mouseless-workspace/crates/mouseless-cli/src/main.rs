//! `mouseless` binary: show/hide/toggle, layer commands, device list.
//!
//! `status` queries live kanata TCP; `--list-input-devices` parses
//! `/proc/bus/input/devices` (no special permissions needed, unlike the
//! `EVIOCGNAME` ioctl path). Layer-changing verbs speak kanata TCP in
//! Phase 1.2; the scaffold prints the intent so deep-link parity (plan §4)
//! stays testable until then.

use mouseless_cli::{CliError, Verb};
use mouseless_kbd::{KanataTcpConfig, query_current_layer};
use std::process::ExitCode;

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
    eprintln!("{out} devices (grab/exclude/ignore classification in Phase 1.2)");
}

fn main() -> ExitCode {
    let arg = std::env::args().nth(1);
    let verb = match arg.as_deref().map(Verb::parse) {
        Some(Ok(v)) => v,
        _ => {
            eprintln!("Error: {}", CliError::Usage);
            return ExitCode::FAILURE;
        }
    };
    match verb {
        Verb::Status => {
            match query_current_layer(&KanataTcpConfig::default()) {
                Ok(name) => {
                    println!("kanata layer: {name}");
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("kanata TCP unavailable: {e}");
                    ExitCode::FAILURE
                }
            }
        }
        Verb::ListInputDevices => {
            list_input_devices();
            ExitCode::SUCCESS
        }
        _ => {
            println!("mouseless: {verb:?} ({})", verb.deep_link());
            println!("Phase 1.2 sends this over kanata TCP / daemon socket.");
            ExitCode::SUCCESS
        }
    }
}
