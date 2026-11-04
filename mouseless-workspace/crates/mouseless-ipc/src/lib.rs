//! `mouseless-ipc`: versioned JSON protocol, daemon ↔ overlay.
//!
//! Every frame carries `schema_version`. Mismatched peers negotiate or refuse
//! with a clear error — never silently misinterpret a frame (plan §3 rule 3,
//! audit G2/G3).
//!
//! Wire format: one JSON [`Envelope`] per line (newline-delimited JSON) over
//! the Unix socket at [`SOCKET_PATH`]. The full 1-page wire spec
//! (`specs/SPEC-ipc.md`) is the authoritative description of this crate.
//!
//! Direction is enforced twice: [`decode_for`] rejects frames whose `kind`
//! does not match the expected peer role, and the daemon authenticates peers
//! with [`peer_allowed`] (audit G3).

pub mod auth;

use mouseless_core::{Monitor, Rect};
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub use auth::{peer_allowed, AuthPolicy, PeerCred};

/// Current protocol schema version. Bump on any breaking change.
pub const SCHEMA_VERSION: u32 = 1;

/// Unix socket path (plan §4, audit G3).
///
/// Created by systemd via `RuntimeDirectory=mouseless` on the daemon unit,
/// mode `0770`, group `mouseless`. Peers authenticate with `SO_PEERCRED`.
pub const SOCKET_PATH: &str = "/run/mouseless/mouselessd.sock";

/// Maximum frame length accepted on a connection (SPEC-ipc transport).
pub const MAX_LINE_BYTES: usize = 1024 * 1024;

/// Sender role for the `kind` envelope field (SPEC-ipc).
pub const KIND_DAEMON: &str = "daemon";
/// Overlay → daemon role.
pub const KIND_OVERLAY: &str = "overlay";
/// CLI/GUI → daemon role (Phase 2/3).
pub const KIND_CLIENT: &str = "client";

/// Protocol errors: version mismatch, malformed frames.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum IpcError {
    /// Peer speaks a newer schema we cannot interpret.
    #[error("unsupported schema_version {got}, expected {expected}")]
    VersionMismatch {
        /// Version the peer sent.
        got: u32,
        /// Version we speak.
        expected: u32,
    },
    /// Frame is not valid JSON for [`Envelope`].
    #[error("malformed frame: {0}")]
    Malformed(String),
    /// Frame carries the wrong sender role for this direction.
    #[error("unexpected kind {got:?}, expected {expected:?}")]
    WrongKind {
        /// Role the peer claimed.
        got: String,
        /// Role this receiver accepts.
        expected: &'static str,
    },
    /// Frame exceeds [`MAX_LINE_BYTES`].
    #[error("frame too large: {0} bytes (max {MAX_LINE_BYTES})")]
    TooLarge(usize),
}

/// Newline-delimited envelope for every frame on the wire.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Envelope {
    /// Schema version of the sender.
    pub schema_version: u32,
    /// Message kind discriminator (sender role).
    pub kind: String,
    /// JSON payload (message-specific schema).
    pub payload: serde_json::Value,
}

/// One rendered cell (wire form). Bounds are global physical pixels computed
/// by `mouseless-core` — the extension MUST NOT re-derive them (SPEC-overlay).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GridCell {
    /// Row-major cell index (`CellId.row * cols + cell.col`).
    pub index: u32,
    /// Display label: one character for a subgrid cell, two for a primary
    /// cell (`column key + row key`, e.g. `AQ`).
    pub label: String,
    /// Left edge in physical px.
    pub x: f64,
    /// Top edge in physical px.
    pub y: f64,
    /// Width in physical px.
    pub w: f64,
    /// Height in physical px.
    pub h: f64,
}

impl GridCell {
    /// Build a cell from pure geometry + label.
    pub fn new(index: u32, label: impl Into<String>, rect: Rect) -> Self {
        Self {
            index,
            label: label.into(),
            x: rect.x,
            y: rect.y,
            w: rect.w,
            h: rect.h,
        }
    }
}

/// Full grid model pushed by `Show`/`ShowSubgrid` (SPEC-ipc).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GridModel {
    /// Monitor the grid was computed for (physical global coordinates).
    pub monitor: Monitor,
    /// Cells in row-major order.
    pub cells: Vec<GridCell>,
    /// Highlighted cell index, if any.
    pub highlight: Option<u32>,
}

/// One monitor in a [`MonitorLayout`].
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MonitorInfo {
    /// Geometry + scale.
    #[serde(flatten)]
    pub monitor: Monitor,
    /// Whether this is the primary monitor.
    #[serde(default)]
    pub primary: bool,
}

/// Monitor layout reported by the extension (SPEC-ipc `Report`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct MonitorLayout {
    /// All connected monitors in compositor order.
    pub monitors: Vec<MonitorInfo>,
}

impl MonitorLayout {
    /// Primary monitor, or the first one when none is flagged primary.
    pub fn primary(&self) -> Option<Monitor> {
        self.monitors
            .iter()
            .find(|m| m.primary)
            .or_else(|| self.monitors.first())
            .map(|m| m.monitor)
    }

    /// Monitor containing `cursor` (global physical px), or the primary.
    pub fn monitor_at(&self, cursor: CursorPos) -> Option<Monitor> {
        let (x, y) = (cursor.x as f64, cursor.y as f64);
        self.monitors
            .iter()
            .map(|m| m.monitor)
            .find(|m| {
                x >= m.x as f64
                    && x < (m.x as f64 + m.w as f64)
                    && y >= m.y as f64
                    && y < (m.y as f64 + m.h as f64)
            })
            .or_else(|| self.primary())
    }
}

/// Messages daemon → overlay (plan §4 contracts).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum DaemonToOverlay {
    /// Show the overlay with a full grid model.
    Show {
        /// Grid model.
        model: GridModel,
    },
    /// Move the highlight to a cell.
    HighlightCell {
        /// Cell index in row-major order.
        index: u32,
    },
    /// Show the subgrid inside a cell.
    ShowSubgrid {
        /// Parent cell index.
        parent: u32,
        /// Subgrid model.
        model: GridModel,
    },
    /// Hide the overlay (idempotent).
    Hide,
    /// Request a fresh cursor position before a click.
    QueryCursor,
    /// Protocol-level error (e.g. version mismatch); receiver logs and may close.
    Error {
        /// Machine-readable code (`version_mismatch`, …).
        code: String,
        /// Human-readable message.
        message: String,
        /// Version the sender got (optional).
        got: Option<u32>,
        /// Version the sender expected (optional).
        expected: Option<u32>,
    },
}

/// Messages overlay → daemon (plan §4 contracts).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum OverlayToDaemon {
    /// Overlay is ready / Shell restarted (resync hint). MUST be first frame.
    Hello {
        /// Extension version string.
        extension_version: String,
    },
    /// Monitor geometry + cursor position report.
    Report {
        /// Monitor layout.
        geometry: MonitorLayout,
        /// Cursor position in global physical pixels.
        cursor: CursorPos,
    },
    /// Reply to [`DaemonToOverlay::QueryCursor`].
    CursorReply {
        /// X in global physical pixels.
        x: i32,
        /// Y in global physical pixels.
        y: i32,
    },
}

/// Cursor position in global physical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CursorPos {
    /// X in global physical pixels.
    pub x: i32,
    /// Y in global physical pixels.
    pub y: i32,
}

impl From<CursorPos> for (i32, i32) {
    fn from(c: CursorPos) -> Self {
        (c.x, c.y)
    }
}

/// Wrap a message in a versioned [`Envelope`].
pub fn encode<T: Serialize>(kind: &str, msg: &T) -> Result<String, IpcError> {
    let payload = serde_json::to_value(msg).map_err(|e| IpcError::Malformed(e.to_string()))?;
    let env = Envelope {
        schema_version: SCHEMA_VERSION,
        kind: kind.to_string(),
        payload,
    };
    serde_json::to_string(&env).map_err(|e| IpcError::Malformed(e.to_string()))
}

/// Decode one line, enforcing the schema version and [`MAX_LINE_BYTES`].
///
/// Older peers are accepted (forward-compatible reads); newer peers are
/// refused loudly so a mismatch can never silently corrupt the overlay.
pub fn decode(line: &str) -> Result<Envelope, IpcError> {
    if line.len() > MAX_LINE_BYTES {
        return Err(IpcError::TooLarge(line.len()));
    }
    let env: Envelope =
        serde_json::from_str(line).map_err(|e| IpcError::Malformed(e.to_string()))?;
    if env.schema_version > SCHEMA_VERSION {
        return Err(IpcError::VersionMismatch {
            got: env.schema_version,
            expected: SCHEMA_VERSION,
        });
    }
    Ok(env)
}

/// Decode one line and enforce the expected sender role (SPEC-ipc `kind`).
///
/// A mismatched kind is dropped by the caller, never fatal to the connection.
pub fn decode_for(line: &str, expected_kind: &'static str) -> Result<Envelope, IpcError> {
    let env = decode(line)?;
    if env.kind != expected_kind {
        return Err(IpcError::WrongKind {
            got: env.kind,
            expected: expected_kind,
        });
    }
    Ok(env)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn monitor_1080p() -> Monitor {
        Monitor {
            x: 0,
            y: 0,
            w: 1920,
            h: 1080,
            scale: 1.0,
        }
    }

    fn model() -> GridModel {
        GridModel {
            monitor: monitor_1080p(),
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
        }
    }

    #[test]
    fn round_trip_show_hide() {
        for msg in [
            DaemonToOverlay::Show { model: model() },
            DaemonToOverlay::Hide,
            DaemonToOverlay::HighlightCell { index: 3 },
            DaemonToOverlay::ShowSubgrid {
                parent: 1,
                model: model(),
            },
            DaemonToOverlay::QueryCursor,
            DaemonToOverlay::Error {
                code: "version_mismatch".into(),
                message: "peer too new".into(),
                got: Some(2),
                expected: Some(1),
            },
        ] {
            let line = encode(KIND_DAEMON, &msg).unwrap();
            let env = decode_for(&line, KIND_DAEMON).unwrap();
            assert_eq!(env.schema_version, SCHEMA_VERSION);
            let decoded: DaemonToOverlay = serde_json::from_value(env.payload).unwrap();
            assert_eq!(decoded, msg);
        }
    }

    #[test]
    fn report_round_trip_with_typed_layout() {
        let msg = OverlayToDaemon::Report {
            geometry: MonitorLayout {
                monitors: vec![MonitorInfo {
                    monitor: monitor_1080p(),
                    primary: true,
                }],
            },
            cursor: CursorPos { x: 100, y: 200 },
        };
        let line = encode(KIND_OVERLAY, &msg).unwrap();
        assert!(line.ends_with('}'));
        assert!(!line.contains('\n'));
        let env = decode_for(&line, KIND_OVERLAY).unwrap();
        let decoded: OverlayToDaemon = serde_json::from_value(env.payload).unwrap();
        assert_eq!(decoded, msg);

        let reply = OverlayToDaemon::CursorReply { x: 640, y: 360 };
        let line = encode(KIND_OVERLAY, &reply).unwrap();
        let env = decode_for(&line, KIND_OVERLAY).unwrap();
        let decoded: OverlayToDaemon = serde_json::from_value(env.payload).unwrap();
        assert_eq!(decoded, reply);
    }

    #[test]
    fn layout_selects_monitor_at_cursor_then_primary() {
        let layout = MonitorLayout {
            monitors: vec![
                MonitorInfo {
                    monitor: Monitor {
                        x: 0,
                        y: 0,
                        w: 1920,
                        h: 1080,
                        scale: 1.0,
                    },
                    primary: false,
                },
                MonitorInfo {
                    monitor: Monitor {
                        x: 1920,
                        y: 0,
                        w: 1080,
                        h: 1920,
                        scale: 1.25,
                    },
                    primary: true,
                },
            ],
        };
        assert_eq!(
            layout.monitor_at(CursorPos { x: 10, y: 10 }).unwrap().w,
            1920
        );
        assert_eq!(
            layout.monitor_at(CursorPos { x: 2000, y: 10 }).unwrap().w,
            1080
        );
        // Outside every monitor: fall back to primary.
        assert_eq!(
            layout.monitor_at(CursorPos { x: -50, y: -50 }).unwrap().w,
            1080
        );
    }

    #[test]
    fn newer_schema_refused_loudly() {
        let line = json!({
            "schema_version": SCHEMA_VERSION + 1,
            "kind": "daemon",
            "payload": { "type": "Hide" },
        })
        .to_string();
        assert_eq!(
            decode(&line),
            Err(IpcError::VersionMismatch {
                got: SCHEMA_VERSION + 1,
                expected: SCHEMA_VERSION,
            })
        );
    }

    #[test]
    fn wrong_kind_is_rejected_not_fatal() {
        let line = encode(KIND_OVERLAY, &OverlayToDaemon::CursorReply { x: 1, y: 2 }).unwrap();
        assert_eq!(
            decode_for(&line, KIND_DAEMON),
            Err(IpcError::WrongKind {
                got: KIND_OVERLAY.into(),
                expected: KIND_DAEMON,
            })
        );
        // The base decoder still accepts it (caller decides to log + skip).
        assert!(decode(&line).is_ok());
    }

    #[test]
    fn oversized_frame_refused() {
        let huge = "x".repeat(MAX_LINE_BYTES + 1);
        assert_eq!(decode(&huge), Err(IpcError::TooLarge(MAX_LINE_BYTES + 1)));
    }

    #[test]
    fn malformed_frames_never_panic() {
        // Curated fuzz corpus (blocking matrix #8): empty, truncated, wrong
        // types, and every prefix of a valid frame.
        let corpus = [
            "",
            "{",
            "not json",
            "[1,2",
            "null",
            "42",
            r#"{"schema_version":1}"#,
            r#"{"schema_version":1,"kind":"daemon"}"#,
            r#"{"schema_version":"one","kind":"daemon","payload":{}}"#,
            r#"{"schema_version":1,"kind":null,"payload":{}}"#,
        ];
        for bad in corpus {
            assert!(matches!(decode(bad), Err(IpcError::Malformed(_))), "{bad}");
        }
        let line = encode(KIND_DAEMON, &DaemonToOverlay::Hide).unwrap();
        for i in 0..line.len() {
            let truncated = &line[..i];
            assert!(decode(truncated).is_err());
        }
    }

    #[test]
    fn checked_in_corpus_never_panics() {
        // Matrix #8: the curated corpus ships as test data (SPEC-ipc).
        let corpus = include_str!("../tests/corpus/malformed.ndjson");
        for line in corpus.lines() {
            assert!(
                matches!(decode(line), Err(IpcError::Malformed(_))),
                "{line:?}"
            );
        }
    }
}
