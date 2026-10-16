//! `mouseless-overlay`: overlay backend abstraction.
//!
//! Draw-only (plan §1.5, SKILL-03): the GNOME Shell extension renders the
//! grid and reports geometry + cursor position. It never captures keys, never
//! touches evdev/uinput, never runs physics. All show/hide is idempotent so
//! Shell restarts and duplicate frames cannot desync the daemon.

pub mod impls;

use mouseless_ipc::CursorPos;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Overlay backend errors (logged, never crash the Shell).
#[derive(Debug, Error)]
pub enum OverlayError {
    /// Socket/connection failure (retry with backoff).
    #[error("overlay connection: {0}")]
    Connection(String),
    /// Peer speaks an unsupported schema (negotiate or refuse loudly).
    #[error("schema mismatch: {0}")]
    Schema(String),
}

/// Grid model pushed by the daemon (`Show(GridModel)`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GridModel {
    /// Monitor geometry the grid was computed for.
    pub geometry: serde_json::Value,
    /// Cell labels in row-major order.
    pub labels: Vec<String>,
    /// Highlighted cell index, if any.
    pub highlight: Option<u32>,
}

/// Swappable overlay backend (plan §1.8).
///
/// `gnome` (Unix-socket JSON) today; `wlr-layer-shell` and X11 follow in
/// Phase 4 (plan §4.2).
pub trait OverlayBackend {
    /// Error type.
    type Error: std::error::Error;
    /// Show the grid (idempotent).
    fn show(&mut self, model: &GridModel) -> Result<(), Self::Error>;
    /// Move the highlight (idempotent).
    fn highlight(&mut self, index: u32) -> Result<(), Self::Error>;
    /// Hide the overlay (idempotent, best-effort).
    fn hide(&mut self) -> Result<(), Self::Error>;
    /// Latest cursor position reported by the compositor, if any.
    fn last_cursor(&self) -> Option<CursorPos>;
}

#[cfg(test)]
mod tests {
    use super::impls::FakeOverlay;
    use super::*;

    #[test]
    fn show_hide_idempotent_never_fails() {
        let mut o = FakeOverlay::default();
        o.hide().unwrap(); // hiding while hidden: no-op
        o.show(&GridModel {
            geometry: serde_json::json!({}),
            labels: vec!["a".into()],
            highlight: None,
        })
        .unwrap();
        o.show(&GridModel {
            geometry: serde_json::json!({}),
            labels: vec!["a".into()],
            highlight: None,
        })
        .unwrap(); // duplicate show: no-op
        assert!(o.visible);
        o.hide().unwrap();
        assert!(!o.visible);
    }
}
