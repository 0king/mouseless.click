//! Kanata TCP backend (plan §2–§3).
//!
//! Localhost TCP only (`-p 127.0.0.1:port`): subscribes `LayerChange`, sends
//! `ChangeLayer`, resyncs with `RequestCurrentLayerName/Info`, applies config
//! with `Reload*`. Reconnect = exponential backoff + resync; all events
//! idempotent. No `cmd` (audit W1).

use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
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
        TcpStream::connect_timeout(
            &self.addr().parse().expect("valid localhost addr"),
            self.timeout,
        )
    }
}

/// Read one line (one JSON frame) from kanata TCP with a deadline.
// NOTE: scaffold API; wired in Phase 1.2 (plan §9.1.2).
#[allow(dead_code)]
pub fn read_frame(reader: &mut BufReader<TcpStream>, timeout: Duration) -> std::io::Result<String> {
    reader
        .get_ref()
        .set_read_timeout(Some(timeout))
        .expect("set timeout");
    let mut line = String::new();
    reader.read_line(&mut line)?;
    Ok(line)
}

/// Send one command line to kanata TCP.
// NOTE: scaffold API; wired in Phase 1.2 (plan §9.1.2).
#[allow(dead_code)]
pub fn send_command(stream: &mut TcpStream, cmd: &str) -> std::io::Result<()> {
    stream.write_all(cmd.as_bytes())?;
    stream.write_all(b"\n")?;
    stream.flush()
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
