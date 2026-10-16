//! `mouseless` binary: show/hide/toggle, layer commands, device list.
//!
//! Phase 1.2 speaks kanata TCP (`ChangeLayer`) and the daemon socket from
//! here; the scaffold parses verbs so deep-link parity (plan §4) is testable.

use mouseless_cli::{CliError, Verb};
use std::process::ExitCode;

fn main() -> ExitCode {
    let arg = std::env::args().nth(1);
    match arg.as_deref().map(Verb::parse) {
        Some(Ok(verb)) => {
            println!("mouseless scaffold: {verb:?} ({})", verb.deep_link());
            println!("Phase 1.2 sends this over kanata TCP / daemon socket.");
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!("Error: {}", CliError::Usage);
            ExitCode::FAILURE
        }
    }
}
