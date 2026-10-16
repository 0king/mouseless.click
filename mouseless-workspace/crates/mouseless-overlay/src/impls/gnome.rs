//! GNOME Shell overlay constants (Phase 1.3 implements the extension).
//!
//! Draw-only Clutter actor via
//! `Main.layoutManager.addTopChrome(actor, { affectsInputRegion: false,
//! trackFullscreen: true })`; GNOME 45+ ESM syntax (audit C2).

/// Supported Shell versions (`metadata.json`).
pub const GNOME_SHELL_VERSIONS: &[&str] = &["45", "46", "47", "48", "49", "50"];

/// Unix socket path (mirrors `mouseless-ipc::SOCKET_PATH`, audit G3).
pub const SOCKET_PATH: &str = "/run/mouseless/mouselessd.sock";
