//! `mouselessd` binary: thin orchestration, Phase 1.3 production wiring.
//!
//! ```text
//! kanata TCP LayerChange → show/hide → EvdevCellGrab (fresh fd, EVIOCGKEY)
//!   → cell keys → subgrid → click via uinput mouse
//!   → Unix-socket Show/Highlight/ShowSubgrid/Hide frames (GNOME extension)
//! ```
//!
//! Kill switch (plan §7): kanata TCP EOF/reset → release grab, best-effort
//! hide broadcast, exit(1) so the supervisor restarts the stack in order.
//!
//! Needs `input` group membership for `/dev/input` and the `mouseless` group
//! plus the udev rule for `/dev/uinput` (plan §10.6). Missing uinput degrades
//! to "overlay tracks, clicks disabled"; missing overlay degrades to "typing
//! unaffected, no grab" (plan §6).

use mouseless_daemon::{Daemon, DaemonConfig, DaemonError, SessionStep, OVERLAY_LAYER};
use mouseless_ipc::CursorPos;
use mouseless_kbd::{
    EvdevCellGrab, KanataSubscriber, KanataTcpConfig, KbdError, KeyboardBackend, LayerEvent,
};
use mouseless_mouse::impls::UinputMouse;
use mouseless_mouse::{MouseBackend, MouseButton, MouseError};
use mouseless_overlay::impls::GnomeOverlay;
use mouseless_overlay::{GridModel, MonitorLayout, OverlayBackend, OverlayError, OverlayEvent};
use std::process::ExitCode;
use std::time::Duration;

/// Production mouse: real uinput, or a loud no-op when `/dev/uinput` is denied.
enum ProdMouse {
    Uinput(UinputMouse),
    Disabled,
}

impl ProdMouse {
    fn create() -> Self {
        match UinputMouse::create() {
            Ok(m) => {
                println!("mouselessd: uinput mouseless-mouse ready");
                Self::Uinput(m)
            }
            Err(e) => {
                eprintln!(
                    "mouselessd: uinput unavailable ({e}); clicks disabled, overlay still tracks"
                );
                Self::Disabled
            }
        }
    }
}

impl MouseBackend for ProdMouse {
    type Error = MouseError;

    fn move_rel(&mut self, dx: i32, dy: i32) -> Result<(), Self::Error> {
        match self {
            Self::Uinput(m) => m.move_rel(dx, dy),
            Self::Disabled => Err(MouseError::Io("uinput unavailable".into())),
        }
    }

    fn click(&mut self, button: MouseButton) -> Result<(), Self::Error> {
        match self {
            Self::Uinput(m) => m.click(button),
            Self::Disabled => Err(MouseError::Io("uinput unavailable".into())),
        }
    }

    fn scroll(&mut self, dx: i32, dy: i32) -> Result<(), Self::Error> {
        match self {
            Self::Uinput(m) => m.scroll(dx, dy),
            Self::Disabled => Err(MouseError::Io("uinput unavailable".into())),
        }
    }
}

/// Production overlay: GNOME socket backend, or a no-op if bind failed.
enum ProdOverlay {
    Gnome(Box<GnomeOverlay>),
    Disabled,
}

impl ProdOverlay {
    fn bind(path: &str) -> Self {
        match GnomeOverlay::bind(path) {
            Ok(o) => {
                println!("mouselessd: overlay socket {path}");
                Self::Gnome(Box::new(o))
            }
            Err(e) => {
                eprintln!("mouselessd: overlay socket unavailable ({e}); typing unaffected");
                Self::Disabled
            }
        }
    }
}

impl OverlayBackend for ProdOverlay {
    type Error = OverlayError;

    fn show(&mut self, model: &GridModel) -> Result<(), Self::Error> {
        match self {
            Self::Gnome(o) => o.show(model),
            Self::Disabled => Ok(()),
        }
    }

    fn highlight(&mut self, index: u32) -> Result<(), Self::Error> {
        match self {
            Self::Gnome(o) => o.highlight(index),
            Self::Disabled => Ok(()),
        }
    }

    fn show_subgrid(&mut self, parent: u32, model: &GridModel) -> Result<(), Self::Error> {
        match self {
            Self::Gnome(o) => o.show_subgrid(parent, model),
            Self::Disabled => Ok(()),
        }
    }

    fn hide(&mut self) -> Result<(), Self::Error> {
        match self {
            Self::Gnome(o) => o.hide(),
            Self::Disabled => Ok(()),
        }
    }

    fn query_cursor(&mut self) -> Result<(), Self::Error> {
        match self {
            Self::Gnome(o) => o.query_cursor(),
            Self::Disabled => Ok(()),
        }
    }

    fn poll_events(&mut self) -> Vec<OverlayEvent> {
        match self {
            Self::Gnome(o) => {
                o.poll();
                o.drain_events()
            }
            Self::Disabled => Vec::new(),
        }
    }

    fn last_cursor(&self) -> Option<CursorPos> {
        match self {
            Self::Gnome(o) => o.last_cursor(),
            Self::Disabled => None,
        }
    }

    fn last_layout(&self) -> Option<MonitorLayout> {
        match self {
            Self::Gnome(o) => o.last_layout(),
            Self::Disabled => None,
        }
    }

    fn is_connected(&self) -> bool {
        match self {
            Self::Gnome(o) => o.is_connected(),
            Self::Disabled => false,
        }
    }
}

type ProdDaemon = Daemon<KanataSubscriber, ProdMouse, ProdOverlay, EvdevCellGrab>;

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

    let overlay = ProdOverlay::bind(&cfg.socket_path);
    let mouse = ProdMouse::create();
    let mut daemon = Daemon::with_grab(
        KanataSubscriber::new(tcp_cfg),
        mouse,
        overlay,
        cfg,
        EvdevCellGrab::acquire,
    );

    // Resync on (re)connect (plan §5): never assume the starting layer.
    let start = loop {
        match daemon.keyboard_mut().current_layer() {
            Ok(name) => break name,
            Err(e) => {
                eprintln!("mouselessd: kanata TCP unavailable ({e}); retrying…");
                std::thread::sleep(Duration::from_secs(1));
            }
        }
    };
    println!("mouselessd: kanata layer at startup: {start}");

    // A Shell may already be on the overlay layer when the daemon starts.
    if start == OVERLAY_LAYER {
        let _ = daemon.poll_overlay_events();
        if let Err(e) = start_with_retry(&mut daemon) {
            eprintln!("mouselessd: overlay start degraded: {e}");
        } else if run_session(&mut daemon) {
            let _ = daemon.on_kanata_lost();
            return ExitCode::from(1);
        }
    }

    loop {
        // Overlay reports must be current before a layer-event show.
        let mut overlay_lost = false;
        for ev in daemon.poll_overlay_events() {
            if matches!(ev, OverlayEvent::Disconnected) {
                overlay_lost = true;
            }
        }
        if overlay_lost {
            eprintln!("mouselessd: overlay disconnected; releasing grab, typing unaffected");
            daemon.on_overlay_lost();
        }

        match daemon.keyboard_mut().next_layer_event() {
            Ok(LayerEvent::LayerChanged(name)) if name == OVERLAY_LAYER => {
                let _ = daemon.poll_overlay_events();
                match start_with_retry(&mut daemon) {
                    Ok(()) => {
                        if run_session(&mut daemon) {
                            let _ = daemon.on_kanata_lost();
                            return ExitCode::from(1);
                        }
                    }
                    Err(e) => eprintln!("mouselessd: overlay start degraded: {e}"),
                }
            }
            Ok(_) => {
                // Any other layer hides; idempotent if already hidden.
                daemon.end_overlay();
            }
            Err(KbdError::Unavailable(_)) | Err(KbdError::Busy) => {}
            Err(KbdError::Io(msg)) => {
                if is_kanata_gone(&msg) {
                    eprintln!("mouselessd: kanata lost ({msg}); co-exit for supervisor restart");
                    let _ = daemon.on_kanata_lost();
                    return ExitCode::from(1);
                }
                eprintln!("mouselessd: tcp ({msg}); backing off…");
                std::thread::sleep(Duration::from_secs(1));
            }
        }
    }
}

/// Start the overlay, retrying `EBUSY` for up to ~2 s (plan §5 rule 5).
fn start_with_retry(daemon: &mut ProdDaemon) -> Result<(), DaemonError> {
    for _ in 0..40 {
        match daemon.start_overlay() {
            Ok(()) => return Ok(()),
            Err(DaemonError::Kbd(KbdError::Busy)) => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => return Err(e),
        }
    }
    Err(DaemonError::Kbd(KbdError::Busy))
}

/// One overlay session. Returns `true` when kanata died (co-exit path).
///
/// The grab is held ONLY here (plan §5 rule 1). Esc/click/layer-exit all end
/// the session; `end_overlay` releases the grab and restores the base layer.
fn run_session(daemon: &mut ProdDaemon) -> bool {
    // Tight idle timeout so grabbed keys are polled every few milliseconds;
    // TCP data still wakes `read_line` immediately.
    if let Err(e) = daemon
        .keyboard_mut()
        .set_read_timeout(Duration::from_millis(2))
    {
        eprintln!("mouselessd: session read timeout: {e}");
    }

    let mut kanata_gone = false;
    loop {
        let mut overlay_lost = false;
        for ev in daemon.poll_overlay_events() {
            if matches!(ev, OverlayEvent::Disconnected) {
                overlay_lost = true;
            }
        }
        if overlay_lost {
            eprintln!("mouselessd: overlay disconnected mid-session; ending");
            break;
        }

        match daemon.keyboard_mut().next_layer_event() {
            Ok(LayerEvent::LayerChanged(name)) if name != OVERLAY_LAYER => break,
            Ok(_) => {}
            Err(KbdError::Unavailable(_)) | Err(KbdError::Busy) => {}
            Err(KbdError::Io(msg)) => {
                if is_kanata_gone(&msg) {
                    kanata_gone = true;
                } else {
                    eprintln!("mouselessd: tcp during session ({msg}); ending session");
                }
                break;
            }
        }

        match daemon.poll_cell_key() {
            Ok(Some(key)) => match daemon.handle_cell_key(key) {
                Ok(SessionStep::Continue) => {}
                Ok(SessionStep::Clicked | SessionStep::Exit) => break,
                Err(e) => {
                    eprintln!("mouselessd: cell action failed ({e}); ending session");
                    break;
                }
            },
            Ok(None) => {}
            Err(e) => {
                eprintln!("mouselessd: cell read ({e}); ending session");
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(1));
    }

    daemon.end_overlay();
    // `end_overlay` already switched to base; this is a redundant guard for
    // kanata restarts that missed the layer change.
    if !kanata_gone {
        let _ = daemon
            .keyboard_mut()
            .set_read_timeout(Duration::from_millis(500));
    }
    kanata_gone
}

/// Kanata TCP EOF/reset means the process died (kill switch, plan §7).
fn is_kanata_gone(msg: &str) -> bool {
    msg.contains("EOF") || msg.contains("closed") || msg.contains("reset")
}
