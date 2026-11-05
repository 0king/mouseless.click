//! `mouseless-overlay`: overlay backend abstraction.
//!
//! Draw-only (plan §1.5, SKILL-03): the GNOME Shell extension renders the
//! grid and reports geometry + cursor position. It never captures keys, never
//! touches evdev/uinput, never runs physics. All show/hide is idempotent so
//! Shell restarts and duplicate frames cannot desync the daemon.

pub mod impls;

use mouseless_ipc::CursorPos;
use thiserror::Error;

pub use mouseless_ipc::{
    ClientToDaemon, DaemonToClient, GridCell, GridModel, MonitorInfo, MonitorLayout,
};

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

/// Events reported by the overlay (extension → daemon).
#[derive(Debug, Clone, PartialEq)]
pub enum OverlayEvent {
    /// Extension connected and completed the Hello handshake.
    Hello {
        /// Extension version string.
        extension_version: String,
    },
    /// Monitor/cursor report (on connect and geometry changes).
    Report {
        /// Current monitor layout.
        layout: MonitorLayout,
        /// Cursor position in global physical pixels.
        cursor: CursorPos,
    },
    /// Reply to a `QueryCursor` request.
    CursorReply {
        /// Fresh cursor position.
        cursor: CursorPos,
    },
    /// A previously-helloed client disconnected (Shell restart/crash).
    Disconnected,
    /// A CLI/GUI client sent `Reload`/`Update`. The daemon applies it and
    /// replies with [`OverlayBackend::reply_client`].
    ClientCommand {
        /// Request identifier used to address the reply.
        request_id: u64,
        /// Validated client frame (payload parsed, schema checked).
        command: ClientToDaemon,
    },
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
    /// Fade every primary label except the selected column's.
    ///
    /// `column` is `None` to clear the emphasis (e.g. when the subgrid opens);
    /// `cols` is the primary-grid column count. Idempotent.
    fn highlight_column(&mut self, column: Option<u32>, cols: u32) -> Result<(), Self::Error>;
    /// Show the subgrid inside parent cell `parent` (idempotent).
    fn show_subgrid(&mut self, parent: u32, model: &GridModel) -> Result<(), Self::Error>;
    /// Hide the overlay (idempotent, best-effort).
    fn hide(&mut self) -> Result<(), Self::Error>;
    /// Ask the compositor for a fresh cursor position (`QueryCursor`).
    fn query_cursor(&mut self) -> Result<(), Self::Error>;
    /// Drain queued overlay events without blocking.
    fn poll_events(&mut self) -> Vec<OverlayEvent>;
    /// Reply to a [`OverlayEvent::ClientCommand`] request.
    ///
    /// Default is a no-op so draw-only/fake backends stay source-compatible.
    fn reply_client(&mut self, request_id: u64, reply: &DaemonToClient) -> Result<(), Self::Error> {
        let _ = (request_id, reply);
        Ok(())
    }
    /// Latest cursor position reported by the compositor, if any.
    fn last_cursor(&self) -> Option<CursorPos>;
    /// Latest monitor layout reported by the compositor, if any.
    fn last_layout(&self) -> Option<MonitorLayout>;
    /// Whether a live overlay client has completed the Hello handshake.
    ///
    /// The daemon MUST NOT grab cell keys without a live overlay: the keys
    /// would be swallowed with nowhere to render (plan §6 degraded mode).
    fn is_connected(&self) -> bool;
}

#[cfg(test)]
mod tests {
    use super::impls::FakeOverlay;
    use super::*;
    use mouseless_core::{Monitor, Rect};

    fn model() -> GridModel {
        GridModel {
            monitor: Monitor {
                x: 0,
                y: 0,
                w: 1920,
                h: 1080,
                scale: 1.0,
            },
            cells: vec![GridCell::new(
                0,
                "a",
                Rect {
                    x: 0.0,
                    y: 0.0,
                    w: 480.0,
                    h: 540.0,
                },
            )],
            highlight: None,
            appearance: Default::default(),
        }
    }

    #[test]
    fn show_hide_idempotent_never_fails() {
        let mut o = FakeOverlay::default();
        o.hide().unwrap(); // hiding while hidden: no-op
        o.show(&model()).unwrap();
        o.show(&model()).unwrap(); // duplicate show: no-op
        assert!(o.visible);
        o.hide().unwrap();
        assert!(!o.visible);
    }
}
