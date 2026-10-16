//! Live kanata TCP subscriber (Phase 1.2).
//!
//! Localhost TCP only: the server pushes `{"LayerChange":{"new":…}}` on
//! connect and on every switch. Reconnect = exponential backoff + resync via
//! `RequestCurrentLayerName` (plan §5). Duplicate/missed events stay safe
//! because the daemon's show/hide is idempotent.

use crate::{KbdError, KanataTcpConfig, KeyboardBackend, LayerEvent};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::time::Duration;

/// Backoff schedule for reconnects (plan §5): 100ms doubling to 5s max.
pub fn backoff_for_attempt(attempt: u32) -> Duration {
    Duration::from_millis(100u64.saturating_mul(2u64.saturating_pow(attempt.min(6)))).min(Duration::from_secs(5))
}

/// Query the current layer name once (used for resync and `mouseless status`).
pub fn query_current_layer(cfg: &KanataTcpConfig) -> Result<String, KbdError> {
    let mut stream = cfg
        .connect()
        .map_err(|e| KbdError::Io(format!("connect {}: {e}", cfg.addr())))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .map_err(|e| KbdError::Io(e.to_string()))?;
    stream
        .write_all(b"{\"RequestCurrentLayerName\":{}}\n")
        .map_err(|e| KbdError::Io(e.to_string()))?;
    let mut reader = BufReader::new(stream);
    loop {
        let mut line = String::new();
        reader
            .read_line(&mut line)
            .map_err(|e| KbdError::Io(e.to_string()))?;
        if line.is_empty() {
            return Err(KbdError::Io("kanata closed connection".into()));
        }
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) {
            if let Some(name) = v.get("CurrentLayerName").and_then(|m| m.get("name")).and_then(|n| n.as_str()) {
                return Ok(name.to_string());
            }
            // Ignore other pushes (LayerChange, MessagePush) while resyncing.
        }
    }
}

/// Parse one server line into a [`LayerEvent`]. Returns `None` for
/// non-layer messages (config reload notices, message pushes).
pub fn parse_server_line(line: &str) -> Option<LayerEvent> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    let new = v.get("LayerChange")?.get("new")?.as_str()?;
    Some(LayerEvent::LayerChanged(new.to_string()))
}

/// Blocking subscriber with reconnect + resync. Owns one TCP connection;
/// on EOF/error the next poll reconnects with backoff (plan §5).
pub struct KanataSubscriber {
    cfg: KanataTcpConfig,
    reader: Option<BufReader<TcpStream>>,
    failures: u32,
}

impl KanataSubscriber {
    /// Create unconnected; the first poll connects.
    pub fn new(cfg: KanataTcpConfig) -> Self {
        Self {
            cfg,
            reader: None,
            failures: 0,
        }
    }

    /// Ensure a live connection (backoff sleep on repeated failure).
    fn ensure_connected(&mut self) -> Result<(), KbdError> {
        if self.reader.is_some() {
            return Ok(());
        }
        if self.failures > 0 {
            std::thread::sleep(backoff_for_attempt(self.failures - 1));
        }
        match self.cfg.connect() {
            Ok(s) => {
                s.set_read_timeout(Some(Duration::from_millis(500)))
                    .map_err(|e| KbdError::Io(e.to_string()))?;
                self.reader = Some(BufReader::new(s));
                self.failures = 0;
                Ok(())
            }
            Err(e) => {
                self.failures += 1;
                Err(KbdError::Io(format!("connect {}: {e}", self.cfg.addr())))
            }
        }
    }
}

impl KeyboardBackend for KanataSubscriber {
    type Error = KbdError;

    fn next_layer_event(&mut self) -> Result<LayerEvent, Self::Error> {
        loop {
            self.ensure_connected()?;
            let reader = self.reader.as_mut().expect("connected");
            let mut line = String::new();
            match reader.read_line(&mut line) {
                Ok(0) => {
                    // EOF = kanata died (plan §7 kill switch).
                    self.reader = None;
                    self.failures += 1;
                    return Err(KbdError::Io("kanata TCP EOF".into()));
                }
                Ok(_) => {
                    if let Some(ev) = parse_server_line(&line) {
                        return Ok(ev);
                    }
                    // Non-layer push: keep polling (timeout returns Ok with
                    // empty progress; loop back to read again).
                    continue;
                }
                Err(e) if e.kind() == std::io::ErrorKind::TimedOut || e.kind() == std::io::ErrorKind::WouldBlock => {
                    // Poll timeout: no event right now. Report unavailability
                    // so the caller can do other work, staying connected.
                    return Err(KbdError::Unavailable("no layer event yet".into()));
                }
                Err(e) => {
                    self.reader = None;
                    self.failures += 1;
                    return Err(KbdError::Io(format!("tcp read: {e}")));
                }
            }
        }
    }

    fn set_layer(&mut self, name: &str) -> Result<(), Self::Error> {
        self.ensure_connected()?;
        let reader = self.reader.as_mut().expect("connected");
        let cmd = format!("{{\"ChangeLayer\":{{\"new\":\"{name}\"}}}}\n");
        reader
            .get_mut()
            .write_all(cmd.as_bytes())
            .map_err(|e| KbdError::Io(e.to_string()))
    }

    fn current_layer(&mut self) -> Result<String, Self::Error> {
        query_current_layer(&self.cfg)
    }

    fn reload(&mut self) -> Result<(), Self::Error> {
        self.ensure_connected()?;
        let reader = self.reader.as_mut().expect("connected");
        reader
            .get_mut()
            .write_all(b"{\"RequestReload\":{}}\n")
            .map_err(|e| KbdError::Io(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::net::TcpListener;

    #[test]
    fn parses_layer_change_ignores_rest() {
        assert_eq!(
            parse_server_line("{\"LayerChange\":{\"new\":\"lyr-mouseless\"}}\n"),
            Some(LayerEvent::LayerChanged("lyr-mouseless".into()))
        );
        assert_eq!(parse_server_line("{\"ConfigFileReload\":{}}"), None);
        assert_eq!(parse_server_line("not json"), None);
        assert_eq!(parse_server_line(""), None);
    }

    #[test]
    fn backoff_caps_at_5s() {
        assert_eq!(backoff_for_attempt(0), Duration::from_millis(100));
        assert_eq!(backoff_for_attempt(6), Duration::from_secs(5));
        assert_eq!(backoff_for_attempt(100), Duration::from_secs(5));
    }

    #[test]
    fn resync_reads_current_layer_name() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let h = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            use std::io::BufReader;
            let mut r = BufReader::new(s.try_clone().unwrap());
            let mut line = String::new();
            r.read_line(&mut line).unwrap();
            s.write_all(b"{\"LayerChange\":{\"new\":\"lyr-base\"}}\n").unwrap();
            s.write_all(b"{\"CurrentLayerName\":{\"name\":\"lyr-mouseless\"}}\n").unwrap();
            std::thread::sleep(Duration::from_millis(100));
        });
        let cfg = KanataTcpConfig {
            host: "127.0.0.1".into(),
            port,
            timeout: Duration::from_secs(2),
        };
        assert_eq!(query_current_layer(&cfg).unwrap(), "lyr-mouseless");
        h.join().unwrap();
    }

    #[test]
    fn subscriber_receives_layer_change() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let h = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            s.write_all(b"{\"LayerChange\":{\"new\":\"lyr-mouseless\"}}\n").unwrap();
            std::thread::sleep(Duration::from_millis(300));
        });
        let cfg = KanataTcpConfig {
            host: "127.0.0.1".into(),
            port,
            timeout: Duration::from_secs(2),
        };
        let mut sub = KanataSubscriber::new(cfg);
        assert_eq!(
            sub.next_layer_event().unwrap(),
            LayerEvent::LayerChanged("lyr-mouseless".into())
        );
        h.join().unwrap();
    }
}
