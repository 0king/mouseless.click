//! Client → daemon messages (`Reload`/`Update`) and the blocking helper used
//! by the CLI and settings GUI.
//!
//! Clients are one-shot peers: connect, send exactly one NDJSON frame, read
//! exactly one reply, close. They do not send `Hello` (that lifecycle belongs
//! to overlays) and the daemon must never broadcast overlay frames to them.
//! The wire shapes are pinned by `specs/SPEC-ipc.md`.

use crate::{decode_for, encode, IpcError, KIND_CLIENT, KIND_DAEMON, MAX_LINE_BYTES};
use mouseless_config::Config;
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

/// Default budget for one request/reply round-trip.
///
/// The daemon replies after validating + persisting the config, which is a
/// few local file operations; one second is generous on a healthy host while
/// still failing fast when the daemon is wedged.
pub const CLIENT_TIMEOUT: Duration = Duration::from_secs(1);

/// Messages client → daemon (kind `"client"`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ClientToDaemon {
    /// Re-read the daemon-owned config, validate, and apply it.
    Reload,
    /// Apply a complete config document (the GUI is the sole writer).
    Update {
        /// Full schema-v1 config (see `mouseless-config`).
        ///
        /// Boxed because the document is much larger than the other variants;
        /// the wire shape is unchanged (serde flattens the box).
        config: Box<Config>,
    },
    /// Read daemon/overlay state (read-only).
    Status,
    /// Read the effective config document (read-only).
    ///
    /// The settings GUI edits the daemon-owned document rather than keeping a
    /// shadow copy, so it always starts from the values the daemon resolves
    /// (live file → last-good → defaults). Persistence still belongs to the
    /// daemon (`Update`).
    GetConfig,
}

/// Messages daemon → client (kind `"daemon"`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum DaemonToClient {
    /// Request completed and the new config is live.
    Ack {
        /// Human-readable summary suitable for CLI output.
        message: String,
    },
    /// Request rejected; the previous config remains live.
    Error {
        /// Machine-readable code (`invalid_config`, `io`, …).
        code: String,
        /// Human-readable message.
        message: String,
    },
    /// Reply to [`ClientToDaemon::Status`].
    Status {
        /// Daemon package version.
        daemon_version: String,
        /// Whether at least one Helloed overlay is connected.
        overlay_connected: bool,
        /// Whether an overlay/free-mode session is active.
        session_active: bool,
        /// Whether custom free mode is currently running.
        free_mode: bool,
        /// Daemon-owned config path.
        config_path: String,
    },
    /// Reply to [`ClientToDaemon::GetConfig`].
    Config {
        /// Complete effective schema-v1 config (boxed; serde flattens it).
        config: Box<Config>,
    },
}

/// Client-side failures. Every variant is actionable without a stack trace.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    /// Could not connect to the daemon socket.
    #[error("connect {path}: {source}")]
    Connect {
        /// Socket path.
        path: String,
        /// OS error.
        source: std::io::Error,
    },
    /// Socket read/write failure (including EOF before a reply).
    #[error("socket: {0}")]
    Io(#[from] std::io::Error),
    /// Envelope-level protocol failure.
    #[error("protocol: {0}")]
    Ipc(#[from] IpcError),
    /// Reply payload is not a [`DaemonToClient`].
    #[error("invalid reply: {0}")]
    Reply(String),
    /// No complete reply within the budget.
    #[error("daemon did not reply within {0:?}")]
    Timeout(Duration),
}

/// Send one client frame and wait for exactly one reply.
///
/// The connection is closed when the function returns. On timeout the caller
/// must treat the outcome as unknown: the daemon may still have applied the
/// update (the GUI re-reads status rather than retrying blindly).
pub fn request(
    socket_path: impl AsRef<Path>,
    msg: &ClientToDaemon,
    timeout: Duration,
) -> Result<DaemonToClient, ClientError> {
    let path = socket_path.as_ref();
    let mut stream = UnixStream::connect(path).map_err(|source| ClientError::Connect {
        path: path.display().to_string(),
        source,
    })?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;

    let line = encode(KIND_CLIENT, msg)?;
    stream.write_all(line.as_bytes())?;
    stream.write_all(b"\n")?;
    stream.flush()?;

    let mut reader = BufReader::new(stream);
    let mut reply = String::new();
    let read = reader
        .by_ref()
        .take((MAX_LINE_BYTES + 1) as u64)
        .read_line(&mut reply)
        .map_err(|error| map_io(error, timeout))?;
    if read == 0 {
        return Err(ClientError::Io(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "daemon closed the connection without a reply",
        )));
    }
    if reply.len() > MAX_LINE_BYTES {
        return Err(ClientError::Ipc(IpcError::TooLarge(reply.len())));
    }
    let env = decode_for(reply.trim_end(), KIND_DAEMON)?;
    serde_json::from_value(env.payload).map_err(|e| ClientError::Reply(e.to_string()))
}

fn map_io(error: std::io::Error, timeout: Duration) -> ClientError {
    match error.kind() {
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut => {
            ClientError::Timeout(timeout)
        }
        _ => ClientError::Io(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mouseless_config::{Config, DefaultAction};
    use std::os::unix::net::UnixListener;

    fn socket_path(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "mouseless-ipc-client-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("d.sock")
    }

    #[test]
    fn client_frames_round_trip() {
        let mut config = Config::default();
        config.behavior.default_action = DefaultAction::Move;
        for msg in [
            ClientToDaemon::Reload,
            ClientToDaemon::Update {
                config: Box::new(config),
            },
            ClientToDaemon::Status,
            ClientToDaemon::GetConfig,
        ] {
            let line = encode(KIND_CLIENT, &msg).unwrap();
            let env = decode_for(&line, KIND_CLIENT).unwrap();
            let decoded: ClientToDaemon = serde_json::from_value(env.payload).unwrap();
            assert_eq!(decoded, msg);
        }
    }

    #[test]
    fn daemon_replies_round_trip_including_errors() {
        for msg in [
            DaemonToClient::Ack {
                message: "config applied".into(),
            },
            DaemonToClient::Error {
                code: "invalid_config".into(),
                message: "columns must be unique".into(),
            },
            DaemonToClient::Status {
                daemon_version: "0.1.0".into(),
                overlay_connected: true,
                session_active: false,
                free_mode: false,
                config_path: "/var/lib/mouseless/config.yaml".into(),
            },
            DaemonToClient::Config {
                config: Box::new(Config::default()),
            },
        ] {
            let line = encode(KIND_DAEMON, &msg).unwrap();
            let env = decode_for(&line, KIND_DAEMON).unwrap();
            let decoded: DaemonToClient = serde_json::from_value(env.payload).unwrap();
            assert_eq!(decoded, msg);
        }
    }

    #[test]
    fn request_returns_the_daemon_reply() {
        let path = socket_path("ack");
        let listener = UnixListener::bind(&path).unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let env = decode_for(line.trim_end(), KIND_CLIENT).unwrap();
            let request: ClientToDaemon = serde_json::from_value(env.payload).unwrap();
            assert_eq!(request, ClientToDaemon::Reload);
            let reply = encode(
                KIND_DAEMON,
                &DaemonToClient::Ack {
                    message: "reloaded".into(),
                },
            )
            .unwrap();
            stream.write_all(reply.as_bytes()).unwrap();
            stream.write_all(b"\n").unwrap();
        });

        let reply = request(&path, &ClientToDaemon::Reload, Duration::from_secs(2)).unwrap();
        assert_eq!(
            reply,
            DaemonToClient::Ack {
                message: "reloaded".into()
            }
        );
        server.join().unwrap();
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn request_surfaces_error_replies() {
        let path = socket_path("error");
        let listener = UnixListener::bind(&path).unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let reply = encode(
                KIND_DAEMON,
                &DaemonToClient::Error {
                    code: "invalid_config".into(),
                    message: "bad keys".into(),
                },
            )
            .unwrap();
            stream.write_all(reply.as_bytes()).unwrap();
            stream.write_all(b"\n").unwrap();
        });

        let reply = request(
            &path,
            &ClientToDaemon::Update {
                config: Box::new(Config::default()),
            },
            Duration::from_secs(2),
        )
        .unwrap();
        assert!(matches!(
            reply,
            DaemonToClient::Error { ref code, .. } if code == "invalid_config"
        ));
        server.join().unwrap();
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn request_times_out_when_the_daemon_stays_silent() {
        let path = socket_path("timeout");
        let listener = UnixListener::bind(&path).unwrap();
        let server = std::thread::spawn(move || {
            let (_stream, _) = listener.accept().unwrap();
            std::thread::sleep(Duration::from_millis(300));
        });

        let error = request(&path, &ClientToDaemon::Reload, Duration::from_millis(50)).unwrap_err();
        assert!(matches!(error, ClientError::Timeout(_)), "{error}");
        server.join().unwrap();
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn missing_daemon_is_a_connect_error() {
        let path = socket_path("missing");
        let error = request(&path, &ClientToDaemon::Reload, CLIENT_TIMEOUT).unwrap_err();
        assert!(matches!(error, ClientError::Connect { .. }), "{error}");
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
}
