//! Fake-kanata TCP tests for the layer-changing CLI verbs (SPEC-cli).
//!
//! Each test binds an ephemeral localhost port and passes `--kanata`, so the
//! suite never touches the live kanata service on 7070.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::process::{Command, Output};

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_mouseless")
}

fn run(port: u16, verb: &str) -> Output {
    Command::new(bin())
        .args([
            "--kanata",
            &format!("127.0.0.1:{port}"),
            // Keep `status` deterministic even when a real daemon is running.
            "--socket",
            "/dev/null/mouseless.sock",
            verb,
        ])
        .output()
        .expect("run mouseless")
}

fn accept_line(listener: &TcpListener) -> String {
    let (s, _) = listener.accept().expect("accept");
    let mut line = String::new();
    BufReader::new(s)
        .read_line(&mut line)
        .expect("read request");
    line
}

#[test]
fn show_sends_overlay_layer_change() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = std::thread::spawn(move || accept_line(&listener));

    let out = run(port, "show");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "mouseless: layer lyr-mouseless"
    );
    assert_eq!(
        server.join().unwrap().trim(),
        r#"{"ChangeLayer":{"new":"lyr-mouseless"}}"#
    );
}

#[test]
fn deep_link_url_runs_the_verb() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = std::thread::spawn(move || accept_line(&listener));

    let out = run(port, "mouseless://show-overlay");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "mouseless: layer lyr-mouseless"
    );
    assert_eq!(
        server.join().unwrap().trim(),
        r#"{"ChangeLayer":{"new":"lyr-mouseless"}}"#
    );
}

#[test]
fn unknown_deep_link_exits_2() {
    let out = run(1, "mouseless://frobnicate");
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn settings_deep_link_spawns_the_gui_with_the_page() {
    let dir = std::env::temp_dir().join(format!(
        "mouseless-cli-settings-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let script = dir.join("fake-settings");
    let args_file = dir.join("args");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\nprintf '%s' \"$1\" > '{}'\n",
            args_file.display()
        ),
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

    let out = Command::new(bin())
        .env("MOUSELESS_SETTINGS", &script)
        .arg("mouseless://settings/appearance")
        .output()
        .expect("run mouseless");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let mut args = String::new();
    for _ in 0..100 {
        if let Ok(text) = std::fs::read_to_string(&args_file) {
            args = text;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(args, "mouseless://settings/appearance");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn settings_verb_accepts_a_page_argument() {
    let dir = std::env::temp_dir().join(format!(
        "mouseless-cli-settings-page-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let script = dir.join("fake-settings");
    let args_file = dir.join("args");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\nprintf '%s' \"$1\" > '{}'\n",
            args_file.display()
        ),
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

    let out = Command::new(bin())
        .env("MOUSELESS_SETTINGS", &script)
        .args(["settings", "appearance"])
        .output()
        .expect("run mouseless");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let mut args = String::new();
    for _ in 0..100 {
        if let Ok(text) = std::fs::read_to_string(&args_file) {
            args = text;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(args, "mouseless://settings/appearance");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn settings_verb_rejects_an_unknown_page() {
    let out = Command::new(bin())
        .args(["settings", "bogus"])
        .output()
        .expect("run mouseless");
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn missing_settings_binary_exits_1() {
    let out = Command::new(bin())
        .env("MOUSELESS_SETTINGS", "/nonexistent/mouseless-settings")
        .arg("settings")
        .output()
        .expect("run mouseless");
    assert_eq!(out.status.code(), Some(1));
}

#[test]
fn enter_free_mode_sends_lyr_free() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = std::thread::spawn(move || accept_line(&listener));

    let out = run(port, "enter-free-mode");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        server.join().unwrap().trim(),
        r#"{"ChangeLayer":{"new":"lyr-free"}}"#
    );
}

#[test]
fn toggle_queries_then_hides_visible_overlay() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = std::thread::spawn(move || {
        // First connection: RequestCurrentLayerName -> CurrentLayerName.
        let (mut first, _) = listener.accept().unwrap();
        let mut request = String::new();
        BufReader::new(first.try_clone().unwrap())
            .read_line(&mut request)
            .unwrap();
        assert_eq!(request.trim(), r#"{"RequestCurrentLayerName":{}}"#);
        first
            .write_all(b"{\"LayerChange\":{\"new\":\"lyr-mouseless\"}}\n")
            .unwrap();
        first
            .write_all(b"{\"CurrentLayerName\":{\"name\":\"lyr-mouseless\"}}\n")
            .unwrap();
        drop(first);
        // Second connection: ChangeLayer back to base.
        accept_line(&listener)
    });

    let out = run(port, "toggle");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "mouseless: layer lyr-base"
    );
    assert_eq!(
        server.join().unwrap().trim(),
        r#"{"ChangeLayer":{"new":"lyr-base"}}"#
    );
}

#[test]
fn status_queries_current_layer() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = std::thread::spawn(move || {
        let (mut first, _) = listener.accept().unwrap();
        let mut request = String::new();
        BufReader::new(first.try_clone().unwrap())
            .read_line(&mut request)
            .unwrap();
        assert_eq!(request.trim(), r#"{"RequestCurrentLayerName":{}}"#);
        first
            .write_all(b"{\"CurrentLayerName\":{\"name\":\"lyr-base\"}}\n")
            .unwrap();
    });

    let out = run(port, "status");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "kanata: lyr-base"
    );
    server.join().unwrap();
}

#[test]
fn status_unreachable_exits_1() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    let out = run(port, "status");
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("kanata TCP unavailable"));
}

#[test]
fn help_documents_exit_codes() {
    let out = Command::new(bin())
        .arg("--help")
        .output()
        .expect("run mouseless");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("exit: 0 success, 1 operational failure, 2 usage error"));
}

#[test]
fn version_prints_package_version() {
    let out = Command::new(bin())
        .arg("--version")
        .output()
        .expect("run mouseless");
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        format!("mouseless {}", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn unknown_verb_exits_2() {
    let out = Command::new(bin())
        .arg("frobnicate")
        .output()
        .expect("run mouseless");
    assert_eq!(out.status.code(), Some(2));
}
