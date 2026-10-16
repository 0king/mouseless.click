//! `mouselessd` binary: thin orchestration only (plan §1.2).
//!
//! Phase-1.2 wires real backends (kanata TCP + evdev temp-grab + uinput +
//! Unix-socket IPC). This scaffold parses config and exits cleanly so the
//! supervisor contract (plan §7) is testable from day one.

use mouseless_daemon::DaemonConfig;

fn main() {
    let cfg = DaemonConfig::default();
    println!(
        "mouselessd scaffold: keyboard_backend={} overlay_backend={} kanata={} socket={}",
        cfg.keyboard_backend, cfg.overlay_backend, cfg.kanata_addr, cfg.socket_path
    );
    println!("Phase 1.2 wires real backends; see plan §9.1.2.");
}
