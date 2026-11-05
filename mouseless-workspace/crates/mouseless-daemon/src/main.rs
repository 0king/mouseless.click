//! `mouselessd` binary: thin orchestration, Phase 2 production wiring.
//!
//! ```text
//! kanata TCP LayerChange → overlay/free session → EvdevCellGrab (fresh fd)
//!   → cell keys / free-mode key state → subgrid/click or 8 ms physics tick
//!   → Unix-socket Show/Highlight/ShowSubgrid/Hide frames (GNOME extension)
//! ```
//!
//! Kill switch (plan §7): kanata TCP EOF/reset → release grab, best-effort
//! hide broadcast, exit(1) so the supervisor restarts the stack in order.
//!
//! Needs `input` for `/dev/input` enumeration + grab and the existing
//! `uinput` group for `/dev/uinput` (kanata already has both);
//! `Group=mouseless` owns the overlay socket dir (plan §10.7). Human accounts
//! never get `input`/uinput.
//! Missing uinput degrades to "overlay tracks, clicks disabled"; missing
//! overlay degrades to "typing unaffected, no grab" (plan §6). Custom free
//! mode is relative and keeps working without the extension, but a missing
//! uinput device ends the session.

use mouseless_config::{load_effective, AppConfig, Config, ConfigError, ConfigStore};
use mouseless_daemon::{Daemon, DaemonConfig, DaemonError, SessionStep};
use mouseless_ipc::{ClientToDaemon, CursorPos, DaemonToClient};
use mouseless_kbd::{
    EvdevCellGrab, KanataSubscriber, KanataTcpConfig, KbdError, KeyboardBackend, LayerEvent,
};
use mouseless_mouse::impls::UinputMouse;
use mouseless_mouse::{MouseBackend, MouseButton, MouseError};
use mouseless_overlay::impls::{GnomeOverlay, SOCKET_GROUP};
use mouseless_overlay::{GridModel, MonitorLayout, OverlayBackend, OverlayError, OverlayEvent};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, Instant};

/// Production mouse: real uinput, or a loud no-op when `/dev/uinput` is denied.
enum ProdMouse {
    Uinput(UinputMouse),
    Disabled,
}

impl ProdMouse {
    fn create(name: &str, vendor: u16) -> Self {
        match UinputMouse::create_named(name, vendor) {
            Ok(m) => {
                println!("mouselessd: uinput {name} ready");
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

    fn button_down(&mut self, button: MouseButton) -> Result<(), Self::Error> {
        match self {
            Self::Uinput(m) => m.button_down(button),
            Self::Disabled => Err(MouseError::Io("uinput unavailable".into())),
        }
    }

    fn button_up(&mut self, button: MouseButton) -> Result<(), Self::Error> {
        match self {
            Self::Uinput(m) => m.button_up(button),
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
    fn bind(path: &str, group: &str) -> Self {
        match GnomeOverlay::bind_with_group(path, group) {
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

    fn highlight_column(&mut self, column: Option<u32>, cols: u32) -> Result<(), Self::Error> {
        match self {
            Self::Gnome(o) => o.highlight_column(column, cols),
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

    fn reply_client(&mut self, request_id: u64, reply: &DaemonToClient) -> Result<(), Self::Error> {
        match self {
            Self::Gnome(o) => o.reply_client(request_id, reply),
            Self::Disabled => Ok(()),
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

/// Why a session loop returned.
enum SessionEnd {
    /// Session ended on its own (Esc, click, auto-off, overlay loss).
    Done,
    /// Kanata TCP died; the caller must co-exit for a supervisor restart.
    KanataGone,
    /// Kanata switched to another layer; the caller applies the transition.
    Layer(String),
}

/// `--config PATH` / `--config=PATH` is resolved before the main pass.
fn config_path_arg() -> Option<PathBuf> {
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if let Some(path) = arg.strip_prefix("--config=") {
            return Some(PathBuf::from(path));
        }
        if arg == "--config" {
            return args.next().map(PathBuf::from);
        }
    }
    None
}

/// systemd sets `STATE_DIRECTORY`; `mouseless.toml` is the installed fallback.
fn default_state_dir(app: &AppConfig) -> PathBuf {
    std::env::var_os("STATE_DIRECTORY")
        .and_then(|value| std::env::split_paths(&value).next())
        .unwrap_or_else(|| PathBuf::from(&app.paths.state_dir))
}

fn parse_args(app: &AppConfig) -> (DaemonConfig, PathBuf) {
    let mut cfg = DaemonConfig::default();
    cfg.apply_app_config(app);
    let mut state_dir = default_state_dir(app);
    let mut state_flag: Option<PathBuf> = None;
    let mut kanata_flag: Option<String> = None;
    let mut socket_flag: Option<String> = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--config" => {
                // Already applied by `config_path_arg`; consume its value.
                let _ = args.next();
            }
            _ if a.starts_with("--config=") => {}
            "--kanata" => {
                if let Some(v) = args.next() {
                    kanata_flag = Some(v);
                }
            }
            "--socket" => {
                if let Some(v) = args.next() {
                    socket_flag = Some(v);
                }
            }
            "--state-dir" => {
                if let Some(v) = args.next() {
                    state_flag = Some(PathBuf::from(v));
                }
            }
            "--help" | "-h" => {
                println!(
                    "usage: mouselessd [--config PATH] [--kanata HOST:PORT] [--socket PATH] [--state-dir DIR] [--version]"
                );
                println!(
                    "defaults: config {} | kanata {} | socket {} | state {}",
                    mouseless_config::APP_CONFIG_PATH,
                    app.kanata.address,
                    app.paths.socket,
                    app.paths.state_dir
                );
                std::process::exit(0);
            }
            "--version" | "-V" => {
                println!("mouselessd {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            _ => {
                eprintln!("unknown arg {a}; see --help");
                std::process::exit(2);
            }
        }
    }
    // Dev overrides without touching the systemd unit contract. CLI flags
    // have the final word over environment variables and the TOML file.
    if let Ok(sock) = std::env::var("MOUSELESS_SOCKET") {
        cfg.socket_path = sock;
    }
    if let Ok(dir) = std::env::var("MOUSELESS_STATE_DIR") {
        state_dir = PathBuf::from(dir);
    }
    if let Some(addr) = kanata_flag {
        cfg.kanata_addr = addr;
    }
    if let Some(sock) = socket_flag {
        cfg.socket_path = sock;
    }
    if let Some(dir) = state_flag {
        state_dir = dir;
    }
    (cfg, state_dir)
}

fn main() -> ExitCode {
    let explicit = config_path_arg();
    let loaded = load_effective(explicit.as_deref());
    let app = match loaded.error {
        Some(error) => {
            eprintln!("mouselessd: {}: {error}", loaded.source.label());
            return ExitCode::from(2);
        }
        None => loaded.config,
    };

    let (mut cfg, state_dir) = parse_args(&app);
    let store = ConfigStore::new(app.state_config_path(&state_dir));
    // Load the daemon-owned last-applied config. Corruption falls back to
    // last-good, then schema defaults, and is logged rather than fatal: the
    // daemon must stay available to receive a corrected `Save`/`Update`.
    let (stored, config_error) = store.effective();
    match &config_error {
        Some(ConfigError::NotFound { .. }) => {
            println!(
                "mouselessd: no persisted config at {}; using mouseless.toml defaults",
                store.config_path().display()
            );
        }
        Some(error) => eprintln!("mouselessd: config fallback ({error}); using last-good"),
        None => println!("mouselessd: config {}", store.config_path().display()),
    }
    // A missing settings document keeps the TOML appearance/paths; an existing
    // (or last-good) document wins for every section it defines.
    if !matches!(config_error, Some(ConfigError::NotFound { .. })) {
        cfg.apply_config(&stored);
    }
    let start_overlay_layer = cfg.overlay_layer.clone();
    let start_free_layer = cfg.free_mode_layer.clone();

    let (host, port) = cfg
        .kanata_addr
        .rsplit_once(':')
        .and_then(|(host, port)| {
            port.parse::<u16>()
                .ok()
                .map(|port| (host.to_string(), port))
        })
        .unwrap_or_else(|| app.kanata.endpoint());
    let tcp_cfg = KanataTcpConfig {
        host,
        port,
        timeout: app.kanata.connect_timeout(),
    };

    let overlay = ProdOverlay::bind(&cfg.socket_path, SOCKET_GROUP);
    let mouse = ProdMouse::create(&app.devices.mouse_name, app.devices.vendor_id);
    let mut daemon = Daemon::with_grab(
        KanataSubscriber::new(tcp_cfg),
        mouse,
        overlay,
        cfg,
        EvdevCellGrab::acquire_named,
    );

    // Resync on (re)connect (plan §5): never assume the starting layer.
    let start = loop {
        match daemon.keyboard_mut().current_layer() {
            Ok(name) => break name,
            Err(e) => {
                eprintln!("mouselessd: kanata TCP unavailable ({e}); retrying…");
                std::thread::sleep(app.kanata.reconnect_backoff());
            }
        }
    };
    println!("mouselessd: kanata layer at startup: {start}");

    // A session may already be active when the daemon starts. Base and other
    // layers are reconciled through the idle loop without grabbing.
    let mut pending_layer = if start == start_overlay_layer || start == start_free_layer {
        Some(start)
    } else {
        None
    };

    loop {
        // Overlay reports must be current before a layer-event show.
        if let Err(e) = daemon
            .keyboard_mut()
            .set_read_timeout(app.daemon.idle_read_timeout())
        {
            eprintln!("mouselessd: idle read timeout: {e}");
        }
        if drain_overlay_events(&mut daemon, &store, &app) {
            eprintln!("mouselessd: overlay disconnected; releasing grab, typing unaffected");
            daemon.on_overlay_lost();
        }

        let name = match pending_layer.take() {
            Some(name) => name,
            None => match daemon.keyboard_mut().next_layer_event() {
                Ok(LayerEvent::LayerChanged(name)) => name,
                Err(KbdError::Unavailable(_)) | Err(KbdError::Busy) => continue,
                Err(KbdError::Io(msg)) => {
                    if is_kanata_gone(&msg) {
                        eprintln!(
                            "mouselessd: kanata lost ({msg}); co-exit for supervisor restart"
                        );
                        let _ = daemon.on_kanata_lost();
                        return ExitCode::from(1);
                    }
                    eprintln!("mouselessd: tcp ({msg}); backing off…");
                    std::thread::sleep(app.kanata.reconnect_backoff());
                    continue;
                }
            },
        };

        if name == daemon.overlay_layer() {
            let _ = drain_overlay_events(&mut daemon, &store, &app);
            match start_overlay_with_retry(&mut daemon, &app) {
                Ok(()) => match run_overlay_session(&mut daemon, &store, &app) {
                    SessionEnd::Done => {}
                    SessionEnd::KanataGone => {
                        let _ = daemon.on_kanata_lost();
                        return ExitCode::from(1);
                    }
                    SessionEnd::Layer(next) => pending_layer = Some(next),
                },
                Err(e) => {
                    eprintln!("mouselessd: overlay start degraded: {e}");
                    // Never leave the user on an overlay layer without a
                    // working grab: restore base so typing is unaffected.
                    daemon.end_overlay();
                }
            }
        } else if name == daemon.free_mode_layer() {
            match start_free_with_retry(&mut daemon, &app) {
                Ok(()) => {
                    if daemon.free_mode_active() {
                        match run_free_session(&mut daemon, &store, &app) {
                            SessionEnd::Done => {}
                            SessionEnd::KanataGone => {
                                let _ = daemon.on_kanata_lost();
                                return ExitCode::from(1);
                            }
                            SessionEnd::Layer(next) => pending_layer = Some(next),
                        }
                    }
                }
                Err(e) => {
                    eprintln!("mouselessd: free-mode start degraded: {e}");
                    daemon.end_free_mode();
                }
            }
        } else {
            // Base and kanata's own free-mode fallback both land here.
            // `on_layer_event` releases buttons/grab and restores base
            // except for the kanata fallback layer, which kanata chose.
            if let Err(e) = daemon.on_layer_event(LayerEvent::LayerChanged(name)) {
                eprintln!("mouselessd: layer transition failed: {e}");
            }
        }
    }
}

/// Start the overlay, retrying `EBUSY` within the configured budget.
fn start_overlay_with_retry(daemon: &mut ProdDaemon, app: &AppConfig) -> Result<(), DaemonError> {
    for _ in 0..app.daemon.grab_retry_attempts {
        match daemon.start_overlay() {
            Ok(()) => return Ok(()),
            Err(DaemonError::Kbd(KbdError::Busy)) => {
                std::thread::sleep(app.daemon.grab_retry_delay())
            }
            Err(e) => return Err(e),
        }
    }
    Err(DaemonError::Kbd(KbdError::Busy))
}

/// Start custom free mode, retrying `EBUSY` within the configured budget.
fn start_free_with_retry(daemon: &mut ProdDaemon, app: &AppConfig) -> Result<(), DaemonError> {
    for _ in 0..app.daemon.grab_retry_attempts {
        match daemon.start_free_mode() {
            Ok(()) => return Ok(()),
            Err(DaemonError::Kbd(KbdError::Busy)) => {
                std::thread::sleep(app.daemon.grab_retry_delay())
            }
            Err(e) => return Err(e),
        }
    }
    Err(DaemonError::Kbd(KbdError::Busy))
}

/// One overlay session. Returns why it ended; `Layer` means kanata already
/// switched and the caller must apply that transition without forcing base.
///
/// The grab is held ONLY here (plan §5 rule 1). Esc/click/layer-exit all end
/// the session; `end_overlay` releases the grab and restores the base layer.
fn run_overlay_session(
    daemon: &mut ProdDaemon,
    store: &ConfigStore,
    app: &AppConfig,
) -> SessionEnd {
    if let Err(e) = daemon
        .keyboard_mut()
        .set_read_timeout(app.daemon.session_read_timeout())
    {
        eprintln!("mouselessd: session read timeout: {e}");
    }

    let mut next_tick = Instant::now();
    loop {
        if drain_overlay_events(daemon, store, app) {
            eprintln!("mouselessd: overlay disconnected mid-session; ending");
            break;
        }

        match daemon.keyboard_mut().next_layer_event() {
            Ok(LayerEvent::LayerChanged(name)) => {
                if name != daemon.overlay_layer() {
                    return SessionEnd::Layer(name);
                }
            }
            Err(KbdError::Unavailable(_)) | Err(KbdError::Busy) => {}
            Err(KbdError::Io(msg)) => {
                if is_kanata_gone(&msg) {
                    return SessionEnd::KanataGone;
                }
                eprintln!("mouselessd: tcp during session ({msg}); ending session");
                break;
            }
        }

        // Drain every queued press/release before the next physics/tick step.
        let mut end_session = false;
        loop {
            match daemon.poll_key_event() {
                Ok(Some(ev)) => match daemon.handle_key_event(ev) {
                    Ok(SessionStep::Continue) => {}
                    Ok(SessionStep::Clicked) => {
                        // Multi-click/continuous/drag sessions stay alive.
                        if !daemon.session_active() {
                            end_session = true;
                            break;
                        }
                    }
                    Ok(SessionStep::Exit) => {
                        end_session = true;
                        break;
                    }
                    Err(e) => {
                        eprintln!("mouselessd: cell action failed ({e}); ending session");
                        end_session = true;
                        break;
                    }
                },
                Ok(None) => break,
                Err(e) => {
                    eprintln!("mouselessd: cell read ({e}); ending session");
                    end_session = true;
                    break;
                }
            }
        }
        if end_session {
            break;
        }

        let now = Instant::now();
        if now >= next_tick {
            if let Err(e) = daemon.tick_now() {
                eprintln!("mouselessd: overlay tick failed ({e}); ending session");
                break;
            }
            next_tick = now + app.daemon.free_mode_tick();
            if !daemon.session_active() {
                break;
            }
        } else {
            std::thread::sleep((next_tick - now).min(Duration::from_millis(1)));
        }
    }

    daemon.end_overlay();
    SessionEnd::Done
}

/// One custom free-mode session: grab held, no overlay, fixed 8 ms physics.
///
/// Free mode is relative, so an overlay disconnect only hides the overlay and
/// leaves the session running. Esc, auto-off, uinput failure, or a layer change
/// end it; held buttons are always released by `end_free_mode`/teardown.
fn run_free_session(daemon: &mut ProdDaemon, store: &ConfigStore, app: &AppConfig) -> SessionEnd {
    if let Err(e) = daemon
        .keyboard_mut()
        .set_read_timeout(app.daemon.session_read_timeout())
    {
        eprintln!("mouselessd: free-mode read timeout: {e}");
    }

    let mut next_tick = Instant::now();
    loop {
        if drain_overlay_events(daemon, store, app) {
            // Relative motion keeps working; only the overlay is gone.
            eprintln!("mouselessd: overlay disconnected; free mode continues");
            daemon.on_overlay_lost();
        }

        match daemon.keyboard_mut().next_layer_event() {
            Ok(LayerEvent::LayerChanged(name)) => {
                if name != daemon.free_mode_layer() {
                    return SessionEnd::Layer(name);
                }
            }
            Err(KbdError::Unavailable(_)) | Err(KbdError::Busy) => {}
            Err(KbdError::Io(msg)) => {
                if is_kanata_gone(&msg) {
                    return SessionEnd::KanataGone;
                }
                eprintln!("mouselessd: tcp during free mode ({msg}); ending session");
                break;
            }
        }

        let mut end_session = false;
        loop {
            match daemon.poll_key_event() {
                Ok(Some(ev)) => match daemon.handle_key_event(ev) {
                    Ok(SessionStep::Continue) => {}
                    Ok(SessionStep::Clicked) => {}
                    Ok(SessionStep::Exit) => {
                        end_session = true;
                        break;
                    }
                    Err(e) => {
                        eprintln!("mouselessd: free-mode key failed ({e}); ending session");
                        end_session = true;
                        break;
                    }
                },
                Ok(None) => break,
                Err(e) => {
                    eprintln!("mouselessd: free-mode read ({e}); ending session");
                    end_session = true;
                    break;
                }
            }
        }
        if end_session {
            break;
        }

        let now = Instant::now();
        if now >= next_tick {
            if let Err(e) = daemon.tick_now() {
                eprintln!("mouselessd: free-mode tick failed ({e}); ending session");
                break;
            }
            next_tick = now + app.daemon.free_mode_tick();
            if !daemon.free_mode_active() {
                break;
            }
        } else {
            std::thread::sleep((next_tick - now).min(Duration::from_millis(1)));
        }
    }

    // Layer transitions are applied by the caller (so `lyr-mouse` is not
    // overridden); every other exit path must release buttons + restore base.
    if daemon.free_mode_active() {
        daemon.end_free_mode();
    }
    SessionEnd::Done
}

/// Drain overlay events, applying any CLI/GUI `Reload`/`Save`/`Update`
/// requests. Returns whether a helloed overlay disconnected.
fn drain_overlay_events(daemon: &mut ProdDaemon, store: &ConfigStore, app: &AppConfig) -> bool {
    let mut lost = false;
    for ev in daemon.poll_overlay_events() {
        log_overlay_event(&ev, app.daemon.debug_layout);
        match ev {
            OverlayEvent::Disconnected => lost = true,
            OverlayEvent::ClientCommand {
                request_id,
                command,
            } => process_client_command(daemon, store, request_id, &command),
            _ => {}
        }
    }
    lost
}

fn process_client_command(
    daemon: &mut ProdDaemon,
    store: &ConfigStore,
    request_id: u64,
    command: &ClientToDaemon,
) {
    let reply = match command {
        ClientToDaemon::Reload => match store.load() {
            Ok(config) => apply_candidate(daemon, &config, None),
            Err(error) => config_error(&error),
        },
        ClientToDaemon::Update { config } => apply_candidate(daemon, config, Some(store)),
        ClientToDaemon::Save { config } => save_candidate(config, store),
        ClientToDaemon::Status => DaemonToClient::Status {
            daemon_version: env!("CARGO_PKG_VERSION").into(),
            overlay_connected: daemon.overlay_connected(),
            session_active: daemon.session_active(),
            free_mode: daemon.free_mode_active(),
            config_path: store.config_path().display().to_string(),
        },
        // The effective document is exactly what the daemon would reload: the
        // persisted file, last-good, or defaults. `Save` persists without
        // changing the running app; `Update` persists and applies.
        ClientToDaemon::GetConfig => DaemonToClient::Config {
            config: Box::new(store.effective().0),
        },
    };
    match &reply {
        DaemonToClient::Ack { message } => println!("mouselessd: client ack: {message}"),
        DaemonToClient::Error { code, message } => {
            eprintln!("mouselessd: client error ({code}): {message}");
        }
        DaemonToClient::Status { .. } | DaemonToClient::Config { .. } => {}
    }
    if let Err(e) = daemon.reply_client(request_id, &reply) {
        eprintln!("mouselessd: client reply failed: {e}");
    }
}

/// Validate, optionally persist, then swap the live config. Persistence
/// happens before the runtime swap so a failed write leaves the old behavior
/// in place.
fn apply_candidate(
    daemon: &mut ProdDaemon,
    config: &Config,
    persist: Option<&ConfigStore>,
) -> DaemonToClient {
    if let Err(error) = mouseless_config::validate(config) {
        return DaemonToClient::Error {
            code: "invalid_config".into(),
            message: error.to_string(),
        };
    }
    if let Some(store) = persist {
        if let Err(error) = store.save(config) {
            return config_error(&error);
        }
    }
    let mut next = daemon.config().clone();
    next.apply_config(config);
    daemon.apply_config(next);
    DaemonToClient::Ack {
        message: if persist.is_some() {
            "configuration saved and applied"
        } else {
            "configuration reloaded"
        }
        .into(),
    }
}

/// Validate and persist a candidate without swapping the live runtime config.
///
/// This is the auto-save path: the running app keeps its current behavior
/// until a later `Update` or `Reload` applies the saved document.
fn save_candidate(config: &Config, store: &ConfigStore) -> DaemonToClient {
    if let Err(error) = mouseless_config::validate(config) {
        return DaemonToClient::Error {
            code: "invalid_config".into(),
            message: error.to_string(),
        };
    }
    match store.save(config) {
        Ok(()) => DaemonToClient::Ack {
            message: "configuration saved; restart the app to apply it".into(),
        },
        Err(error) => config_error(&error),
    }
}

fn config_error(error: &ConfigError) -> DaemonToClient {
    let code = match error {
        ConfigError::NotFound { .. } => "not_found",
        ConfigError::Parse { .. } => "parse",
        ConfigError::UnsupportedVersion { .. } => "unsupported_version",
        ConfigError::CheckFailed { .. } => "check_failed",
        ConfigError::Io { .. } => "io",
        ConfigError::Invalid(_) => "invalid_config",
    };
    DaemonToClient::Error {
        code: code.into(),
        message: error.to_string(),
    }
}

/// Kanata TCP EOF/reset means the process died (kill switch, plan §7).
fn is_kanata_gone(msg: &str) -> bool {
    msg.contains("EOF") || msg.contains("closed") || msg.contains("reset")
}

/// `MOUSELESS_DEBUG_LAYOUT=1` logs overlay geometry/cursor reports.
///
/// Off by default. Useful when diagnosing multi-monitor offsets: the report
/// carries the extension's physical-pixel layout and cursor position.
fn log_overlay_event(ev: &OverlayEvent, enabled: bool) {
    if !enabled && std::env::var_os("MOUSELESS_DEBUG_LAYOUT").is_none() {
        return;
    }
    match ev {
        OverlayEvent::Report { layout, cursor } => eprintln!(
            "mouselessd: report cursor=({}, {}) monitors={:?}",
            cursor.x, cursor.y, layout.monitors
        ),
        OverlayEvent::CursorReply { cursor } => {
            eprintln!("mouselessd: cursor reply=({}, {})", cursor.x, cursor.y)
        }
        OverlayEvent::ClientCommand { command, .. } => {
            eprintln!("mouselessd: client command {command:?}")
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mouseless_ipc::{decode_for, encode, KIND_CLIENT, KIND_DAEMON};
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;

    /// Auto-save must persist without touching the running daemon. This test
    /// pins the persistence half; the runtime swap only happens in
    /// `apply_candidate`, which `Save` never calls.
    #[test]
    fn save_candidate_persists_without_applying() {
        let dir = std::env::temp_dir().join(format!(
            "mouseless-save-candidate-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let store = ConfigStore::for_daemon(&dir);
        let mut config = Config::default();
        config.behavior.default_action = mouseless_config::DefaultAction::Move;

        let reply = save_candidate(&config, &store);
        assert!(matches!(
            reply,
            DaemonToClient::Ack { ref message } if message.contains("saved")
        ));
        assert_eq!(store.load().unwrap(), config);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_candidate_rejects_invalid_config_without_writing() {
        let dir = std::env::temp_dir().join(format!(
            "mouseless-save-invalid-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let store = ConfigStore::for_daemon(&dir);
        let mut config = Config::default();
        config.grid.columns.clear();

        let reply = save_candidate(&config, &store);
        assert!(matches!(
            reply,
            DaemonToClient::Error { ref code, .. } if code == "invalid_config"
        ));
        assert!(store.load().is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Regression: `ProdOverlay` must forward client replies. The trait has a
    /// no-op default, and omitting this delegation makes every `Reload`,
    /// `Save`, `Update`, `Status`, and `GetConfig` caller time out even though
    /// the daemon processed the request.
    #[test]
    fn prod_overlay_forwards_client_replies() {
        let dir = std::env::temp_dir().join(format!(
            "mouseless-prod-overlay-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("daemon.sock");
        let path_str = path.to_str().unwrap().to_string();
        let mut overlay = ProdOverlay::bind(&path_str, "mouseless");

        let mut client = UnixStream::connect(&path).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let request = encode(KIND_CLIENT, &ClientToDaemon::Status).unwrap();
        client.write_all(format!("{request}\n").as_bytes()).unwrap();

        let request_id = (0..100)
            .find_map(|_| {
                let found = overlay
                    .poll_events()
                    .into_iter()
                    .find_map(|event| match event {
                        OverlayEvent::ClientCommand {
                            request_id,
                            command,
                        } => {
                            assert_eq!(command, ClientToDaemon::Status);
                            Some(request_id)
                        }
                        _ => None,
                    });
                if found.is_none() {
                    std::thread::sleep(Duration::from_millis(10));
                }
                found
            })
            .expect("client command queued");

        let reply = DaemonToClient::Status {
            daemon_version: "test".into(),
            overlay_connected: false,
            session_active: false,
            free_mode: false,
            config_path: "/tmp/mouseless/config.yaml".into(),
        };
        overlay.reply_client(request_id, &reply).unwrap();

        let mut reader = BufReader::new(client);
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let envelope = decode_for(&line, KIND_DAEMON).unwrap();
        assert_eq!(envelope.payload["type"], "Status");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
