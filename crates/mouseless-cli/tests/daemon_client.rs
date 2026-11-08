//! Fake-daemon Unix-socket tests for `mouseless reload`/`status` (SPEC-cli).

use mouseless_ipc::{decode_for, encode, ClientToDaemon, DaemonToClient, KIND_CLIENT};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_mouseless")
}

fn temp_socket(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mouseless-cli-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join("d.sock")
}

fn run(socket: &Path, verb: &str) -> Output {
    run_with(socket, 1, verb)
}

/// Run the CLI against an explicit socket and kanata port. Port 0/1 means
/// "closed"; tests that need a live kanata pass a bound ephemeral port.
fn run_with(socket: &Path, kanata_port: u16, verb: &str) -> Output {
    Command::new(bin())
        .args([
            "--kanata",
            &format!("127.0.0.1:{kanata_port}"),
            "--socket",
            socket.to_str().unwrap(),
            verb,
        ])
        .output()
        .expect("run mouseless")
}

#[test]
fn reload_sends_a_client_frame_and_prints_the_ack() {
    let path = temp_socket("reload");
    let listener = UnixListener::bind(&path).unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut line = String::new();
        BufReader::new(stream.try_clone().unwrap())
            .read_line(&mut line)
            .unwrap();
        let env = decode_for(line.trim_end(), KIND_CLIENT).unwrap();
        let request: ClientToDaemon = serde_json::from_value(env.payload).unwrap();
        assert_eq!(request, ClientToDaemon::Reload);
        let reply = encode(
            "daemon",
            &DaemonToClient::Ack {
                message: "configuration reloaded".into(),
            },
        )
        .unwrap();
        stream.write_all(reply.as_bytes()).unwrap();
        stream.write_all(b"\n").unwrap();
    });

    let out = run(&path, "reload");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "mouseless: configuration reloaded"
    );
    server.join().unwrap();
    std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
}

#[test]
fn reload_reports_a_daemon_error_and_exits_1() {
    let path = temp_socket("reload-error");
    let listener = UnixListener::bind(&path).unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut line = String::new();
        BufReader::new(stream.try_clone().unwrap())
            .read_line(&mut line)
            .unwrap();
        let reply = encode(
            "daemon",
            &DaemonToClient::Error {
                code: "invalid_config".into(),
                message: "columns must be unique".into(),
            },
        )
        .unwrap();
        stream.write_all(reply.as_bytes()).unwrap();
        stream.write_all(b"\n").unwrap();
    });

    let out = run(&path, "reload");
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("invalid_config"));
    server.join().unwrap();
    std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
}

#[test]
fn status_without_a_daemon_still_reports_kanata() {
    let path = temp_socket("status");
    // Bind-then-drop guarantees the socket path does not exist.
    let listener = UnixListener::bind(&path).unwrap();
    drop(listener);

    let out = run(&path, "status");
    // Kanata is unreachable in this test, so the kanata leg exits 1.
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("kanata TCP unavailable"));
    std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
}

#[test]
fn status_prints_daemon_state_when_reachable() {
    let path = temp_socket("status-online");
    let listener = UnixListener::bind(&path).unwrap();
    let daemon = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut line = String::new();
        BufReader::new(stream.try_clone().unwrap())
            .read_line(&mut line)
            .unwrap();
        let env = decode_for(line.trim_end(), KIND_CLIENT).unwrap();
        let request: ClientToDaemon = serde_json::from_value(env.payload).unwrap();
        assert_eq!(request, ClientToDaemon::Status);
        let reply = encode(
            "daemon",
            &DaemonToClient::Status {
                daemon_version: "9.9.9".into(),
                overlay_connected: true,
                session_active: false,
                config_path: "/var/lib/mouseless/config.yaml".into(),
            },
        )
        .unwrap();
        stream.write_all(reply.as_bytes()).unwrap();
        stream.write_all(b"\n").unwrap();
    });

    // Fake kanata: RequestCurrentLayerName -> CurrentLayerName.
    let kanata = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = kanata.local_addr().unwrap().port();
    let kanata_thread = std::thread::spawn(move || {
        let (mut stream, _) = kanata.accept().unwrap();
        let mut request = String::new();
        BufReader::new(stream.try_clone().unwrap())
            .read_line(&mut request)
            .unwrap();
        assert_eq!(request.trim(), r#"{"RequestCurrentLayerName":{}}"#);
        stream
            .write_all(b"{\"CurrentLayerName\":{\"name\":\"lyr-base\"}}\n")
            .unwrap();
    });

    let out = run_with(&path, port, "status");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("kanata: lyr-base"), "{stdout}");
    assert!(stdout.contains("daemon: ok (9.9.9)"), "{stdout}");
    assert!(stdout.contains("overlay: connected"), "{stdout}");
    assert!(stdout.contains("session: idle"), "{stdout}");
    assert!(
        stdout.contains("config: /var/lib/mouseless/config.yaml"),
        "{stdout}"
    );
    daemon.join().unwrap();
    kanata_thread.join().unwrap();
    std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
}
