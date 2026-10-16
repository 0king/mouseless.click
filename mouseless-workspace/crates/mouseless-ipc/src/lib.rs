//! `mouseless-ipc`: versioned JSON protocol, daemon ↔ overlay.
//!
//! Every frame carries `schema_version`. Mismatched peers negotiate or refuse
//! with a clear error — never silently misinterpret a frame (plan §3 rule 3,
//! audit G2/G3).
//!
//! Wire format: one JSON [`Envelope`] per line (newline-delimited JSON) over
//! the Unix socket at [`SOCKET_PATH`]. The full 1-page wire spec (hello /
//! negotiate → stream → close, keepalive, reconnect) is a Phase-1 pre-coding
//! deliverable; this crate is its executable core.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Current protocol schema version. Bump on any breaking change.
pub const SCHEMA_VERSION: u32 = 1;

/// Unix socket path (plan §4, audit G3).
///
/// Created by systemd via `RuntimeDirectory=mouseless` on the daemon unit,
/// mode `0770`, group `mouseless`. Peers authenticate with `SO_PEERCRED`.
pub const SOCKET_PATH: &str = "/run/mouseless/mouselessd.sock";

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
}

/// Newline-delimited envelope for every frame on the wire.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Envelope {
    /// Schema version of the sender.
    pub schema_version: u32,
    /// Message kind discriminator.
    pub kind: String,
    /// JSON payload (message-specific schema).
    pub payload: serde_json::Value,
}

/// Messages daemon → overlay (plan §4 contracts).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum DaemonToOverlay {
    /// Show the overlay with a full grid model.
    Show {
        /// Opaque grid model (cells, labels, highlight).
        model: serde_json::Value,
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
        /// Opaque subgrid model.
        model: serde_json::Value,
    },
    /// Hide the overlay (idempotent).
    Hide,
}

/// Messages overlay → daemon (plan §4 contracts).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum OverlayToDaemon {
    /// Monitor geometry + cursor position report.
    Report {
        /// Monitor geometry.
        geometry: serde_json::Value,
        /// Cursor position in global physical pixels.
        cursor: CursorPos,
    },
    /// Overlay is ready / Shell restarted (resync hint).
    Hello {
        /// Extension version string.
        extension_version: String,
    },
}

/// Cursor position in global physical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CursorPos {
    /// X in global physical pixels.
    pub x: i32,
    /// Y in global physical pixels.
    pub y: i32,
}

/// Wrap a message in a versioned [`Envelope`].
pub fn encode<T: Serialize>(kind: &str, msg: &T) -> Result<String, IpcError> {
    let payload =
        serde_json::to_value(msg).map_err(|e| IpcError::Malformed(e.to_string()))?;
    let env = Envelope {
        schema_version: SCHEMA_VERSION,
        kind: kind.to_string(),
        payload,
    };
    serde_json::to_string(&env).map_err(|e| IpcError::Malformed(e.to_string()))
}

/// Decode one line, enforcing the schema version.
///
/// Older peers are accepted (forward-compatible reads); newer peers are
/// refused loudly so a mismatch can never silently corrupt the overlay.
pub fn decode(line: &str) -> Result<Envelope, IpcError> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn round_trip_show_hide() {
        let line = encode(
            "daemon",
            &DaemonToOverlay::Show {
                model: json!({"cells": 6}),
            },
        )
        .unwrap();
        let env = decode(&line).unwrap();
        assert_eq!(env.schema_version, SCHEMA_VERSION);
        let msg: DaemonToOverlay = serde_json::from_value(env.payload).unwrap();
        assert_eq!(
            msg,
            DaemonToOverlay::Show {
                model: json!({"cells": 6})
            }
        );

        let line = encode("daemon", &DaemonToOverlay::Hide).unwrap();
        let env = decode(&line).unwrap();
        let msg: DaemonToOverlay = serde_json::from_value(env.payload).unwrap();
        assert_eq!(msg, DaemonToOverlay::Hide);
    }

    #[test]
    fn report_round_trip() {
        let line = encode(
            "overlay",
            &OverlayToDaemon::Report {
                geometry: json!({"w": 1920}),
                cursor: CursorPos { x: 100, y: 200 },
            },
        )
        .unwrap();
        assert!(line.ends_with('}'));
        assert!(!line.contains('\n'));
        let env = decode(&line).unwrap();
        let msg: OverlayToDaemon = serde_json::from_value(env.payload).unwrap();
        assert_eq!(
            msg,
            OverlayToDaemon::Report {
                geometry: json!({"w": 1920}),
                cursor: CursorPos { x: 100, y: 200 }
            }
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
    fn malformed_frames_never_panic() {
        for bad in ["", "{", "not json", "[1,2", "null", "42"] {
            assert!(matches!(decode(bad), Err(IpcError::Malformed(_))), "{bad}");
        }
        // Fuzz corpus seed: truncated valid frame.
        let line = encode("daemon", &DaemonToOverlay::Hide).unwrap();
        for i in 0..line.len() {
            let truncated = &line[..i];
            assert!(decode(truncated).is_err());
        }
    }
}
