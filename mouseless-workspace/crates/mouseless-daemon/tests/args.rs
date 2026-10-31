//! Binary-level argument tests for `mouselessd`.
//!
//! These run before the daemon binds the socket or connects to kanata, so they
//! are safe on a host with a live service.

use std::process::Command;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_mouselessd")
}

#[test]
fn version_prints_package_version() {
    let out = Command::new(bin())
        .arg("--version")
        .output()
        .expect("run mouselessd");
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        format!("mouselessd {}", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn unknown_arg_exits_2() {
    let out = Command::new(bin())
        .arg("--frobnicate")
        .output()
        .expect("run mouselessd");
    assert_eq!(out.status.code(), Some(2));
}
