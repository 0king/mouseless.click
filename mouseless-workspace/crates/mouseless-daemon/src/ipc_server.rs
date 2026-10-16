//! Unix-socket IPC server, daemon side (plan §4, audits G2/G3).
//!
//! Binds [`mouseless_ipc::SOCKET_PATH`] (or an override for dev), speaks
//! newline-delimited [`Envelope`]s. Filesystem perms (dir `0770`, group
//! `mouseless` via `RuntimeDirectory`) restrict peers today; `SO_PEERCRED`
//! authentication lands with the Phase-1.3 wire spec hardening (audit G3).
//!
//! Single-threaded by design: [`IpcServer::poll_accept`] is non-blocking and
//! [`IpcServer::broadcast`] drops dead clients silently.

use mouseless_ipc::Envelope;
use std::io::Write;
use std::os::unix::net::{UnixListener, UnixStream};

/// Broadcast server for overlay frames.
pub struct IpcServer {
    listener: UnixListener,
    clients: Vec<UnixStream>,
}

impl IpcServer {
    /// Bind `socket_path`, removing a stale file first.
    pub fn bind(socket_path: &str) -> std::io::Result<Self> {
        let _ = std::fs::remove_file(socket_path);
        if let Some(parent) = std::path::Path::new(socket_path).parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let listener = UnixListener::bind(socket_path)?;
        listener.set_nonblocking(true)?;
        Ok(Self {
            listener,
            clients: vec![],
        })
    }

    /// Accept pending clients without blocking.
    pub fn poll_accept(&mut self) {
        loop {
            match self.listener.accept() {
                Ok((stream, _)) => {
                    let _ = stream.set_nonblocking(true);
                    self.clients.push(stream);
                }
                Err(_) => break,
            }
        }
    }

    /// Send one envelope to all clients, pruning dead ones.
    pub fn broadcast(&mut self, env: &Envelope) {
        let line = match serde_json::to_string(env) {
            Ok(mut s) => {
                s.push('\n');
                s
            }
            Err(_) => return,
        };
        self.clients.retain_mut(|c| {
            c.write_all(line.as_bytes())
                .and_then(|()| c.flush())
                .is_ok()
        });
    }

    /// Number of connected overlays (for status/logs).
    pub fn client_count(&self) -> usize {
        self.clients.len()
    }
}
