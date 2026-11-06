//! Kanata TCP backend (plan §2–§3).
//!
//! Localhost TCP only (`-p 127.0.0.1:port`): subscribes `LayerChange`, sends
//! `ChangeLayer`, resyncs with `RequestCurrentLayerName/Info`, applies config
//! with `Reload*`. Reconnect = exponential backoff + resync; all events
//! idempotent. No `cmd` (audit W1).

use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

/// Kanata TCP connection parameters.
#[derive(Debug, Clone)]
pub struct KanataTcpConfig {
    /// Host, always `127.0.0.1` (localhost only).
    pub host: String,
    /// Port from `kanata.service` `-p` flag.
    pub port: u16,
    /// Connect timeout.
    pub timeout: Duration,
}

impl Default for KanataTcpConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".into(),
            port: 7070,
            timeout: Duration::from_secs(2),
        }
    }
}

impl KanataTcpConfig {
    /// Socket address `host:port`.
    pub fn addr(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }

    /// Open a TCP connection (caller owns reconnect/backoff, plan §5).
    pub fn connect(&self) -> std::io::Result<TcpStream> {
        let addr = self.addr();
        let socket: SocketAddr = addr.parse().map_err(|e| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("invalid kanata address {addr}: {e}"),
            )
        })?;
        TcpStream::connect_timeout(&socket, self.timeout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn localhost_only_by_default() {
        let cfg = KanataTcpConfig::default();
        assert_eq!(cfg.host, "127.0.0.1");
        assert_eq!(cfg.addr(), "127.0.0.1:7070");
    }
}
