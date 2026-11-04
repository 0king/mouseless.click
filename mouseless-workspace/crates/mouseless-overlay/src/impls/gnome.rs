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

use crate::{
    ClientToDaemon, DaemonToClient, GridModel, OverlayBackend, OverlayError, OverlayEvent,
};
use mouseless_ipc::auth::{group_gid, peer_allowed, peer_cred, peer_groups};
use mouseless_ipc::{
    decode, encode, AuthPolicy, CursorPos, DaemonToOverlay, Envelope, IpcError, MonitorLayout,
    OverlayToDaemon, KIND_CLIENT, KIND_DAEMON, KIND_OVERLAY, SOCKET_PATH as IPC_SOCKET_PATH,
};
use std::collections::HashMap;
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

/// Role a peer takes from its first frame. Overlays own the Hello lifecycle;
/// clients are one-shot request/reply peers and never receive broadcasts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PeerRole {
    Overlay,
    Client,
}

/// One connected peer (overlay or client), classified on its first frame.
struct Client {
    stream: UnixStream,
    pending: Vec<u8>,
    hello: bool,
    role: Option<PeerRole>,
    conn_id: u64,
    accepted: Instant,
}

impl Client {
    fn new(stream: UnixStream, conn_id: u64) -> Self {
        Self {
            stream,
            pending: Vec::new(),
            hello: false,
            role: None,
            conn_id,
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

    /// Send a daemon → client reply. The client helper expects exactly one
    /// NDJSON frame and then closes.
    fn send_client(&mut self, reply: &DaemonToClient) -> bool {
        let Ok(line) = encode(KIND_DAEMON, reply) else {
            return false;
        };
        let Ok(env) = serde_json::from_str::<Envelope>(&line) else {
            return false;
        };
        self.send(&env)
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
    next_conn_id: u64,
    next_request_id: u64,
    /// Client request id → connection id, so replies survive index shifts.
    pending_requests: HashMap<u64, u64>,
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
            next_conn_id: 1,
            next_request_id: 1,
            pending_requests: HashMap::new(),
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
                            let conn_id = self.next_conn_id;
                            self.next_conn_id = self.next_conn_id.wrapping_add(1).max(1);
                            self.clients.push(Client::new(stream, conn_id));
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
            // Only overlays (or peers that have not identified themselves yet)
            // owe a Hello. Clients are one-shot and never send one.
            if client.role != Some(PeerRole::Client)
                && !client.hello
                && client.accepted.elapsed() > HELLO_TIMEOUT
            {
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
                let client = self.clients.remove(idx);
                self.pending_requests
                    .retain(|_, conn| *conn != client.conn_id);
                if client.hello && client.role == Some(PeerRole::Overlay) {
                    self.events.push(OverlayEvent::Disconnected);
                }
            }
        }
    }

    fn handle_line(&mut self, idx: usize, line: &str) {
        let env = match decode(line) {
            Ok(env) => env,
            Err(IpcError::VersionMismatch { got, expected }) => {
                self.send_protocol_error(idx, got, expected);
                return;
            }
            Err(e) => {
                eprintln!("mouselessd: peer frame ignored: {e}");
                return;
            }
        };

        match env.kind.as_str() {
            KIND_OVERLAY => self.handle_overlay_frame(idx, env.payload),
            KIND_CLIENT => self.handle_client_frame(idx, env.payload),
            other => eprintln!("mouselessd: frame ignored (unexpected kind {other:?})"),
        }
    }

    /// Send a version-mismatch error in the peer's shape, then close it.
    fn send_protocol_error(&mut self, idx: usize, got: u32, expected: u32) {
        let Some(client) = self.clients.get_mut(idx) else {
            return;
        };
        match client.role {
            Some(PeerRole::Client) => {
                let _ = client.send_client(&DaemonToClient::Error {
                    code: "version_mismatch".into(),
                    message: "client schema is newer than daemon".into(),
                });
            }
            _ => {
                let msg = DaemonToOverlay::Error {
                    code: "version_mismatch".into(),
                    message: "peer schema is newer than daemon".into(),
                    got: Some(got),
                    expected: Some(expected),
                };
                if let Some(env) = envelope(&msg) {
                    let _ = client.send(&env);
                }
            }
        }
        let _ = client.stream.shutdown(std::net::Shutdown::Both);
        eprintln!("mouselessd: peer schema {got} > {expected}; closing peer");
    }

    fn handle_overlay_frame(&mut self, idx: usize, payload: serde_json::Value) {
        match self.clients.get_mut(idx) {
            None => return,
            Some(client) if client.role == Some(PeerRole::Client) => {
                eprintln!("mouselessd: overlay frame on a client connection ignored");
                return;
            }
            Some(client) => client.role = Some(PeerRole::Overlay),
        }
        let Ok(msg) = serde_json::from_value::<OverlayToDaemon>(payload) else {
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

    fn handle_client_frame(&mut self, idx: usize, payload: serde_json::Value) {
        let conn_id = match self.clients.get_mut(idx) {
            None => return,
            Some(client) if client.role == Some(PeerRole::Overlay) => {
                eprintln!("mouselessd: client frame on an overlay connection ignored");
                return;
            }
            Some(client) => {
                client.role = Some(PeerRole::Client);
                client.conn_id
            }
        };
        let Ok(command) = serde_json::from_value::<ClientToDaemon>(payload) else {
            // Unknown client message: forward-compatible ignore (SPEC-ipc).
            return;
        };
        let request_id = self.next_request_id;
        self.next_request_id = self.next_request_id.wrapping_add(1).max(1);
        self.pending_requests.insert(request_id, conn_id);
        self.events.push(OverlayEvent::ClientCommand {
            request_id,
            command,
        });
    }

    /// Late-join replay: current frame while visible, Hide otherwise.
    fn replay(&mut self, idx: usize) {
        if self.clients.get(idx).and_then(|client| client.role) != Some(PeerRole::Overlay) {
            return;
        }
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
        // Broadcast only to helloed overlays: clients are request/reply peers
        // and a silent overlay must never receive unrendered state.
        self.clients.retain_mut(|client| {
            if client.role == Some(PeerRole::Overlay) && client.hello {
                client.send(&env)
            } else {
                true
            }
        });
        Ok(())
    }

    /// Send the daemon reply and half-close the client's write side so the
    /// one-shot helper sees EOF after the frame.
    fn reply_client(
        &mut self,
        request_id: u64,
        reply: &DaemonToClient,
    ) -> Result<(), OverlayError> {
        let Some(conn_id) = self.pending_requests.remove(&request_id) else {
            return Ok(());
        };
        let Some(idx) = self
            .clients
            .iter()
            .position(|client| client.conn_id == conn_id)
        else {
            return Ok(());
        };
        let client = &mut self.clients[idx];
        let sent = client.send_client(reply);
        let _ = client.stream.shutdown(std::net::Shutdown::Write);
        if !sent {
            self.clients.remove(idx);
        }
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

    fn reply_client(&mut self, request_id: u64, reply: &DaemonToClient) -> Result<(), Self::Error> {
        GnomeOverlay::reply_client(self, request_id, reply)
    }

    fn is_connected(&self) -> bool {
        self.clients
            .iter()
            .any(|c| c.hello && c.role == Some(PeerRole::Overlay))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mouseless_ipc::{GridCell, MonitorInfo, KIND_CLIENT, KIND_OVERLAY};
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

    fn read_client_reply(stream: &mut UnixStream) -> DaemonToClient {
        let mut reader = std::io::BufReader::new(stream);
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let env = mouseless_ipc::decode_for(&line, KIND_DAEMON).unwrap();
        serde_json::from_value(env.payload).unwrap()
    }

    fn temp_socket(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "mouseless-overlay-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        dir.join("d.sock")
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
                    connector: None,
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
            appearance: Default::default(),
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
    fn client_reload_is_queued_acked_and_never_broadcast_to() {
        let path = temp_socket("client");
        let path = path.to_str().unwrap();
        let mut overlay = GnomeOverlay::bind(path).unwrap();

        let mut client = UnixStream::connect(path).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let msg = ClientToDaemon::Reload;
        let line = encode(KIND_CLIENT, &msg).unwrap();
        client.write_all(format!("{line}\n").as_bytes()).unwrap();
        std::thread::sleep(Duration::from_millis(50));
        overlay.poll();

        let request_id = overlay
            .drain_events()
            .into_iter()
            .find_map(|ev| match ev {
                OverlayEvent::ClientCommand {
                    request_id,
                    command,
                } => {
                    assert_eq!(command, msg);
                    Some(request_id)
                }
                _ => None,
            })
            .expect("client command event");
        // A client alone never counts as a connected overlay.
        assert!(!overlay.is_connected());

        // A broadcast while only the client is connected must not reach it.
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
            appearance: Default::default(),
        };
        overlay.show(&model).unwrap();

        let reply = DaemonToClient::Ack {
            message: "reloaded".into(),
        };
        overlay.reply_client(request_id, &reply).unwrap();
        assert_eq!(read_client_reply(&mut client), reply);
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
