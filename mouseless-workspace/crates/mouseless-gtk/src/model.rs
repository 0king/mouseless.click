//! Pure helpers shared by the settings pages.
//!
//! Everything here is toolkit-free on purpose: paths, monitor parsing, key
//! hints, the kanata-check decision, and status gathering can be unit-tested
//! without initializing GTK. The widgets live in [`crate::form`] and
//! [`crate::pages`].

use mouseless_config::{
    parse_hex_color, render_include, CheckError, Config, KanataCheck, COLUMN_KEY_POOL, ROW_KEY_POOL,
};
use mouseless_ipc::{request, ClientToDaemon, DaemonToClient, CLIENT_TIMEOUT, SOCKET_PATH};
use mouseless_kbd::{query_current_layer, KanataTcpConfig};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Default daemon state directory (`StateDirectory=mouseless`).
pub const DEFAULT_STATE_DIR: &str = "/var/lib/mouseless";
/// Kanata main config that includes the generated fragment.
pub const KANATA_MAIN_CONFIG: &str = "/etc/kanata/unicfg.kbd";

/// Daemon socket path (`MOUSELESS_SOCKET` overrides it for development).
pub fn socket_path() -> PathBuf {
    std::env::var_os("MOUSELESS_SOCKET")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(SOCKET_PATH))
}

/// Daemon state directory (`MOUSELESS_STATE_DIR` overrides it for development).
pub fn state_dir() -> PathBuf {
    std::env::var_os("MOUSELESS_STATE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_STATE_DIR))
}

/// Kanata TCP endpoint (`MOUSELESS_KANATA` overrides it, as in the CLI).
pub fn kanata_config() -> KanataTcpConfig {
    let addr = std::env::var("MOUSELESS_KANATA").unwrap_or_else(|_| "127.0.0.1:7070".to_string());
    let (host, port) = addr.rsplit_once(':').unwrap_or(("127.0.0.1", "7070"));
    KanataTcpConfig {
        host: host.to_string(),
        port: port.parse().unwrap_or(7070),
        timeout: Duration::from_secs(2),
    }
}

/// Split a monitor list on commas and/or whitespace.
pub fn split_monitors(text: &str) -> Vec<String> {
    text.split(|c: char| c == ',' || c.is_whitespace())
        .filter(|part| !part.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

/// Render a monitor list for the entry row.
pub fn join_monitors(monitors: &[String]) -> String {
    monitors.join(", ")
}

/// The kanata include only changes when tap timing or key roles change.
///
/// Grid columns/rows are interpreted by the daemon and appear only in a
/// comment in the generated include, so editing them must not pay the cost of
/// a `kanata --check` run.
pub fn needs_kanata_check(previous: &Config, candidate: &Config) -> bool {
    previous.behavior.tap_threshold_ms != candidate.behavior.tap_threshold_ms
        || previous.keybindings != candidate.keybindings
}

/// Live hint for a cell-key field (count, pool, duplicates).
pub fn key_hint(text: &str, pool: &str, expected: Option<usize>) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut seen: Vec<char> = Vec::new();
    let mut duplicates: Vec<char> = Vec::new();
    let mut invalid: Vec<char> = Vec::new();
    for ch in &chars {
        if !pool.contains(*ch) {
            if !invalid.contains(ch) {
                invalid.push(*ch);
            }
        } else if seen.contains(ch) {
            if !duplicates.contains(ch) {
                duplicates.push(*ch);
            }
        } else {
            seen.push(*ch);
        }
    }

    let mut parts = vec![format!(
        "{} key{}",
        chars.len(),
        if chars.len() == 1 { "" } else { "s" }
    )];
    if let Some(expected) = expected {
        parts.push(format!("needs {expected}"));
    }
    if !invalid.is_empty() {
        parts.push(format!("outside pool: {}", collect(&invalid)));
    }
    if !duplicates.is_empty() {
        parts.push(format!("duplicates: {}", collect(&duplicates)));
    }
    if invalid.is_empty() && duplicates.is_empty() {
        parts.push("unique".into());
    }
    parts.join(" · ")
}

/// Hint for the two configurable key pools.
pub fn pool_hint(text: &str, pool: &str) -> String {
    key_hint(text, pool, None)
}

/// Hint for `grid.columns`.
pub fn columns_hint(text: &str) -> String {
    pool_hint(text, COLUMN_KEY_POOL)
}

/// Hint for `grid.rows`.
pub fn rows_hint(text: &str) -> String {
    pool_hint(text, ROW_KEY_POOL)
}

/// Hint for `grid.subgrid.keys` (`columns * rows` expected).
pub fn subgrid_hint(keys: &str, columns: u32, rows: u32) -> String {
    let expected = columns.saturating_mul(rows) as usize;
    key_hint(keys, ROW_KEY_POOL, Some(expected))
}

fn collect(chars: &[char]) -> String {
    chars.iter().collect()
}

/// Parse `#rrggbb` into 0.0–1.0 channels for GTK colours.
pub fn rgb_channels(hex: &str) -> [f64; 3] {
    parse_hex_color(hex)
        .map(|[r, g, b]| {
            [
                f64::from(r) / 255.0,
                f64::from(g) / 255.0,
                f64::from(b) / 255.0,
            ]
        })
        .unwrap_or([0.0, 0.0, 0.0])
}

/// Format 0.0–1.0 channels as `#rrggbb` (clamped).
pub fn hex_from_channels(rgb: [f64; 3]) -> String {
    let channel = |value: f64| (value.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!(
        "#{:02x}{:02x}{:02x}",
        channel(rgb[0]),
        channel(rgb[1]),
        channel(rgb[2])
    )
}

/// Find an executable in `PATH` without spawning it.
pub fn find_in_path(program: &str, path_var: Option<&OsStr>) -> Option<PathBuf> {
    let path_var = path_var?;
    for dir in std::env::split_paths(path_var) {
        let candidate = dir.join(program);
        if candidate.is_file() {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Ok(meta) = candidate.metadata() {
                    if meta.permissions().mode() & 0o111 != 0 {
                        return Some(candidate);
                    }
                }
            }
            #[cfg(not(unix))]
            return Some(candidate);
        }
    }
    None
}

/// Run the `kanata --check` gate for a kanata-affecting edit.
///
/// `Ok(warnings)` means the candidate may be applied (warnings explain a
/// skipped check). `Err` means kanata rejected the include and the candidate
/// must not be pushed to the daemon.
pub fn check_kanata_candidate(
    previous: &Config,
    candidate: &Config,
) -> Result<Vec<String>, String> {
    if !needs_kanata_check(previous, candidate) {
        return Ok(Vec::new());
    }
    let main = Path::new(KANATA_MAIN_CONFIG);
    if !main.is_file() {
        return Ok(vec![format!(
            "skipped kanata check: {} not found",
            main.display()
        )]);
    }
    if find_in_path("kanata", std::env::var_os("PATH").as_deref()).is_none() {
        return Ok(vec![
            "skipped kanata check: kanata binary not found".to_string()
        ]);
    }
    let checker = KanataCheck::new("kanata", main);
    match checker.check(&render_include(candidate)) {
        Ok(()) => Ok(Vec::new()),
        Err(error @ CheckError::Rejected { .. }) => Err(error.to_string()),
        Err(CheckError::Setup(message)) => Ok(vec![format!("skipped kanata check: {message}")]),
        Err(error @ CheckError::Spawn(_)) => Ok(vec![format!("skipped kanata check: {error}")]),
    }
}

/// One daemon status reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaemonStatus {
    /// Package version reported by the daemon.
    pub version: String,
    /// Whether an overlay is connected.
    pub overlay_connected: bool,
    /// Whether a session is active.
    pub session_active: bool,
    /// Whether custom free mode is running.
    pub free_mode: bool,
    /// Daemon-owned config path.
    pub config_path: String,
}

/// Complete status snapshot shown by the Status page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusSnapshot {
    /// Daemon reachability + state.
    pub daemon: Result<DaemonStatus, String>,
    /// Current kanata layer, or the connection error.
    pub kanata_layer: Result<String, String>,
    /// Whether `/dev/uinput` exists.
    pub uinput_present: bool,
}

/// Query daemon + kanata + uinput state (blocking; call off the UI thread).
pub fn gather_status(socket: PathBuf) -> StatusSnapshot {
    let daemon = match request(&socket, &ClientToDaemon::Status, CLIENT_TIMEOUT) {
        Ok(DaemonToClient::Status {
            daemon_version,
            overlay_connected,
            session_active,
            free_mode,
            config_path,
        }) => Ok(DaemonStatus {
            version: daemon_version,
            overlay_connected,
            session_active,
            free_mode,
            config_path,
        }),
        Ok(DaemonToClient::Error { code, message }) => Err(format!("{code}: {message}")),
        Ok(other) => Err(format!("unexpected daemon reply: {other:?}")),
        Err(error) => Err(error.to_string()),
    };
    let kanata_layer = query_current_layer(&kanata_config()).map_err(|error| error.to_string());
    StatusSnapshot {
        daemon,
        kanata_layer,
        uinput_present: Path::new("/dev/uinput").exists(),
    }
}

/// Parse a `GetConfig` reply into the live document.
pub fn config_from_reply(reply: DaemonToClient) -> Result<Config, String> {
    match reply {
        DaemonToClient::Config { config } => Ok(*config),
        DaemonToClient::Error { code, message } => Err(format!("{code}: {message}")),
        other => Err(format!("unexpected daemon reply: {other:?}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mouseless_config::DefaultAction;

    #[test]
    fn monitor_lists_round_trip() {
        assert_eq!(
            split_monitors(" DP-1, HDMI-1  eDP-1 "),
            vec!["DP-1", "HDMI-1", "eDP-1"]
        );
        assert!(split_monitors("  , ").is_empty());
        assert_eq!(
            join_monitors(&["DP-1".into(), "HDMI-1".into()]),
            "DP-1, HDMI-1"
        );
    }

    #[test]
    fn kanata_check_only_runs_for_include_affecting_fields() {
        let previous = Config::default();
        let mut candidate = previous.clone();
        candidate.grid.columns = "asdf".into();
        candidate.grid.rows = "qwer".into();
        candidate.grid.subgrid.keys = "qwert".into();
        candidate.behavior.nudges_per_cell = 8;
        assert!(!needs_kanata_check(&previous, &candidate));

        candidate.behavior.tap_threshold_ms = 250;
        assert!(needs_kanata_check(&previous, &candidate));

        let mut renamed = previous.clone();
        renamed.keybindings.overlay_layer = "lyr-other".into();
        assert!(needs_kanata_check(&previous, &renamed));
    }

    #[test]
    fn key_hints_report_count_pool_and_duplicates() {
        assert_eq!(columns_hint("asdf"), "4 keys · unique");
        assert_eq!(columns_hint("asdz"), "4 keys · outside pool: z");
        assert_eq!(columns_hint("asda"), "4 keys · duplicates: a");
        assert_eq!(subgrid_hint("qwert", 10, 3), "5 keys · needs 30 · unique");
        assert_eq!(subgrid_hint("qq", 1, 2), "2 keys · needs 2 · duplicates: q");
    }

    #[test]
    fn color_channels_round_trip() {
        let channels = rgb_channels("#d4de04");
        assert!((channels[0] - 212.0 / 255.0).abs() < f64::EPSILON);
        assert_eq!(hex_from_channels(channels), "#d4de04");
        assert_eq!(hex_from_channels([2.0, -1.0, 0.5]), "#ff0080");
        assert_eq!(rgb_channels("not-a-color"), [0.0, 0.0, 0.0]);
    }

    #[test]
    fn path_lookup_requires_an_executable_bit() {
        let dir = std::env::temp_dir().join(format!(
            "mouseless-gtk-path-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let plain = dir.join("plain");
        std::fs::write(&plain, b"not executable").unwrap();
        let path = Some(dir.as_os_str());
        assert_eq!(find_in_path("plain", path), None);

        let executable = dir.join("tool");
        std::fs::write(&executable, b"#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        assert_eq!(find_in_path("tool", path), Some(executable));
        assert_eq!(find_in_path("tool", None), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn status_snapshot_keeps_subsystem_errors_separate() {
        let snapshot = StatusSnapshot {
            daemon: Err("unreachable".into()),
            kanata_layer: Ok("lyr-base".into()),
            uinput_present: true,
        };
        assert_eq!(snapshot.kanata_layer.as_deref(), Ok("lyr-base"));
        assert!(snapshot.daemon.is_err());
        assert!(snapshot.uinput_present);
    }

    #[test]
    fn preset_behavior_is_not_confused_with_other_sections() {
        let mut config = Config::default();
        config.behavior.default_action = DefaultAction::Move;
        assert_eq!(config.behavior.default_action, DefaultAction::Move);
        assert_eq!(config.grid.columns, "asdfghjkl;");
    }
}
