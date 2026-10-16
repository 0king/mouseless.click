//! `mouselessd` binary: thin orchestration, Phase 1.2 event loop.
//!
//! ```text
//! kanata TCP LayerChange → show/hide → EvdevCellGrab (fresh fd, EVIOCGKEY)
//!   → cell keys logged → esc exits via ChangeLayer → uinput mouse (Phase 1.3 clicks)
//!   → Unix-socket Show/Hide frames for the overlay (Phase 1.3 renders)
//! ```
//!
//! Kill switch (plan §7): kanata TCP EOF/reset → release grab, best-effort
//! hide broadcast, exit(1) so the supervisor restarts the stack in order.
//!
//! Needs `input` group membership for `/dev/input` and the `mouseless` group
//! + udev rule for `/dev/uinput` (plan §10.6). Without them the device paths
//! degrade loudly at startup instead of failing mid-overlay.

mod ipc_server;

use ipc_server::IpcServer;
use mouseless_daemon::{Daemon, DaemonConfig};
use mouseless_ipc::{DaemonToOverlay, Envelope, SCHEMA_VERSION};
use mouseless_kbd::{CellKey, EvdevCellGrab, KanataSubscriber, KanataTcpConfig, KbdError, KeyboardBackend, LayerEvent};
use mouseless_mouse::impls::{FakeMouse, UinputMouse};
use mouseless_overlay::impls::FakeOverlay;
use std::process::ExitCode;
use std::time::Duration;

fn hide_frame() -> Envelope {
    Envelope {
        schema_version: SCHEMA_VERSION,
        kind: "daemon".into(),
        payload: serde_json::to_value(DaemonToOverlay::Hide).expect("Hide serializes"),
    }
}

fn show_frame() -> Envelope {
    Envelope {
        schema_version: SCHEMA_VERSION,
        kind: "daemon".into(),
        payload: serde_json::to_value(DaemonToOverlay::Show {
            model: serde_json::json!({"cells": [], "note": "Phase 1.3 renders"}),
        })
        .expect("Show serializes"),
    }
}

fn parse_args() -> DaemonConfig {
    let mut cfg = DaemonConfig::default();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--kanata" => {
                if let Some(v) = args.next() {
                    cfg.kanata_addr = v;
                }
            }
            "--socket" => {
                if let Some(v) = args.next() {
                    cfg.socket_path = v;
                }
            }
            "--help" | "-h" => {
                println!("mouselessd [--kanata 127.0.0.1:7070] [--socket PATH]");
                std::process::exit(0);
            }
            _ => {
                eprintln!("unknown arg {a}; see --help");
                std::process::exit(2);
            }
        }
    }
    // Dev override without touching the systemd unit contract:
    if let Ok(sock) = std::env::var("MOUSELESS_SOCKET") {
        cfg.socket_path = sock;
    }
    cfg
}

fn main() -> ExitCode {
    let cfg = parse_args();
    let (host, port) = cfg
        .kanata_addr
        .rsplit_once(':')
        .unwrap_or(("127.0.0.1", "7070"));
    let tcp_cfg = KanataTcpConfig {
        host: host.to_string(),
        port: port.parse().unwrap_or(7070),
        timeout: Duration::from_secs(2),
    };

    let mut ipc = match IpcServer::bind(&cfg.socket_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("bind {}: {e}", cfg.socket_path);
            return ExitCode::FAILURE;
        }
    };
    println!("mouselessd: ipc socket {}", cfg.socket_path);

    match UinputMouse::create() {
        Ok(_) => println!("mouselessd: uinput mouseless-mouse ready"),
        Err(e) => eprintln!("mouselessd: uinput unavailable ({e}); clicks disabled, overlay still tracks"),
    }

    let mut sub = KanataSubscriber::new(tcp_cfg);
    // Resync on (re)connect (plan §5): never assume the starting layer.
    let start = loop {
        match sub.current_layer() {
            Ok(name) => break name,
            Err(e) => {
                eprintln!("kanata TCP unavailable ({e}); retrying…");
                std::thread::sleep(Duration::from_secs(1));
            }
        }
    };
    println!("mouselessd: kanata layer at startup: {start}");

    // Orchestration state uses fakes until the overlay geometry arrives
    // (Phase 1.3); the REAL grab + TCP subscriber drive it below.
    let mut daemon = Daemon::new(
        mouseless_kbd::impls::FakeKeyboard::new(&start),
        FakeMouse::default(),
        FakeOverlay::default(),
    );
    if start == "lyr-mouseless" {
        let _ = daemon.on_layer_event(LayerEvent::LayerChanged(start.clone()));
    }

    let mut last_clients = usize::MAX;
    loop {
        ipc.poll_accept();
        let n = ipc.client_count();
        if n != last_clients {
            println!("mouselessd: overlay clients: {n}");
            last_clients = n;
            // Late joiners missed earlier broadcasts (e.g. overlay reconnects
            // after a Shell restart): replay current state. Idempotent frames
            // make the extra send to existing clients a safe no-op.
            if daemon.overlay_visible() {
                ipc.broadcast(&show_frame());
            } else {
                ipc.broadcast(&hide_frame());
            }
        }
        let ev = match sub.next_layer_event() {
            Ok(ev) => ev,
            Err(KbdError::Unavailable(_)) => continue, // poll tick, stay alive
            Err(KbdError::Busy) => continue,           // EBUSY-style, retry
            Err(KbdError::Io(msg)) => {
                if msg.contains("EOF") || msg.contains("closed") || msg.contains("reset") {
                    eprintln!("mouselessd: kanata lost ({msg}); co-exit for supervisor restart");
                    ipc.broadcast(&hide_frame());
                    return ExitCode::from(1);
                }
                eprintln!("mouselessd: tcp ({msg}); backing off…");
                std::thread::sleep(Duration::from_secs(1));
                continue;
            }
        };

        if ev == LayerEvent::LayerChanged("lyr-mouseless".into()) {
            let _ = daemon.on_layer_event(ev);
            ipc.broadcast(&show_frame());
            run_overlay_session(&mut sub, &mut ipc);
            let _ = daemon.on_layer_event(LayerEvent::LayerChanged("base".into()));
            ipc.broadcast(&hide_frame());
        } else {
            let _ = daemon.on_layer_event(ev);
            ipc.broadcast(&hide_frame());
        }
    }
}

/// One overlay session: grab cells, log keys, exit on Escape.
///
/// The grab is held ONLY here (plan §5 rule 1). Without overlay geometry
/// (Phase 1.3) clicks are logged, not executed — esc (or layer exit) ends
/// the session and releases the grab.
fn run_overlay_session(sub: &mut KanataSubscriber, ipc: &mut IpcServer) {
    // EBUSY retry with timeout (plan §5 rule 5): never fatal, degrade to
    // "overlay unavailable, typing unaffected" by bailing to base.
    let grab = {
        let mut attempt = 0;
        loop {
            match EvdevCellGrab::acquire() {
                Ok(g) => break Some(g),
                Err(KbdError::Busy) | Err(KbdError::Unavailable(_)) if attempt < 40 => {
                    attempt += 1;
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(e) => {
                    eprintln!("mouselessd: grab failed ({e}); overlay unavailable, typing unaffected");
                    break None;
                }
            }
        }
    };
    let mut grab = match grab {
        Some(g) => g,
        None => {
            let _ = sub.set_layer("lyr-base");
            return;
        }
    };

    let mut cell: Vec<char> = Vec::new();
    loop {
        ipc.poll_accept();
        // Layer exit from elsewhere (esc in kanata layer, restart) ends us.
        match sub.next_layer_event() {
            Ok(LayerEvent::LayerChanged(name)) if name != "lyr-mouseless" => break,
            Ok(_) => {}
            Err(KbdError::Unavailable(_)) => {}
            Err(_) => break, // TCP trouble: drop grab, outer loop handles it
        }
        match grab.next_key_press() {
            Ok(Some(CellKey::Escape)) => break,
            Ok(Some(CellKey::Space)) => {
                println!("mouselessd: execute cell {cell:?} (clicks land in Phase 1.3)");
                cell.clear();
            }
            Ok(Some(CellKey::Char(c))) => {
                cell.push(c);
                println!("mouselessd: cell so far: {cell:?}");
                if cell.len() >= 2 {
                    println!("mouselessd: cell {cell:?} selected (subgrid in Phase 2)");
                }
            }
            Ok(None) => {}
            Err(e) => {
                eprintln!("mouselessd: cell read ({e})");
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    // Explicit base switch is harmless if already there (idempotent).
    let _ = sub.set_layer("lyr-base");
    // `grab` drops here: drain + ungrab + close (audits G6/G7).
}
