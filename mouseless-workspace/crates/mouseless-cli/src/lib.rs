//! `mouseless-cli` library: verbs shared by the binary and deep links.
//!
//! Parity requirement (plan §4): every `mouseless://` deep link maps 1:1 to a
//! CLI verb (`mouseless://toggle-overlay` ↔ `mouseless toggle-overlay`).

use thiserror::Error;

/// CLI errors (surfaced with a clear message, never a stack trace).
#[derive(Debug, Error, PartialEq, Eq)]
pub enum CliError {
    /// Unknown verb or bad arguments.
    #[error("usage: mouseless <show|hide|toggle|enter-free-mode|exit-free-mode|status|--list-input-devices>")]
    Usage,
}

/// Verbs with exact deep-link parity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verb {
    /// `mouseless://show-overlay`.
    Show,
    /// `mouseless://hide-overlay`.
    Hide,
    /// `mouseless://toggle-overlay`.
    Toggle,
    /// `mouseless://enter-free-mode`.
    EnterFreeMode,
    /// `mouseless://exit-free-mode`.
    ExitFreeMode,
    /// Local status (no daemon round-trip in the scaffold).
    Status,
    /// List `/dev/input` devices (mirrors upstream `--list-input-devices`).
    ListInputDevices,
}

impl Verb {
    /// Parse `argv[1]` into a verb.
    pub fn parse(arg: &str) -> Result<Self, CliError> {
        match arg {
            "show" | "show-overlay" => Ok(Verb::Show),
            "hide" | "hide-overlay" => Ok(Verb::Hide),
            "toggle" | "toggle-overlay" => Ok(Verb::Toggle),
            "enter-free-mode" => Ok(Verb::EnterFreeMode),
            "exit-free-mode" => Ok(Verb::ExitFreeMode),
            "status" => Ok(Verb::Status),
            "--list-input-devices" => Ok(Verb::ListInputDevices),
            _ => Err(CliError::Usage),
        }
    }

    /// Deep-link URL for this verb (`mouseless://…`).
    pub fn deep_link(&self) -> &'static str {
        match self {
            Verb::Show => "mouseless://show-overlay",
            Verb::Hide => "mouseless://hide-overlay",
            Verb::Toggle => "mouseless://toggle-overlay",
            Verb::EnterFreeMode => "mouseless://enter-free-mode",
            Verb::ExitFreeMode => "mouseless://exit-free-mode",
            Verb::Status => "mouseless://status",
            Verb::ListInputDevices => "mouseless://list-input-devices",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verbs_have_deep_link_parity() {
        for (arg, link) in [
            ("show", "mouseless://show-overlay"),
            ("hide", "mouseless://hide-overlay"),
            ("toggle", "mouseless://toggle-overlay"),
            ("enter-free-mode", "mouseless://enter-free-mode"),
            ("exit-free-mode", "mouseless://exit-free-mode"),
        ] {
            assert_eq!(Verb::parse(arg).unwrap().deep_link(), link);
        }
    }

    #[test]
    fn unknown_verb_prints_usage() {
        assert_eq!(Verb::parse("frobnicate"), Err(CliError::Usage));
    }
}
