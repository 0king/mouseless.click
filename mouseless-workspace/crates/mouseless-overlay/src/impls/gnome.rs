//! GNOME Shell overlay backend (Phase 1.3).
//!
//! The daemon owns the listener; the extension is the client (SPEC-ipc
//! lifecycle). This backend:
//! - binds `/run/mouseless/mouselessd.sock` (or a dev override),
//! - authenticates peers with `SO_PEERCRED` (audit G3),
//! - speaks newline-delimited JSON (`Hello`/`Report`/`CursorReply` upstream,
//!   `Show`/`HighlightCell`/`ShowSubgrid`/`Hide`/`QueryCursor` downstream),
//! - replays current state when a Shell restart reconnects.
//!
//! It is draw-only: nothing here captures input (plan §1.5).

use crate::{GridModel, OverlayBackend, OverlayError, OverlayEvent};
use mouseless_ipc::auth::{group_gid, peer_allowed, peer_cred, peer_groups};
use mouseless_ipc::{
    decode_for, encode, AuthPolicy, CursorPos, DaemonToOverlay, Envelope, IpcError, MonitorLayout,
    OverlayToDaemon, KIND_DAEMON, KIND_OVERLAY, SOCKET_PATH as IPC_SOCKET_PATH,
};
use std::io::{ErrorKind, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::time::{Duration, Instant};

/// Supported Shell versions (`metadata.json`).
pub const GNOME_SHELL_VERSIONS: &[&str] = &["45", "46", "47", "48", "49", "50"];

/// Unix socket path (mirrors `mouseless-ipc::SOCKET_PATH`, audit G3).
pub const SOCKET_PATH: &str = IPC_SOCKET_PATH;

/// Group allowed to open the socket (`RuntimeDirectory` group).
pub const SOCKET_GROUP: &str = "mouseless";

/// A peer that connects must send `Hello` within this budget (SPEC-ipc).
const HELLO_TIMEOUT: Duration = Duration::from_secs(2);

/// One connected overlay client.
struct Client {
    stream: UnixStream,
    pending: Vec<u8>,
    hello: bool,
    accepted: Instant,
}

impl Client {
    fn new(stream: UnixStream) -> Self {
        Self {
            stream,
            pending: Vec::new(),
            hello: false,
            accepted: Instant::now(),
        }
    }

    /// Queue bytes and return every complete line. `true` means a line
    /// exceeded [`mouseless_ipc::MAX_LINE_BYTES`] and the peer must be closed.
    fn take_lines(&mut self) -> (Vec<String>, bool) {
        let mut lines = Vec::new();
        while let Some(pos) = self.pending.iter().position(|b| *b == b'\n') {
            let raw: Vec<u8> = self.pending.drain(..=pos).collect();
            let length = raw.len().saturating_sub(1);
            if length > mouseless_ipc::MAX_LINE_BYTES {
                self.pending.clear();
                return (lines, true);
            }
            let line = String::from_utf8_lossy(&raw[..length]).into_owned();
            if !line.trim().is_empty() {
                lines.push(line);
            }
        }
        (lines, false)
    }

    fn send(&mut self, env: &Envelope) -> bool {
        let Ok(mut line) = serde_json::to_string(env) else {
            return false;
        };
        line.push('\n');
        self.stream.write_all(line.as_bytes()).is_ok()
    }
}

/// Build a daemon frame without a fallible parse step.
fn envelope(msg: &DaemonToOverlay) -> Option<Envelope> {
    let line = encode(KIND_DAEMON, msg).ok()?;
    serde_json::from_str(&line).ok()
}

/// Socket-server overlay backend for GNOME Shell.
pub struct GnomeOverlay {
    listener: UnixListener,
    clients: Vec<Client>,
    policy: AuthPolicy,
    last_cursor: Option<CursorPos>,
    last_layout: Option<MonitorLayout>,
    visible: bool,
    last_model: Option<GridModel>,
    last_frame: Option<DaemonToOverlay>,
    events: Vec<OverlayEvent>,
}

impl GnomeOverlay {
    /// Bind the overlay socket. Removes a stale socket file first (crash
    /// recovery); the systemd unit owns the directory.
    pub fn bind(socket_path: &str) -> Result<Self, OverlayError> {
        if let Some(parent) = Path::new(socket_path).parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| OverlayError::Connection(format!("mkdir: {e}")))?;
            }
        }
        let _ = std::fs::remove_file(socket_path);
        let listener = UnixListener::bind(socket_path)
            .map_err(|e| OverlayError::Connection(format!("bind {socket_path}: {e}")))?;
        listener
            .set_nonblocking(true)
            .map_err(|e| OverlayError::Connection(format!("nonblocking: {e}")))?;
        let allowed_gid = group_gid(SOCKET_GROUP).unwrap_or(u32::MAX);
        Ok(Self {
            listener,
            clients: Vec::new(),
            policy: AuthPolicy::for_current_process(allowed_gid),
            last_cursor: None,
            last_layout: None,
            visible: false,
            last_model: None,
            last_frame: None,
            events: Vec::new(),
        })
    }

    /// Bind the pinned production socket path.
    pub fn bind_default() -> Result<Self, OverlayError> {
        Self::bind(SOCKET_PATH)
    }

    /// Number of accepted clients (authorized, Hello not required yet).
    pub fn client_count(&self) -> usize {
        self.clients.len()
    }

    /// Drain queued overlay events since the last call.
    pub fn drain_events(&mut self) -> Vec<OverlayEvent> {
        std::mem::take(&mut self.events)
    }

    /// The grid model last pushed with [`OverlayBackend::show`].
    pub fn last_model(&self) -> Option<&GridModel> {
        self.last_model.as_ref()
    }

    /// Accept pending clients and read pending frames without blocking.
    pub fn poll(&mut self) {
        self.poll_accept();
        self.poll_reads();
    }

    fn poll_accept(&mut self) {
        loop {
            match self.listener.accept() {
                Ok((stream, _)) => {
                    if stream.set_nonblocking(true).is_err() {
                        continue;
                    }
                    match peer_cred(&stream) {
                        Ok(cred) => {
                            let groups = peer_groups(cred.pid);
                            if !peer_allowed(cred, &groups, &self.policy) {
                                eprintln!(
                                    "mouselessd: overlay peer uid={} pid={} refused (not in {SOCKET_GROUP})",
                                    cred.uid, cred.pid
                                );
                                continue;
                            }
                            self.clients.push(Client::new(stream));
                        }
                        Err(e) => eprintln!("mouselessd: SO_PEERCRED failed: {e}"),
                    }
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(e) => {
                    eprintln!("mouselessd: overlay accept: {e}");
                    break;
                }
            }
        }
    }

    fn poll_reads(&mut self) {
        let mut dead = Vec::new();
        let mut incoming: Vec<(usize, String)> = Vec::new();
        for (idx, client) in self.clients.iter_mut().enumerate() {
            if !client.hello && client.accepted.elapsed() > HELLO_TIMEOUT {
                eprintln!("mouselessd: overlay peer sent no Hello within 2s; closing");
                dead.push(idx);
                continue;
            }
            loop {
                let mut buf = [0u8; 8192];
                match client.stream.read(&mut buf) {
                    Ok(0) => {
                        dead.push(idx);
                        break;
                    }
                    Ok(n) => {
                        client.pending.extend_from_slice(&buf[..n]);
                        let (lines, oversized) = client.take_lines();
                        incoming.extend(lines.into_iter().map(|line| (idx, line)));
                        if oversized {
                            eprintln!("mouselessd: overlay frame too large; dropping client");
                            dead.push(idx);
                            break;
                        }
                        if client.pending.len() > mouseless_ipc::MAX_LINE_BYTES {
                            eprintln!("mouselessd: overlay frame too large; dropping client");
                            client.pending.clear();
                            dead.push(idx);
                            break;
                        }
                    }
                    Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                    Err(e) => {
                        eprintln!("mouselessd: overlay read: {e}");
                        dead.push(idx);
                        break;
                    }
                }
            }
        }

        for (idx, line) in incoming {
            self.handle_line(idx, &line);
        }

        dead.sort_unstable();
        dead.dedup();
        for idx in dead.into_iter().rev() {
            if idx < self.clients.len() {
                let had_hello = self.clients[idx].hello;
                self.clients.remove(idx);
                if had_hello {
                    self.events.push(OverlayEvent::Disconnected);
                }
            }
        }
    }

    fn handle_line(&mut self, idx: usize, line: &str) {
        let env = match decode_for(line, KIND_OVERLAY) {
            Ok(env) => env,
            Err(IpcError::VersionMismatch { got, expected }) => {
                let msg = DaemonToOverlay::Error {
                    code: "version_mismatch".into(),
                    message: "extension schema is newer than daemon".into(),
                    got: Some(got),
                    expected: Some(expected),
                };
                if let (Some(client), Some(env)) = (self.clients.get_mut(idx), envelope(&msg)) {
                    let _ = client.send(&env);
                    let _ = client.stream.shutdown(std::net::Shutdown::Both);
                }
                eprintln!("mouselessd: overlay schema {got} > {expected}; closing peer");
                return;
            }
            Err(e) => {
                eprintln!("mouselessd: overlay frame ignored: {e}");
                return;
            }
        };

        let Ok(msg) = serde_json::from_value::<OverlayToDaemon>(env.payload) else {
            // Unknown message type: forward-compatible ignore (SPEC-ipc).
            return;
        };
        match msg {
            OverlayToDaemon::Hello { extension_version } => {
                if let Some(client) = self.clients.get_mut(idx) {
                    client.hello = true;
                }
                eprintln!("mouselessd: overlay hello (extension {extension_version})");
                self.events.push(OverlayEvent::Hello { extension_version });
                self.replay(idx);
            }
            OverlayToDaemon::Report { geometry, cursor } => {
                self.last_layout = Some(geometry.clone());
                self.last_cursor = Some(cursor);
                self.events.push(OverlayEvent::Report {
                    layout: geometry,
                    cursor,
                });
            }
            OverlayToDaemon::CursorReply { x, y } => {
                let cursor = CursorPos { x, y };
                self.last_cursor = Some(cursor);
                self.events.push(OverlayEvent::CursorReply { cursor });
            }
        }
    }

    /// Late-join replay: current frame while visible, Hide otherwise.
    fn replay(&mut self, idx: usize) {
        let replay = if self.visible {
            self.last_frame.clone()
        } else {
            Some(DaemonToOverlay::Hide)
        };
        let (Some(client), Some(env)) = (
            self.clients.get_mut(idx),
            replay.as_ref().and_then(envelope),
        ) else {
            return;
        };
        let _ = client.send(&env);
    }

    fn broadcast(&mut self, msg: &DaemonToOverlay) -> Result<(), OverlayError> {
        let Some(env) = envelope(msg) else {
            return Err(OverlayError::Schema("frame does not serialize".into()));
        };
        self.clients
            .retain_mut(|client| !client.hello || client.send(&env));
        Ok(())
    }
}

impl OverlayBackend for GnomeOverlay {
    type Error = OverlayError;

    fn show(&mut self, model: &GridModel) -> Result<(), Self::Error> {
        self.visible = true;
        self.last_model = Some(model.clone());
        self.last_frame = Some(DaemonToOverlay::Show {
            model: model.clone(),
        });
        self.broadcast(&DaemonToOverlay::Show {
            model: model.clone(),
        })
    }

    fn highlight(&mut self, index: u32) -> Result<(), Self::Error> {
        if let Some(model) = self.last_model.as_mut() {
            model.highlight = Some(index);
        }
        self.broadcast(&DaemonToOverlay::HighlightCell { index })
    }

    fn show_subgrid(&mut self, parent: u32, model: &GridModel) -> Result<(), Self::Error> {
        let msg = DaemonToOverlay::ShowSubgrid {
            parent,
            model: model.clone(),
        };
        self.last_frame = Some(msg.clone());
        self.broadcast(&msg)
    }

    fn hide(&mut self) -> Result<(), Self::Error> {
        self.visible = false;
        self.broadcast(&DaemonToOverlay::Hide)
    }

    fn query_cursor(&mut self) -> Result<(), Self::Error> {
        self.broadcast(&DaemonToOverlay::QueryCursor)
    }

    fn poll_events(&mut self) -> Vec<OverlayEvent> {
        self.poll();
        self.drain_events()
    }

    fn last_cursor(&self) -> Option<CursorPos> {
        self.last_cursor
    }

    fn last_layout(&self) -> Option<MonitorLayout> {
        self.last_layout.clone()
    }

    fn is_connected(&self) -> bool {
        self.clients.iter().any(|c| c.hello)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mouseless_ipc::{GridCell, MonitorInfo, KIND_OVERLAY};
    use std::io::BufRead;
    use std::os::unix::net::UnixStream;

    fn hello(stream: &mut UnixStream) {
        let msg = OverlayToDaemon::Hello {
            extension_version: "test".into(),
        };
        let line = encode(KIND_OVERLAY, &msg).unwrap();
        stream.write_all(format!("{line}\n").as_bytes()).unwrap();
    }

    fn read_frame(stream: &mut UnixStream) -> DaemonToOverlay {
        let mut reader = std::io::BufReader::new(stream);
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let env = mouseless_ipc::decode(&line).unwrap();
        serde_json::from_value(env.payload).unwrap()
    }

    #[test]
    fn hello_replay_and_report_updates_state() {
        let dir = std::env::temp_dir().join(format!(
            "mouseless-overlay-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = dir.join("d.sock");
        let path = path.to_str().unwrap();
        let mut overlay = GnomeOverlay::bind(path).unwrap();

        let mut client = UnixStream::connect(path).unwrap();
        client.set_nonblocking(true).unwrap();
        hello(&mut client);
        // Give the kernel a moment to deliver the Hello, then poll.
        std::thread::sleep(std::time::Duration::from_millis(50));
        overlay.poll();
        assert!(overlay.is_connected());
        // Late-join replay is Hide while not visible.
        let frame = read_frame(&mut client);
        assert_eq!(frame, DaemonToOverlay::Hide);

        let report = OverlayToDaemon::Report {
            geometry: MonitorLayout {
                monitors: vec![MonitorInfo {
                    monitor: mouseless_core::Monitor {
                        x: 0,
                        y: 0,
                        w: 1920,
                        h: 1080,
                        scale: 1.0,
                    },
                    primary: true,
                }],
            },
            cursor: CursorPos { x: 10, y: 20 },
        };
        let line = encode(KIND_OVERLAY, &report).unwrap();
        client.write_all(format!("{line}\n").as_bytes()).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(50));
        overlay.poll();
        assert_eq!(overlay.last_cursor(), Some(CursorPos { x: 10, y: 20 }));
        assert_eq!(overlay.last_layout().unwrap().monitors.len(), 1);

        let model = GridModel {
            monitor: mouseless_core::Monitor {
                x: 0,
                y: 0,
                w: 1920,
                h: 1080,
                scale: 1.0,
            },
            cells: vec![GridCell::new(
                0,
                "a",
                mouseless_core::Rect {
                    x: 0.0,
                    y: 0.0,
                    w: 480.0,
                    h: 540.0,
                },
            )],
            highlight: None,
        };
        overlay.show(&model).unwrap();
        assert_eq!(read_frame(&mut client), DaemonToOverlay::Show { model });
        overlay.highlight(0).unwrap();
        assert_eq!(
            read_frame(&mut client),
            DaemonToOverlay::HighlightCell { index: 0 }
        );
        overlay.hide().unwrap();
        assert_eq!(read_frame(&mut client), DaemonToOverlay::Hide);
    }

    #[test]
    fn silent_peer_is_closed_after_hello_timeout() {
        let dir = std::env::temp_dir().join(format!(
            "mouseless-overlay-hello-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = dir.join("d.sock");
        let path = path.to_str().unwrap();
        let mut overlay = GnomeOverlay::bind(path).unwrap();
        let _client = UnixStream::connect(path).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(50));
        overlay.poll_accept();
        assert_eq!(overlay.client_count(), 1);
        overlay.clients[0].accepted = Instant::now() - HELLO_TIMEOUT - Duration::from_millis(1);
        overlay.poll_reads();
        assert_eq!(overlay.client_count(), 0);
    }

    #[test]
    fn malformed_frames_do_not_drop_the_client() {
        let dir = std::env::temp_dir().join(format!(
            "mouseless-overlay-malformed-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = dir.join("d.sock");
        let path = path.to_str().unwrap();
        let mut overlay = GnomeOverlay::bind(path).unwrap();
        let mut client = UnixStream::connect(path).unwrap();
        client.set_nonblocking(true).unwrap();
        hello(&mut client);
        std::thread::sleep(std::time::Duration::from_millis(50));
        overlay.poll();
        let _ = read_frame(&mut client);

        client.write_all(b"not json\n{broken\n").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(50));
        overlay.poll();
        // Still connected and still receives daemon frames.
        assert!(overlay.is_connected());
        overlay.hide().unwrap();
        assert_eq!(read_frame(&mut client), DaemonToOverlay::Hide);
    }
}
