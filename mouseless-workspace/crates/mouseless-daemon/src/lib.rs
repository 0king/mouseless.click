//! `mouseless-daemon`: thin orchestration over trait backends.
//!
//! The daemon owns NO backend logic (plan §1.2): it wires a [`KeyboardBackend`],
//! [`MouseBackend`], and [`OverlayBackend`] together behind a single-flight
//! overlay machine (one mutex, idempotent show/hide — plan §5).
//!
//! Kill switch (plan §7, audit W2): kanata TCP EOF/reset means kanata died —
//! release any held [`CellGrab`], best-effort overlay-hide, then co-exit with
//! a failure code so the supervisor restarts the stack in order.
//!
//! Degraded modes (plan §6): no overlay client or no monitor report means NO
//! grab and a base-layer restore — typing is never affected by a broken
//! overlay. No uinput device means clicks are skipped but the overlay still
//! tracks.

use mouseless_core::{cell_bounds, cell_center, subcell_bounds, GridConfig, Monitor, Point};
use mouseless_ipc::{CursorPos, GridCell, GridModel, MonitorLayout};
use mouseless_kbd::{CellGrab, CellKey, CellKeySource, KbdError, KeyboardBackend, LayerEvent};
use mouseless_mouse::{MouseBackend, MouseButton, MouseError};
use mouseless_overlay::{OverlayBackend, OverlayError, OverlayEvent};
use mouseless_state::{OverlayMachine, Phase, StateError};
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};
use thiserror::Error;

/// Layer that shows the overlay (kanata include).
pub const OVERLAY_LAYER: &str = "lyr-mouseless";
/// Base layer restored on every exit path (idempotent).
pub const BASE_LAYER: &str = "lyr-base";
/// Cursor refresh budget before a click (SPEC-daemon step 6).
pub const CURSOR_QUERY_TIMEOUT: Duration = Duration::from_millis(100);

/// Daemon errors (all fallible ops return `Result` with context, plan §6).
#[derive(Debug, Error)]
pub enum DaemonError {
    /// Keyboard backend failure.
    #[error("keyboard: {0}")]
    Kbd(#[from] KbdError),
    /// Mouse backend failure.
    #[error("mouse: {0}")]
    Mouse(#[from] MouseError),
    /// Overlay backend failure (degraded: keyboard/mouse stay safe).
    #[error("overlay: {0}")]
    Overlay(#[from] OverlayError),
    /// State-machine event rejected (never expected in the session flow).
    #[error("state: {0}")]
    State(#[from] StateError),
    /// Kanata died (TCP EOF/reset) — caller must release + co-exit (plan §7).
    #[error("kanata connection lost; co-exit for supervisor restart")]
    KanataLost,
    /// No overlay client: do not grab; restore base so typing is unaffected.
    #[error("overlay unavailable; typing unaffected")]
    OverlayUnavailable,
    /// Extension has not reported a monitor layout yet.
    #[error("no monitor layout reported; overlay unavailable")]
    NoLayout,
    /// Cursor position is required for a click but no report arrived.
    #[error("no cursor report; click skipped")]
    NoCursor,
    /// Cell index derived from config was invalid (never expected).
    #[error("invalid cell selection: {0}")]
    InvalidCell(u32),
}

/// Backend selection is config, never a refactor (plan §3 rule 2).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonConfig {
    /// `keyboard_backend: kanata` (today) | `keyd` | `evdev_raw` (Phase 4).
    pub keyboard_backend: String,
    /// `overlay_backend: gnome` (today) | `wlr` | `x11` (Phase 4).
    pub overlay_backend: String,
    /// Kanata TCP address (localhost only), e.g. `127.0.0.1:7070`.
    pub kanata_addr: String,
    /// Overlay socket path ([`mouseless_ipc::SOCKET_PATH`]).
    pub socket_path: String,
    /// Level-1 grid (MVP default 4×2, keys `a s d f j k l ;`).
    pub grid: GridConfig,
    /// Subgrid inside a selected cell (same keys by default).
    pub subgrid: GridConfig,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            keyboard_backend: "kanata".into(),
            overlay_backend: "gnome".into(),
            kanata_addr: "127.0.0.1:7070".into(),
            socket_path: mouseless_ipc::SOCKET_PATH.into(),
            grid: default_grid(),
            subgrid: default_grid(),
        }
    }
}

/// MVP default grid: 4 columns × 2 rows, row-major.
pub fn default_grid() -> GridConfig {
    GridConfig {
        cols: 4,
        rows: 2,
        keys: vec!['a', 's', 'd', 'f', 'j', 'k', 'l', ';'],
    }
}

/// Result of handling one grabbed cell key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionStep {
    /// Stay in the session (e.g. a cell was highlighted).
    Continue,
    /// A click was executed; the caller must end the session.
    Clicked,
    /// Escape/layer exit; the caller must end the session.
    Exit,
}

/// Thin orchestrator, generic over backends for testability.
pub struct Daemon<K, M, O, G = CellGrab> {
    kbd: K,
    mouse: M,
    overlay: O,
    config: DaemonConfig,
    machine: OverlayMachine,
    grab: Option<G>,
    grab_factory: fn() -> Result<G, KbdError>,
    selected_cell: Option<u32>,
    active_monitor: Option<Monitor>,
}

impl<K, M, O> Daemon<K, M, O, CellGrab>
where
    K: KeyboardBackend<Error = KbdError>,
    M: MouseBackend<Error = MouseError>,
    O: OverlayBackend<Error = OverlayError>,
{
    /// Wire already-constructed backends together with default config.
    pub fn new(kbd: K, mouse: M, overlay: O) -> Self {
        Self::with_grab(
            kbd,
            mouse,
            overlay,
            DaemonConfig::default(),
            CellGrab::acquire,
        )
    }

    /// Wire backends with an explicit config (grid/monitor paths).
    pub fn with_config(kbd: K, mouse: M, overlay: O, config: DaemonConfig) -> Self {
        Self::with_grab(kbd, mouse, overlay, config, CellGrab::acquire)
    }
}

impl<K, M, O, G> Daemon<K, M, O, G>
where
    K: KeyboardBackend<Error = KbdError>,
    M: MouseBackend<Error = MouseError>,
    O: OverlayBackend<Error = OverlayError>,
    G: CellKeySource,
{
    /// Wire backends with an explicit config and grab factory.
    ///
    /// Production passes [`mouseless_kbd::EvdevCellGrab::acquire`]; tests pass
    /// a fake factory so no `/dev/input` access is needed.
    pub fn with_grab(
        kbd: K,
        mouse: M,
        overlay: O,
        config: DaemonConfig,
        grab_factory: fn() -> Result<G, KbdError>,
    ) -> Self {
        Self {
            kbd,
            mouse,
            overlay,
            config,
            machine: OverlayMachine::new(),
            grab: None,
            grab_factory,
            selected_cell: None,
            active_monitor: None,
        }
    }

    /// Handle one layer event.
    ///
    /// `lyr-mouseless` shows the overlay (grab acquired only when a live
    /// overlay client + monitor layout exist); anything else hides it.
    /// Duplicate events are safe: show/hide are idempotent (plan §5).
    pub fn on_layer_event(&mut self, ev: LayerEvent) -> Result<(), DaemonError> {
        match ev {
            LayerEvent::LayerChanged(name) if name == OVERLAY_LAYER => self.start_overlay(),
            LayerEvent::LayerChanged(_) => {
                self.end_overlay();
                Ok(())
            }
        }
    }

    /// Start an overlay session: verify the extension, compute the grid, grab
    /// cell keys, and render. Any failure degrades to base (typing unaffected).
    pub fn start_overlay(&mut self) -> Result<(), DaemonError> {
        if !self.overlay.is_connected() {
            self.degrade_base();
            return Err(DaemonError::OverlayUnavailable);
        }
        let Some(layout) = self.overlay.last_layout() else {
            self.degrade_base();
            return Err(DaemonError::NoLayout);
        };
        let cursor = self.overlay.last_cursor();
        let Some(monitor) = pick_monitor(&layout, cursor) else {
            self.degrade_base();
            return Err(DaemonError::NoLayout);
        };
        let model = build_grid_model(&self.config.grid, &monitor);
        // Grab before showing: if the grab fails, the user sees no overlay
        // and typing continues on the base layer (plan §5 rule 5).
        if self.grab.is_none() {
            self.grab = Some((self.grab_factory)()?);
        }
        // Any failure after the grab is held must release it again: a grab
        // without a running session would swallow every keystroke (matrix #2).
        if let Err(e) = self
            .machine
            .show()
            .map_err(DaemonError::from)
            .and_then(|()| self.overlay.show(&model).map_err(DaemonError::from))
        {
            self.grab = None;
            self.degrade_base();
            return Err(e);
        }
        self.selected_cell = None;
        self.active_monitor = Some(monitor);
        Ok(())
    }

    /// End the session on every exit path: release the grab (Drop), hide the
    /// overlay, and restore the base layer idempotently.
    pub fn end_overlay(&mut self) {
        self.grab = None; // RAII: drain + ungrab + close (audits G6/G7)
        self.selected_cell = None;
        self.active_monitor = None;
        let _ = self.machine.hide();
        let _ = self.overlay.hide();
        let _ = self.kbd.set_layer(BASE_LAYER);
    }

    /// Overlay client disconnected (Shell restart/crash): never keep the grab.
    pub fn on_overlay_lost(&mut self) {
        self.end_overlay();
    }

    /// Poll the overlay backend for reports/cursor replies (non-blocking).
    pub fn poll_overlay_events(&mut self) -> Vec<OverlayEvent> {
        self.overlay.poll_events()
    }

    /// Poll the grabbed device for one cell key press (non-blocking).
    pub fn poll_cell_key(&mut self) -> Result<Option<CellKey>, DaemonError> {
        match self.grab.as_mut() {
            Some(grab) => Ok(grab.next_key_press()?),
            None => Ok(None),
        }
    }

    /// Handle one cell key. Level-1 keys open the subgrid; the second key (or
    /// Space for a cell center) executes the click (SPEC-daemon OQ-2).
    pub fn handle_cell_key(&mut self, key: CellKey) -> Result<SessionStep, DaemonError> {
        match key {
            CellKey::Escape => {
                self.end_overlay();
                Ok(SessionStep::Exit)
            }
            CellKey::Space => {
                let Some(parent) = self.selected_cell else {
                    return Ok(SessionStep::Continue);
                };
                let monitor = self.active_monitor.ok_or(DaemonError::NoLayout)?;
                let target = cell_center(
                    &self.config.grid,
                    &monitor,
                    cell_id(&self.config.grid, parent),
                );
                self.click_target(MouseButton::Left, target)?;
                Ok(SessionStep::Clicked)
            }
            CellKey::Char(c) => {
                if let Some(parent) = self.selected_cell {
                    let Some(sub) = index_of(&self.config.subgrid.keys, c) else {
                        return Ok(SessionStep::Continue);
                    };
                    let monitor = self.active_monitor.ok_or(DaemonError::NoLayout)?;
                    let parent_rect = cell_bounds(
                        &self.config.grid,
                        &monitor,
                        cell_id(&self.config.grid, parent),
                    );
                    let sub_rect = subcell_bounds(
                        &parent_rect,
                        &self.config.subgrid,
                        cell_id(&self.config.subgrid, sub),
                    );
                    self.click_target(MouseButton::Left, sub_rect.center())?;
                    Ok(SessionStep::Clicked)
                } else {
                    let Some(index) = index_of(&self.config.grid.keys, c) else {
                        return Ok(SessionStep::Continue);
                    };
                    let monitor = self.active_monitor.ok_or(DaemonError::NoLayout)?;
                    let model = build_subgrid_model(
                        &self.config.grid,
                        &self.config.subgrid,
                        &monitor,
                        index,
                    )?;
                    self.selected_cell = Some(index);
                    self.machine.open_subgrid()?;
                    self.overlay.show_subgrid(index, &model)?;
                    Ok(SessionStep::Continue)
                }
            }
        }
    }

    /// Click at an absolute physical-pixel target using the freshest cursor
    /// report available (QueryCursor budget ≤ 100 ms), then hide.
    pub fn click_target(&mut self, button: MouseButton, target: Point) -> Result<(), DaemonError> {
        let cursor = self
            .refresh_cursor(CURSOR_QUERY_TIMEOUT)
            .ok_or(DaemonError::NoCursor)?;
        self.click_relative(
            button,
            cursor,
            (target.x.round() as i32, target.y.round() as i32),
        )
    }

    /// Click at the extension-reported cursor offset by the target and hide.
    pub fn click_relative(
        &mut self,
        button: MouseButton,
        cursor: CursorPos,
        target: (i32, i32),
    ) -> Result<(), DaemonError> {
        let (dx, dy) = mouseless_mouse::compute_delta(target, (cursor.x, cursor.y));
        self.mouse.move_rel(dx, dy)?;
        self.mouse.click(button)?;
        let _ = self.machine.click();
        self.end_overlay();
        Ok(())
    }

    /// Ask the extension for a fresh cursor position, waiting up to `timeout`.
    fn refresh_cursor(&mut self, timeout: Duration) -> Option<CursorPos> {
        if self.overlay.query_cursor().is_err() {
            return self.overlay.last_cursor();
        }
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            for ev in self.overlay.poll_events() {
                if let OverlayEvent::CursorReply { cursor } = ev {
                    return Some(cursor);
                }
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        self.overlay.last_cursor()
    }

    /// Restore base and clear state without touching the grab (never held here).
    fn degrade_base(&mut self) {
        self.selected_cell = None;
        self.active_monitor = None;
        let _ = self.machine.hide();
        let _ = self.overlay.hide();
        let _ = self.kbd.set_layer(BASE_LAYER);
    }

    /// Handle kanata TCP EOF/reset: release grab, best-effort hide, signal
    /// co-exit so the supervisor restarts the stack in order (plan §7).
    pub fn on_kanata_lost(&mut self) -> DaemonError {
        self.grab = None;
        self.selected_cell = None;
        let _ = self.overlay.hide(); // best-effort, idempotent
        DaemonError::KanataLost
    }

    /// Whether the overlay is currently shown.
    pub fn overlay_visible(&self) -> bool {
        !matches!(self.machine.phase(), Phase::Normal)
    }

    /// Current state-machine phase (tests/diagnostics).
    pub fn phase(&self) -> Phase {
        self.machine.phase()
    }

    /// Whether a cell grab is held (tests/diagnostics).
    pub fn grab_held(&self) -> bool {
        self.grab.as_ref().is_some_and(CellKeySource::held)
    }

    /// Mutable access to the grab (tests inject fake keys).
    pub fn grab_mut(&mut self) -> Option<&mut G> {
        self.grab.as_mut()
    }

    /// Mutable access to the keyboard backend (event loop polls it).
    pub fn keyboard_mut(&mut self) -> &mut K {
        &mut self.kbd
    }

    /// Access the overlay backend (diagnostics/tests).
    pub fn overlay(&self) -> &O {
        &self.overlay
    }
}

/// Pick the monitor under the cursor, falling back to the primary monitor.
fn pick_monitor(layout: &MonitorLayout, cursor: Option<CursorPos>) -> Option<Monitor> {
    match cursor {
        Some(cursor) => layout.monitor_at(cursor),
        None => layout.primary(),
    }
}

/// Row-major index → cell id (validated against the config).
fn cell_id(config: &GridConfig, index: u32) -> mouseless_core::CellId {
    mouseless_core::CellId {
        col: index % config.cols,
        row: (index / config.cols).min(config.rows.saturating_sub(1)),
    }
}

fn index_of(keys: &[char], c: char) -> Option<u32> {
    keys.iter().position(|k| *k == c).map(|i| i as u32)
}

/// Build the level-1 wire model from pure geometry (never render in JS).
pub fn build_grid_model(config: &GridConfig, monitor: &Monitor) -> GridModel {
    let cells = config
        .keys
        .iter()
        .enumerate()
        .map(|(i, label)| {
            GridCell::new(
                i as u32,
                label.to_string(),
                cell_bounds(config, monitor, cell_id(config, i as u32)),
            )
        })
        .collect();
    GridModel {
        monitor: *monitor,
        cells,
        highlight: None,
    }
}

/// Build a subgrid model nested inside parent cell `parent`.
pub fn build_subgrid_model(
    config: &GridConfig,
    sub: &GridConfig,
    monitor: &Monitor,
    parent: u32,
) -> Result<GridModel, DaemonError> {
    if parent >= config.cell_count() {
        return Err(DaemonError::InvalidCell(parent));
    }
    let parent_rect = cell_bounds(config, monitor, cell_id(config, parent));
    let cells = sub
        .keys
        .iter()
        .enumerate()
        .map(|(i, label)| {
            GridCell::new(
                i as u32,
                label.to_string(),
                subcell_bounds(&parent_rect, sub, cell_id(sub, i as u32)),
            )
        })
        .collect();
    Ok(GridModel {
        monitor: *monitor,
        cells,
        highlight: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use mouseless_ipc::{CursorPos, MonitorInfo};
    use mouseless_kbd::impls::FakeKeyboard;
    use mouseless_mouse::impls::FakeMouse;
    use mouseless_overlay::impls::FakeOverlay;

    fn layout_1080p() -> MonitorLayout {
        MonitorLayout {
            monitors: vec![MonitorInfo {
                monitor: Monitor {
                    x: 0,
                    y: 0,
                    w: 1920,
                    h: 1080,
                    scale: 1.0,
                },
                primary: true,
            }],
        }
    }

    fn daemon() -> Daemon<FakeKeyboard, FakeMouse, FakeOverlay> {
        let mut overlay = FakeOverlay::default();
        overlay.set_layout(layout_1080p());
        overlay.set_cursor(CursorPos { x: 320, y: 270 });
        Daemon::new(FakeKeyboard::new(BASE_LAYER), FakeMouse::default(), overlay)
    }

    #[test]
    fn layer_switch_drives_grab_and_shows_grid() {
        let mut d = daemon();
        d.on_layer_event(LayerEvent::LayerChanged(OVERLAY_LAYER.into()))
            .unwrap();
        assert!(d.overlay_visible());
        assert!(d.grab_held());
        assert!(d.overlay().visible);
        let model = d.overlay().last_model.as_ref().unwrap();
        assert_eq!(model.cells.len(), 8);
        assert_eq!(model.cells[0].label, "a");
        assert_eq!((model.cells[0].w, model.cells[0].h), (480.0, 540.0));
        // Duplicate LayerChange: still exactly one grab (idempotent).
        d.on_layer_event(LayerEvent::LayerChanged(OVERLAY_LAYER.into()))
            .unwrap();
        assert!(d.grab_held());
        d.on_layer_event(LayerEvent::LayerChanged(BASE_LAYER.into()))
            .unwrap();
        assert!(!d.overlay_visible());
        assert!(!d.grab_held());
    }

    #[test]
    fn no_overlay_client_degrades_to_base_without_grab() {
        let mut overlay = FakeOverlay::default();
        overlay.set_layout(layout_1080p());
        overlay.disconnect();
        let mut d = Daemon::new(FakeKeyboard::new(BASE_LAYER), FakeMouse::default(), overlay);
        let err = d
            .on_layer_event(LayerEvent::LayerChanged(OVERLAY_LAYER.into()))
            .unwrap_err();
        assert!(matches!(err, DaemonError::OverlayUnavailable));
        assert!(!d.grab_held());
        assert!(!d.overlay_visible());
        assert_eq!(d.kbd.set_layer_calls, vec![BASE_LAYER.to_string()]);
    }

    #[test]
    fn show_failure_releases_grab_and_restores_base() {
        // Matrix #2: a grab acquired before a failed show must never survive
        // without a session, or every keystroke would be swallowed.
        let mut overlay = FakeOverlay::default();
        overlay.set_layout(layout_1080p());
        overlay.set_cursor(CursorPos { x: 320, y: 270 });
        overlay.fail_show = true;
        let mut d = Daemon::new(FakeKeyboard::new(BASE_LAYER), FakeMouse::default(), overlay);
        let err = d
            .on_layer_event(LayerEvent::LayerChanged(OVERLAY_LAYER.into()))
            .unwrap_err();
        assert!(matches!(err, DaemonError::Overlay(_)));
        assert!(!d.grab_held());
        assert!(!d.overlay_visible());
        assert_eq!(d.kbd.set_layer_calls, vec![BASE_LAYER.to_string()]);
    }

    #[test]
    fn no_layout_degrades_to_base_without_grab() {
        let mut d = Daemon::new(
            FakeKeyboard::new(BASE_LAYER),
            FakeMouse::default(),
            FakeOverlay::default(), // connected but never reported
        );
        let err = d
            .on_layer_event(LayerEvent::LayerChanged(OVERLAY_LAYER.into()))
            .unwrap_err();
        assert!(matches!(err, DaemonError::NoLayout));
        assert!(!d.grab_held());
    }

    #[test]
    fn cell_key_opens_subgrid_then_second_key_clicks_subcell_center() {
        let mut d = daemon();
        d.on_layer_event(LayerEvent::LayerChanged(OVERLAY_LAYER.into()))
            .unwrap();
        // 'a' selects cell 0 (top-left, 480×540); subgrid key 'd' is the
        // third-quarter sub-cell on row 0: center = (300, 135).
        assert_eq!(
            d.handle_cell_key(CellKey::Char('a')).unwrap(),
            SessionStep::Continue
        );
        assert_eq!(d.phase(), Phase::Subgrid);
        assert_eq!(
            d.handle_cell_key(CellKey::Char('d')).unwrap(),
            SessionStep::Clicked
        );
        // Target (300, 135) minus fake cursor (320, 270).
        assert_eq!(d.mouse.moves, vec![(-20, -135)]);
        assert_eq!(d.mouse.clicks, vec![MouseButton::Left]);
        assert!(!d.overlay_visible());
        assert!(!d.grab_held());
        assert_eq!(d.kbd.set_layer_calls, vec![BASE_LAYER.to_string()]);
    }

    #[test]
    fn space_clicks_selected_cell_center() {
        let mut d = daemon();
        d.on_layer_event(LayerEvent::LayerChanged(OVERLAY_LAYER.into()))
            .unwrap();
        d.handle_cell_key(CellKey::Char('a')).unwrap();
        assert_eq!(
            d.handle_cell_key(CellKey::Space).unwrap(),
            SessionStep::Clicked
        );
        // Cell center (240, 270) minus cursor (320, 270).
        assert_eq!(d.mouse.moves, vec![(-80, 0)]);
        assert!(!d.overlay_visible());
    }

    #[test]
    fn escape_ends_session_and_releases_grab() {
        let mut d = daemon();
        d.on_layer_event(LayerEvent::LayerChanged(OVERLAY_LAYER.into()))
            .unwrap();
        assert_eq!(
            d.handle_cell_key(CellKey::Escape).unwrap(),
            SessionStep::Exit
        );
        assert!(!d.grab_held());
        assert!(!d.overlay_visible());
        assert_eq!(d.kbd.set_layer_calls, vec![BASE_LAYER.to_string()]);
    }

    #[test]
    fn overlay_loss_releases_grab_and_hides() {
        let mut d = daemon();
        d.on_layer_event(LayerEvent::LayerChanged(OVERLAY_LAYER.into()))
            .unwrap();
        d.on_overlay_lost();
        assert!(!d.grab_held());
        assert!(!d.overlay.visible);
        assert!(!d.overlay_visible());
    }

    #[test]
    fn kanata_loss_releases_and_signals_coexit() {
        // Plan §8 test #10 (scaffold level): fake TCP drop mid-overlay.
        let mut d = daemon();
        d.on_layer_event(LayerEvent::LayerChanged(OVERLAY_LAYER.into()))
            .unwrap();
        let err = d.on_kanata_lost();
        assert!(matches!(err, DaemonError::KanataLost));
        assert!(!d.grab_held());
        assert!(!d.overlay.visible);
    }

    #[test]
    fn subgrid_geometry_nests_exactly() {
        let monitor = Monitor {
            x: 0,
            y: 0,
            w: 1920,
            h: 1080,
            scale: 1.0,
        };
        let model = build_subgrid_model(&default_grid(), &default_grid(), &monitor, 0).unwrap();
        assert_eq!(model.cells.len(), 8);
        assert_eq!(
            (
                model.cells[0].x,
                model.cells[0].y,
                model.cells[0].w,
                model.cells[0].h
            ),
            (0.0, 0.0, 120.0, 270.0)
        );
        assert_eq!(
            (
                model.cells[7].x,
                model.cells[7].y,
                model.cells[7].w,
                model.cells[7].h
            ),
            (360.0, 270.0, 120.0, 270.0)
        );
    }
}
