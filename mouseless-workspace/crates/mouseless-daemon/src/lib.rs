//! `mouseless-daemon`: thin orchestration over trait backends.
//!
//! The daemon owns NO backend logic (plan §1.2): it wires a [`KeyboardBackend`],
//! [`MouseBackend`], and [`OverlayBackend`] together behind a single-flight
//! overlay machine (one mutex, idempotent show/hide — plan §5).
//!
//! Kill switch (plan §7, audit W2): kanata TCP EOF/reset means kanata died —
//! release any held [`CellGrab`]/buttons, best-effort overlay-hide, then
//! co-exit with a failure code so the supervisor restarts the stack in order.
//!
//! Degraded modes (plan §6): no overlay client or no monitor report means NO
//! grab and a base-layer restore — typing is never affected by a broken
//! overlay. No uinput device means clicks are skipped but the overlay still
//! tracks.
//!
//! Phase 2 adds two sessions behind the same grab lifecycle:
//! - overlay actions (multi-click, drag, nudges, monitor switch, cursor hide);
//! - custom free mode (`lyr-free`) with an 8 ms fixed-step physics tick.

use mouseless_core::{
    cursor_hide_target, select_monitor, subcell_bounds, Corner, GridConfig, Monitor,
    MonitorDirection, Point, PrimaryGrid, Rect,
};
use mouseless_ipc::{CursorPos, GridCell, GridModel, MonitorLayout};
use mouseless_kbd::{
    CellGrab, CellKey, CellKeySource, KbdError, KeyEvent, KeyboardBackend, LayerEvent, Modifiers,
};
use mouseless_mouse::{MouseBackend, MouseButton, MouseError};
use mouseless_overlay::{OverlayBackend, OverlayError, OverlayEvent};
use mouseless_state::{
    ActionIntent, Button, ClickCounter, DragState, FreeModeEngine, FreeModeKey, FreeModeParams,
    FreeModeSession, NudgeDirection, NudgeState, OverlayMachine, Phase, StateError, WheelParams,
};
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};
use thiserror::Error;

pub use mouseless_kbd::{BASE_LAYER, FREE_MODE_LAYER, KANATA_FREE_MODE_LAYER, OVERLAY_LAYER};

/// Cursor refresh budget before a click (SPEC-daemon step 6).
pub const CURSOR_QUERY_TIMEOUT: Duration = Duration::from_millis(100);
/// Shift tap window for multi-monitor switching.
pub const MONITOR_SWITCH_TAP_MS: u64 = 200;
/// Fixed physics tick for custom free mode (SPEC-daemon OQ-3).
pub const FREE_MODE_TICK: Duration = Duration::from_millis(8);
/// Largest single REL step used by closed-loop [`Daemon::move_to`].
///
/// The compositor applies pointer acceleration and clamps large one-shot
/// deltas at monitor edges, so a single huge `REL_X` can stop at the boundary
/// (the cursor-trapped-at-x=1920 regression). Bounded steps keep every event
/// in the range that crosses edges, and the next iteration re-queries the
/// cursor before moving again.
pub const MAX_MOVE_STEP: i32 = 64;
/// Divisor applied to the remaining error before emitting a REL step.
///
/// The compositor's pointer acceleration has an observed gain of up to ~2×
/// (a 64 px request moves ~128 px). Sending the full error therefore
/// overshoots, reverses, and oscillates around the target (the live post-click
/// "dance"). Half-error steps are stable for gains up to ~2 and still land in
/// one step when the gain is 2; when the gain is ~1 the error halves per step.
pub const MOVE_DAMPING_DIVISOR: i32 = 2;
/// Smallest REL step emitted for a remaining error above tolerance.
///
/// A sub-unity acceleration gain can truncate a 1 px request to zero, which
/// would look like a stalled cursor 3 px from the target. Two pixels survive
/// both sub-unity gains and a 2× gain (which lands within tolerance).
pub const MOVE_MIN_STEP: i32 = 2;
/// Acceptable final cursor error after a move, in physical pixels.
pub const MOVE_TOLERANCE_PX: i32 = 2;
/// Maximum closed-loop iterations: 256 × 64 px covers a 16K-pixel desktop.
pub const MAX_MOVE_ITERATIONS: u32 = 256;
/// Consecutive no-progress queries tolerated before declaring a stalled move.
///
/// One stale report is normal (the compositor may answer before applying the
/// last REL event); eight in a row means the cursor is not following.
pub const MAX_STALLED_STEPS: u32 = 8;
/// Maximum time to wait for a stale cursor report to change after a step.
///
/// A `QueryCursor` sent immediately after `move_rel` can be answered before
/// the compositor applies the motion. The first reply is normally fresh; if it
/// still equals the pre-move position, wait for a change so the next delta is
/// not computed from the stale value.
pub const MOVE_SETTLE_TIMEOUT: Duration = Duration::from_millis(20);
/// Poll interval while waiting for a fresh cursor report.
pub const MOVE_SETTLE_POLL: Duration = Duration::from_millis(2);

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
    /// Cursor never reached the target within the closed-loop budget.
    #[error("cursor stalled at {cursor:?} moving to {target:?}")]
    MoveStalled {
        /// Requested absolute target in physical pixels.
        target: (i32, i32),
        /// Last cursor position reported by the overlay.
        cursor: (i32, i32),
    },
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

/// Where `Space` clicks before a cell is selected (SPEC-state success criteria).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum InitialActionLocation {
    /// Center of the active monitor (default).
    #[default]
    ScreenCenter,
    /// Current system cursor.
    SystemCursor,
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
    /// Primary two-level grid: 10 columns × 30 rows by default, each cell
    /// labelled with its column key followed by its row key (e.g. `AQ`).
    pub grid: PrimaryGrid,
    /// Subgrid inside a selected cell: 10×3, keyboard-row layout by default.
    pub subgrid: GridConfig,
    /// Custom physics (`lyr-free`) or kanata's `lyr-mouse` fallback.
    pub free_mode_engine: FreeModeEngine,
    /// Free-mode movement/auto-off parameters.
    pub free_mode: FreeModeParams,
    /// Free-mode wheel parameters.
    pub wheel: WheelParams,
    /// Multi-click cadence window; 0 disables coalescing.
    pub multi_click_threshold_ms: u64,
    /// Nudges per cell edge (minimum 1).
    pub nudges_per_cell: u32,
    /// Hold a subgrid key to nudge before releasing to click.
    pub hold_subgrid_for_nudge: bool,
    /// Keep the overlay visible after a click until Esc/layer exit.
    pub continuous_mode: bool,
    /// Move the cursor to a corner after each click.
    pub hide_cursor_on_click: bool,
    /// Corner used by `hide_cursor_on_click`.
    pub cursor_hide_corner: Corner,
    /// Shift held on the final key selects the right button.
    pub hold_for_right_button: bool,
    /// Alt held on the final key begins/continues a drag.
    pub hold_for_drag: bool,
    /// Where `Space` clicks before a cell selection.
    pub initial_action_location: InitialActionLocation,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            keyboard_backend: "kanata".into(),
            overlay_backend: "gnome".into(),
            kanata_addr: "127.0.0.1:7070".into(),
            socket_path: mouseless_ipc::SOCKET_PATH.into(),
            grid: default_grid(),
            subgrid: default_subgrid(),
            free_mode_engine: FreeModeEngine::Custom,
            free_mode: FreeModeParams::default(),
            wheel: WheelParams::default(),
            multi_click_threshold_ms: 500,
            nudges_per_cell: 4,
            hold_subgrid_for_nudge: false,
            continuous_mode: false,
            hide_cursor_on_click: false,
            cursor_hide_corner: Corner::TopRight,
            hold_for_right_button: true,
            hold_for_drag: true,
            initial_action_location: InitialActionLocation::ScreenCenter,
        }
    }
}

/// Level-1 (column) keys, left to right: one home-row key per column.
pub const COLUMN_KEYS: &str = "asdfghjkl;";
/// Level-2 (row) keys, top to bottom: the full QWERTY letter block in order.
pub const ROW_KEYS: &str = "qwertyuiopasdfghjkl;zxcvbnm,./";
/// Subgrid keys, row-major: the same QWERTY block laid out 10×3.
pub const SUBGRID_KEYS: &str = "qwertyuiopasdfghjkl;zxcvbnm,./";

/// Default primary grid: 10 columns (`asdfghjkl;`) × 30 rows (`qwerty…/`).
///
/// These sizes mirror the mouseless.click v1.0 "position based columns"
/// preset: a column key picks the horizontal strip, a row key picks the cell
/// inside it, and the two characters form the cell label.
pub fn default_grid() -> PrimaryGrid {
    PrimaryGrid {
        cols: COLUMN_KEYS.chars().count() as u32,
        rows: ROW_KEYS.chars().count() as u32,
        column_keys: COLUMN_KEYS.chars().collect(),
        row_keys: ROW_KEYS.chars().collect(),
    }
}

/// Default subgrid: 10 × 3, one key per sub-cell in keyboard-row order.
pub fn default_subgrid() -> GridConfig {
    GridConfig {
        cols: 10,
        rows: 3,
        keys: SUBGRID_KEYS.chars().collect(),
    }
}

/// Result of handling one grabbed key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionStep {
    /// Stay in the session (e.g. a cell was highlighted or a drag moved).
    Continue,
    /// A click/drop was executed. The session may still be alive for
    /// multi-click/continuous mode; callers check [`Daemon::session_active`].
    Clicked,
    /// Escape/layer exit; the caller must end the session.
    Exit,
}

#[derive(Debug, Clone, Copy)]
struct ShiftPress {
    at_ms: u64,
    used: bool,
}

#[derive(Debug, Clone, Copy)]
struct HeldSubcell {
    key: CellKey,
    button: Button,
    rect: Rect,
    target: Point,
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
    /// Pending level-1 column while a two-key cell selection is in progress.
    pending_column: Option<u32>,
    active_monitor: Option<Monitor>,
    free_mode: Option<FreeModeSession>,
    click_counter: ClickCounter,
    drag: DragState,
    nudge: NudgeState,
    last_click_target: Option<(i32, i32)>,
    pending_click_deadline_ms: Option<u64>,
    shift_press: Option<ShiftPress>,
    held_subcell: Option<HeldSubcell>,
    clock_origin: Instant,
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
        debug_assert!(config.grid.validate().is_ok(), "invalid primary grid");
        debug_assert!(config.subgrid.validate().is_ok(), "invalid subgrid");
        let nudge = NudgeState::new(config.nudges_per_cell);
        Self {
            kbd,
            mouse,
            overlay,
            config,
            machine: OverlayMachine::new(),
            grab: None,
            grab_factory,
            selected_cell: None,
            pending_column: None,
            active_monitor: None,
            free_mode: None,
            click_counter: ClickCounter::default(),
            drag: DragState::default(),
            nudge,
            last_click_target: None,
            pending_click_deadline_ms: None,
            shift_press: None,
            held_subcell: None,
            clock_origin: Instant::now(),
        }
    }

    /// Handle one layer event.
    ///
    /// `lyr-mouseless` shows the overlay; `lyr-free` starts custom free mode;
    /// anything else tears down without ever leaving a grab held. Duplicate
    /// events are safe: both starts are idempotent (plan §5).
    pub fn on_layer_event(&mut self, ev: LayerEvent) -> Result<(), DaemonError> {
        match ev {
            LayerEvent::LayerChanged(name) if name == OVERLAY_LAYER => self.start_overlay(),
            LayerEvent::LayerChanged(name) if name == FREE_MODE_LAYER => self.start_free_mode(),
            LayerEvent::LayerChanged(name) if name == KANATA_FREE_MODE_LAYER => {
                // Kanata's own fallback layer: never grab. Do not force base —
                // kanata already chose this layer deliberately.
                self.release_free_buttons();
                self.teardown(false);
                Ok(())
            }
            LayerEvent::LayerChanged(_) => {
                self.end_overlay();
                Ok(())
            }
        }
    }

    /// Start an overlay session: verify the extension, compute the grid, grab
    /// cell keys, and render. Any failure degrades to base (typing unaffected).
    pub fn start_overlay(&mut self) -> Result<(), DaemonError> {
        if self.overlay_visible() {
            return Ok(()); // duplicate LayerChange: keep the existing grab
        }
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
        self.release_free_buttons();
        self.teardown(false);
        // Grab before showing: if the grab fails, the user sees no overlay
        // and typing continues on the base layer (plan §5 rule 5).
        match (self.grab_factory)() {
            Ok(grab) => self.grab = Some(grab),
            Err(KbdError::Busy) => return Err(DaemonError::Kbd(KbdError::Busy)),
            Err(e) => {
                self.degrade_base();
                return Err(DaemonError::Kbd(e));
            }
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
        self.pending_column = None;
        self.active_monitor = Some(monitor);
        self.pending_click_deadline_ms = None;
        self.click_counter.reset();
        self.last_click_target = None;
        self.shift_press = None;
        self.held_subcell = None;
        self.drag = DragState::default();
        Ok(())
    }

    /// Start custom free mode (`lyr-free`): grab keys, do not show the overlay.
    ///
    /// Free mode is relative, so it works without the extension; a reported
    /// layout is used only for cursor hiding/monitor switching.
    pub fn start_free_mode(&mut self) -> Result<(), DaemonError> {
        if self.free_mode.is_some() {
            return Ok(());
        }
        if self.config.free_mode_engine == FreeModeEngine::Kanata {
            // Kanata's `lyr-mouse` is authoritative; restore base so a stray
            // `lyr-free` cannot leak letters to the focused app.
            self.end_overlay();
            return Ok(());
        }
        self.release_free_buttons();
        self.teardown(false);
        let monitor = self
            .overlay
            .last_layout()
            .and_then(|layout| pick_monitor(&layout, self.overlay.last_cursor()));
        match (self.grab_factory)() {
            Ok(grab) => self.grab = Some(grab),
            Err(KbdError::Busy) => return Err(DaemonError::Kbd(KbdError::Busy)),
            Err(e) => {
                self.degrade_base();
                return Err(DaemonError::Kbd(e));
            }
        }
        let mut session = FreeModeSession::new(self.config.free_mode, self.config.wheel);
        session.motion.mark_activity(self.now_ms());
        self.free_mode = Some(session);
        self.active_monitor = monitor;
        self.selected_cell = None;
        self.pending_column = None;
        self.pending_click_deadline_ms = None;
        self.click_counter.reset();
        self.last_click_target = None;
        self.shift_press = None;
        self.held_subcell = None;
        self.drag = DragState::default();
        Ok(())
    }

    /// End the session on every exit path: release the grab (Drop), hide the
    /// overlay, and restore the base layer idempotently.
    pub fn end_overlay(&mut self) {
        self.release_free_buttons();
        self.teardown(true);
    }

    /// End free mode: release held buttons, drop the grab, restore base.
    pub fn end_free_mode(&mut self) {
        self.release_free_buttons();
        self.teardown(true);
    }

    fn teardown(&mut self, restore_base: bool) {
        self.grab = None; // RAII: drain + ungrab + close (audits G6/G7)
        self.free_mode = None;
        self.selected_cell = None;
        self.pending_column = None;
        self.active_monitor = None;
        // A layer change can end the session without an Esc key event, so the
        // drag button must be released here rather than only in cancel_overlay.
        if let Some(button) = self.drag.button() {
            let _ = self.mouse.button_up(map_button(button));
        }
        self.drag = DragState::default();
        self.held_subcell = None;
        self.pending_click_deadline_ms = None;
        self.shift_press = None;
        self.click_counter.reset();
        self.last_click_target = None;
        let _ = self.machine.hide();
        let _ = self.overlay.hide();
        if restore_base {
            let _ = self.kbd.set_layer(BASE_LAYER);
        }
    }

    /// Release any free-mode mouse buttons still held (never leave one down).
    fn release_free_buttons(&mut self) {
        let held = self
            .free_mode
            .as_ref()
            .map(FreeModeSession::held_buttons)
            .unwrap_or_default();
        for key in held {
            let _ = self.mouse.button_up(free_button(key));
        }
    }

    /// Overlay client disconnected (Shell restart/crash): never keep an
    /// overlay grab. Free mode is relative and may continue without it.
    pub fn on_overlay_lost(&mut self) {
        if self.free_mode.is_some() {
            self.selected_cell = None;
            self.pending_column = None;
            self.held_subcell = None;
            let _ = self.overlay.hide();
            return;
        }
        self.end_overlay();
    }

    /// Poll the overlay backend for reports/cursor replies (non-blocking).
    pub fn poll_overlay_events(&mut self) -> Vec<OverlayEvent> {
        self.overlay.poll_events()
    }

    /// Poll the grabbed device for one cell key press (legacy press-only).
    pub fn poll_cell_key(&mut self) -> Result<Option<CellKey>, DaemonError> {
        match self.grab.as_mut() {
            Some(grab) => Ok(grab.next_key_press()?),
            None => Ok(None),
        }
    }

    /// Poll the grabbed device for one press/release event with modifiers.
    pub fn poll_key_event(&mut self) -> Result<Option<KeyEvent>, DaemonError> {
        match self.grab.as_mut() {
            Some(grab) => Ok(grab.next_key_event()?),
            None => Ok(None),
        }
    }

    /// Handle one cell key press with default modifiers (legacy/test helper).
    pub fn handle_cell_key(&mut self, key: CellKey) -> Result<SessionStep, DaemonError> {
        self.handle_key_event(KeyEvent {
            key,
            pressed: true,
            modifiers: Modifiers::default(),
        })
    }

    /// Handle one grabbed key event, using the daemon's monotonic clock.
    pub fn handle_key_event(&mut self, ev: KeyEvent) -> Result<SessionStep, DaemonError> {
        let now_ms = self.now_ms();
        self.handle_key_event_at(ev, now_ms)
    }

    /// Handle one grabbed key event at an explicit time (deterministic tests).
    pub fn handle_key_event_at(
        &mut self,
        ev: KeyEvent,
        now_ms: u64,
    ) -> Result<SessionStep, DaemonError> {
        if self.free_mode.is_some() {
            return self.handle_free_key_event(ev, now_ms);
        }
        if self.overlay_visible() {
            return self.handle_overlay_key_event(ev, now_ms);
        }
        Ok(SessionStep::Continue)
    }

    fn handle_overlay_key_event(
        &mut self,
        ev: KeyEvent,
        now_ms: u64,
    ) -> Result<SessionStep, DaemonError> {
        if !ev.pressed {
            return self.handle_overlay_release(ev, now_ms);
        }
        match ev.key {
            CellKey::Escape => {
                self.cancel_overlay();
                Ok(SessionStep::Exit)
            }
            CellKey::ShiftLeft | CellKey::ShiftRight => {
                self.shift_press = Some(ShiftPress {
                    at_ms: now_ms,
                    used: false,
                });
                Ok(SessionStep::Continue)
            }
            CellKey::ArrowUp => self.nudge(NudgeDirection::Up),
            CellKey::ArrowDown => self.nudge(NudgeDirection::Down),
            CellKey::ArrowLeft => self.nudge(NudgeDirection::Left),
            CellKey::ArrowRight => self.nudge(NudgeDirection::Right),
            CellKey::Space => self.handle_space(ev.modifiers, now_ms),
            CellKey::Char(c) => self.handle_char(c, ev.modifiers, now_ms),
        }
    }

    fn handle_overlay_release(
        &mut self,
        ev: KeyEvent,
        now_ms: u64,
    ) -> Result<SessionStep, DaemonError> {
        if matches!(ev.key, CellKey::ShiftLeft | CellKey::ShiftRight) {
            if let Some(press) = self.shift_press.take() {
                let duration = now_ms.saturating_sub(press.at_ms);
                if !press.used && duration <= MONITOR_SWITCH_TAP_MS {
                    let direction = if ev.key == CellKey::ShiftLeft {
                        MonitorDirection::Left
                    } else {
                        MonitorDirection::Right
                    };
                    self.switch_monitor(direction)?;
                }
            }
            return Ok(SessionStep::Continue);
        }
        if self.held_subcell.as_ref().is_some_and(|h| h.key == ev.key) {
            if let Some(held) = self.held_subcell.take() {
                // Nudge mode holds the subcell key; Alt on release must still
                // begin/continue a drag instead of always clicking.
                if self.drag.is_dragging() || (ev.modifiers.alt && self.config.hold_for_drag) {
                    return self.final_action(held.target, ev.modifiers, now_ms);
                }
                self.click_at(held.button, held.target, now_ms, ev.modifiers)?;
                return Ok(SessionStep::Clicked);
            }
        }
        Ok(SessionStep::Continue)
    }

    fn handle_char(
        &mut self,
        c: char,
        modifiers: Modifiers,
        now_ms: u64,
    ) -> Result<SessionStep, DaemonError> {
        // A different subgrid key while a nudge is held finishes the pending
        // click first, then processes the new key (if the session survived).
        if self.held_subcell.is_some() {
            if let Some(held) = self.held_subcell.take() {
                self.click_at(held.button, held.target, now_ms, modifiers)?;
                if !self.overlay_visible() {
                    return Ok(SessionStep::Clicked);
                }
            }
        }
        let Some(parent) = self.selected_cell else {
            // Two-key cell selection: the level-1 key picks the column, the
            // level-2 key picks the row. Until both are known the overlay
            // stays on the primary grid. Every level-1 key is also a level-2
            // key, so once a column is pending the next key always resolves
            // as a row (Esc abandons the pending prefix).
            match self.pending_column {
                None => {
                    if let Some(column) = self.config.grid.column_of(c) {
                        self.pending_column = Some(column);
                    }
                }
                Some(column) => {
                    if let Some(row) = self.config.grid.row_of(c) {
                        let index = self.config.grid.index(column, row);
                        let monitor = self.active_monitor.ok_or(DaemonError::NoLayout)?;
                        let model = build_subgrid_model(
                            &self.config.grid,
                            &self.config.subgrid,
                            &monitor,
                            index,
                        )?;
                        self.pending_column = None;
                        self.selected_cell = Some(index);
                        self.machine.open_subgrid()?;
                        self.overlay.show_subgrid(index, &model)?;
                        self.click_counter.reset();
                        self.last_click_target = None;
                    }
                }
            }
            return Ok(SessionStep::Continue);
        };
        let Some(sub) = index_of(&self.config.subgrid.keys, c) else {
            return Ok(SessionStep::Continue);
        };
        let monitor = self.active_monitor.ok_or(DaemonError::NoLayout)?;
        let parent_rect = self
            .config
            .grid
            .bounds(&monitor, self.config.grid.cell_id(parent));
        let sub_rect = subcell_bounds(
            &parent_rect,
            &self.config.subgrid,
            cell_id(&self.config.subgrid, sub),
        );
        let button = self.button_for(modifiers);
        if self.config.hold_subgrid_for_nudge {
            let target = sub_rect.center();
            self.held_subcell = Some(HeldSubcell {
                key: CellKey::Char(c),
                button,
                rect: sub_rect,
                target,
            });
            self.execute_action(ActionIntent::Move { to: target })?;
            Ok(SessionStep::Continue)
        } else {
            self.final_action(sub_rect.center(), modifiers, now_ms)
        }
    }

    /// Execute the final key of a click action: click, begin/continue a drag,
    /// or drop an active drag. This is the single policy point for modifier
    /// clicks and drag/drop (SPEC-state Phase-2 semantics).
    fn final_action(
        &mut self,
        target: Point,
        modifiers: Modifiers,
        now_ms: u64,
    ) -> Result<SessionStep, DaemonError> {
        let button = self.button_for(modifiers);
        if self.drag.is_dragging() {
            self.execute_action(ActionIntent::DragTo { to: target })?;
            if modifiers.alt && self.config.hold_for_drag {
                Ok(SessionStep::Continue)
            } else {
                self.execute_action(ActionIntent::DragRelease)?;
                self.end_overlay();
                Ok(SessionStep::Clicked)
            }
        } else if modifiers.alt && self.config.hold_for_drag {
            self.execute_action(ActionIntent::Move { to: target })?;
            self.execute_action(ActionIntent::DragBegin { button })?;
            self.drag.begin(button);
            self.mark_shift_used(modifiers);
            self.pending_click_deadline_ms = None;
            Ok(SessionStep::Continue)
        } else {
            self.click_at(button, target, now_ms, modifiers)?;
            Ok(SessionStep::Clicked)
        }
    }

    fn handle_space(
        &mut self,
        modifiers: Modifiers,
        now_ms: u64,
    ) -> Result<SessionStep, DaemonError> {
        let target = match self.held_subcell.as_ref() {
            Some(held) => held.target,
            None => match self.selected_cell {
                Some(parent) => {
                    let monitor = self.active_monitor.ok_or(DaemonError::NoLayout)?;
                    self.config
                        .grid
                        .center(&monitor, self.config.grid.cell_id(parent))
                }
                None => self.initial_action_target()?,
            },
        };
        // Space during a drag drops at the target; without this branch a
        // second left down/up would be emitted while the drag button is held.
        if self.drag.is_dragging() {
            self.held_subcell = None;
            return self.final_action(target, modifiers, now_ms);
        }
        let button = self.button_for(modifiers);
        self.held_subcell = None;
        self.click_at(button, target, now_ms, modifiers)?;
        Ok(SessionStep::Clicked)
    }

    fn initial_action_target(&mut self) -> Result<Point, DaemonError> {
        match self.config.initial_action_location {
            InitialActionLocation::ScreenCenter => {
                let monitor = self.active_monitor.ok_or(DaemonError::NoLayout)?;
                Ok(Point {
                    x: monitor.x as f64 + monitor.w as f64 / 2.0,
                    y: monitor.y as f64 + monitor.h as f64 / 2.0,
                })
            }
            InitialActionLocation::SystemCursor => {
                let cursor = self
                    .refresh_cursor(CURSOR_QUERY_TIMEOUT)
                    .ok_or(DaemonError::NoCursor)?;
                Ok(Point {
                    x: cursor.x as f64,
                    y: cursor.y as f64,
                })
            }
        }
    }

    fn button_for(&self, modifiers: Modifiers) -> Button {
        if modifiers.shift && self.config.hold_for_right_button {
            Button::Right
        } else {
            Button::Left
        }
    }

    fn nudge(&mut self, direction: NudgeDirection) -> Result<SessionStep, DaemonError> {
        let Some(held) = self.held_subcell.as_mut() else {
            return Ok(SessionStep::Continue);
        };
        let step = self.nudge.delta(direction, &held.rect);
        held.target.x += step.x;
        held.target.y += step.y;
        if let Some(monitor) = self.active_monitor {
            held.target = clamp_point(&monitor, held.target);
        }
        let target = held.target;
        self.execute_action(ActionIntent::Move { to: target })?;
        Ok(SessionStep::Continue)
    }

    fn cancel_overlay(&mut self) {
        if self.drag.is_dragging() {
            let _ = self.execute_action(ActionIntent::DragRelease);
        }
        self.held_subcell = None;
        self.end_overlay();
    }

    /// Click at an absolute target, updating the multi-click cadence and
    /// applying the post-click policy (continuous/pending/hide).
    fn click_at(
        &mut self,
        button: Button,
        target: Point,
        now_ms: u64,
        modifiers: Modifiers,
    ) -> Result<(), DaemonError> {
        let target_i = (target.x.round() as i32, target.y.round() as i32);
        if self.last_click_target != Some(target_i) {
            self.click_counter.reset();
            self.last_click_target = Some(target_i);
        }
        let count = self
            .click_counter
            .register(now_ms, self.config.multi_click_threshold_ms);
        self.mark_shift_used(modifiers);
        self.execute_action(ActionIntent::Move { to: target })?;
        self.execute_action(ActionIntent::Click { button, count })?;
        if self.config.hide_cursor_on_click {
            self.execute_action(ActionIntent::CursorHide {
                corner: self.config.cursor_hide_corner,
            })?;
        }
        self.after_click(now_ms);
        Ok(())
    }

    fn after_click(&mut self, now_ms: u64) {
        self.held_subcell = None;
        if self.config.multi_click_threshold_ms > 0 {
            self.pending_click_deadline_ms =
                Some(now_ms.saturating_add(self.config.multi_click_threshold_ms));
        } else if self.config.continuous_mode {
            self.reset_to_overlay();
        } else {
            self.end_overlay();
        }
    }

    /// Return to the level-1 overlay while keeping the grab/session alive.
    fn reset_to_overlay(&mut self) {
        self.selected_cell = None;
        self.pending_column = None;
        self.held_subcell = None;
        self.click_counter.reset();
        self.last_click_target = None;
        self.pending_click_deadline_ms = None;
        let _ = self.machine.reset_to_overlay();
        if let Some(monitor) = self.active_monitor {
            let model = build_grid_model(&self.config.grid, &monitor);
            let _ = self.overlay.show(&model);
        }
    }

    fn switch_monitor(&mut self, direction: MonitorDirection) -> Result<(), DaemonError> {
        let Some(layout) = self.overlay.last_layout() else {
            return Ok(());
        };
        let monitors: Vec<Monitor> = layout.monitors.iter().map(|m| m.monitor).collect();
        let current = self
            .active_monitor
            .and_then(|m| monitors.iter().position(|candidate| *candidate == m))
            .unwrap_or(0);
        let Some(next) = select_monitor(&monitors, current, direction) else {
            return Ok(());
        };
        let monitor = monitors[next];
        let model = build_grid_model(&self.config.grid, &monitor);
        self.active_monitor = Some(monitor);
        self.selected_cell = None;
        self.pending_column = None;
        self.held_subcell = None;
        self.click_counter.reset();
        self.last_click_target = None;
        self.machine.reset_to_overlay()?;
        self.overlay.show(&model)?;
        Ok(())
    }

    fn mark_shift_used(&mut self, modifiers: Modifiers) {
        if modifiers.shift {
            if let Some(press) = self.shift_press.as_mut() {
                press.used = true;
            }
        }
    }

    fn handle_free_key_event(
        &mut self,
        ev: KeyEvent,
        now_ms: u64,
    ) -> Result<SessionStep, DaemonError> {
        if ev.key == CellKey::Escape {
            if ev.pressed {
                self.end_free_mode();
                return Ok(SessionStep::Exit);
            }
            return Ok(SessionStep::Continue);
        }
        let Some(key) = free_mode_key(ev.key) else {
            return Ok(SessionStep::Continue);
        };
        let is_button = key.is_button();
        if let Some(session) = self.free_mode.as_mut() {
            if ev.pressed {
                session.press(key, now_ms);
            } else {
                session.release(key);
            }
        }
        if is_button {
            let button = free_button(key);
            if ev.pressed {
                self.mouse.button_down(button)?;
            } else {
                self.mouse.button_up(button)?;
            }
        }
        Ok(SessionStep::Continue)
    }

    /// Advance one fixed step: free-mode physics and multi-click expiry.
    pub fn tick(&mut self, now_ms: u64) -> Result<(), DaemonError> {
        if self.free_mode.is_some() {
            self.tick_free_mode(now_ms)?;
        }
        if let Some(deadline) = self.pending_click_deadline_ms {
            if now_ms >= deadline {
                self.pending_click_deadline_ms = None;
                if self.config.continuous_mode {
                    self.reset_to_overlay();
                } else {
                    self.end_overlay();
                }
            }
        }
        Ok(())
    }

    /// Advance one fixed step using the daemon's monotonic clock.
    ///
    /// Production loops call this on their tick cadence; tests call [`tick`]
    /// with an explicit time for determinism.
    pub fn tick_now(&mut self) -> Result<(), DaemonError> {
        let now_ms = self.now_ms();
        self.tick(now_ms)
    }

    fn tick_free_mode(&mut self, now_ms: u64) -> Result<(), DaemonError> {
        let due = self
            .free_mode
            .as_ref()
            .is_some_and(|session| session.auto_off_due(now_ms));
        if due {
            self.end_free_mode();
            return Ok(());
        }
        let frame = match self.free_mode.as_mut() {
            Some(session) => session.tick(now_ms),
            None => return Ok(()),
        };
        if frame.move_delta != (0, 0) {
            self.mouse
                .move_rel(frame.move_delta.0, frame.move_delta.1)?;
        }
        if frame.wheel_delta != (0, 0) {
            self.mouse
                .scroll(frame.wheel_delta.0, frame.wheel_delta.1)?;
        }
        Ok(())
    }

    /// Execute a backend-neutral state intent (state never calls backends).
    pub fn execute_action(&mut self, intent: ActionIntent) -> Result<(), DaemonError> {
        match intent {
            ActionIntent::Move { to } => self.move_to(to),
            ActionIntent::Click { button, count } => {
                println!("mouselessd: click button={button:?} count={count} (native cadence)");
                self.mouse.click(map_button(button))?;
                Ok(())
            }
            ActionIntent::DragBegin { button } => {
                self.mouse.button_down(map_button(button))?;
                Ok(())
            }
            ActionIntent::DragTo { to } => self.move_to(to),
            ActionIntent::DragRelease => {
                if let Some(button) = self.drag.button() {
                    self.mouse.button_up(map_button(button))?;
                    self.drag.end();
                }
                Ok(())
            }
            ActionIntent::Wheel { dx, dy } => {
                self.mouse.scroll(dx, dy)?;
                Ok(())
            }
            ActionIntent::CursorHide { corner } => {
                if let Some(monitor) = self.active_monitor {
                    let target = cursor_hide_target(&monitor, corner, 1.0);
                    self.move_to(target)?;
                }
                Ok(())
            }
        }
    }

    /// Move the cursor to an absolute physical-pixel target, closed-loop.
    ///
    /// One large `REL` delta is unreliable: the compositor's pointer
    /// acceleration makes it land short, and a delta that would cross a
    /// monitor edge is clamped at that edge (the live "cursor trapped on the
    /// external monitor" regression). Instead, emit bounded steps and
    /// re-query the cursor after each one until the target is within
    /// [`MOVE_TOLERANCE_PX`]. Failure is explicit ([`DaemonError::MoveStalled`])
    /// so callers never click at a guessed position.
    fn move_to(&mut self, to: Point) -> Result<(), DaemonError> {
        let debug = std::env::var_os("MOUSELESS_DEBUG_MOVE").is_some();
        let target = (to.x.round() as i32, to.y.round() as i32);
        let start = self
            .refresh_cursor(CURSOR_QUERY_TIMEOUT)
            .ok_or(DaemonError::NoCursor)?;
        let mut cursor = start;
        let mut steps = 0u32;
        let mut stalled = 0u32;
        loop {
            let (dx, dy) = mouseless_mouse::compute_delta(target, (cursor.x, cursor.y));
            if dx.saturating_abs() <= MOVE_TOLERANCE_PX && dy.saturating_abs() <= MOVE_TOLERANCE_PX
            {
                break;
            }
            if steps >= MAX_MOVE_ITERATIONS {
                return Err(DaemonError::MoveStalled {
                    target,
                    cursor: (cursor.x, cursor.y),
                });
            }
            let step = (damped_step(dx), damped_step(dy));
            let from = cursor;
            self.mouse.move_rel(step.0, step.1)?;
            steps += 1;
            let next = self
                .refresh_cursor_after_move(cursor)
                .ok_or(DaemonError::NoCursor)?;
            if debug {
                eprintln!(
                    "mouselessd: move step={steps} from=({}, {}) sent=({}, {}) reported=({}, {})",
                    from.x, from.y, step.0, step.1, next.x, next.y
                );
            }
            if next == cursor {
                stalled += 1;
                if stalled >= MAX_STALLED_STEPS {
                    return Err(DaemonError::MoveStalled {
                        target,
                        cursor: (next.x, next.y),
                    });
                }
            } else {
                stalled = 0;
            }
            cursor = next;
        }
        println!(
            "mouselessd: move target=({}, {}) cursor=({}, {}) delta=({}, {}) steps={} settled=({}, {})",
            target.0,
            target.1,
            start.x,
            start.y,
            target.0.saturating_sub(start.x),
            target.1.saturating_sub(start.y),
            steps,
            cursor.x,
            cursor.y
        );
        Ok(())
    }

    /// Click at an absolute physical-pixel target using the freshest cursor
    /// report available (QueryCursor budget ≤ 100 ms), then hide.
    ///
    /// Legacy low-level helper; Phase-2 flows use [`ActionIntent`] instead so
    /// multi-click/drag/continuous policies stay in one place.
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
    ///
    /// Movement uses the same closed-loop [`Self::move_to`] as every other
    /// path; `cursor` is only the pre-move snapshot used for logging.
    pub fn click_relative(
        &mut self,
        button: MouseButton,
        cursor: CursorPos,
        target: (i32, i32),
    ) -> Result<(), DaemonError> {
        println!(
            "mouselessd: click target=({}, {}) cursor=({}, {})",
            target.0, target.1, cursor.x, cursor.y
        );
        self.move_to(Point {
            x: target.0 as f64,
            y: target.1 as f64,
        })?;
        self.mouse.click(button)?;
        let _ = self.machine.click();
        self.end_overlay();
        Ok(())
    }

    /// Query the cursor after a REL step, guarding against a stale reply.
    ///
    /// The first reply is normally fresh (the extension answers after the
    /// compositor applied the step). If it still equals `previous`, the reply
    /// may predate the motion: poll until the position changes or
    /// [`MOVE_SETTLE_TIMEOUT`] expires. This is only a guard; the closed loop
    /// itself is stabilized by [`MOVE_DAMPING_DIVISOR`].
    fn refresh_cursor_after_move(&mut self, previous: CursorPos) -> Option<CursorPos> {
        let first = self.refresh_cursor(CURSOR_QUERY_TIMEOUT)?;
        if first != previous {
            return Some(first);
        }
        let deadline = Instant::now() + MOVE_SETTLE_TIMEOUT;
        loop {
            if Instant::now() >= deadline {
                return Some(first);
            }
            std::thread::sleep(MOVE_SETTLE_POLL);
            let next = self.refresh_cursor(CURSOR_QUERY_TIMEOUT)?;
            if next != previous {
                return Some(next);
            }
        }
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
        self.release_free_buttons();
        self.teardown(true);
    }

    /// Handle kanata TCP EOF/reset: release grab/buttons, best-effort hide,
    /// signal co-exit so the supervisor restarts the stack in order (plan §7).
    pub fn on_kanata_lost(&mut self) -> DaemonError {
        self.release_free_buttons();
        self.teardown(false);
        DaemonError::KanataLost
    }

    /// Whether the overlay is currently shown.
    pub fn overlay_visible(&self) -> bool {
        !matches!(self.machine.phase(), Phase::Normal)
    }

    /// Whether custom free mode is active.
    pub fn free_mode_active(&self) -> bool {
        self.free_mode.is_some()
    }

    /// Whether a drag is in progress.
    pub fn drag_active(&self) -> bool {
        self.drag.is_dragging()
    }

    /// Whether any daemon session still owns the grab/overlay.
    pub fn session_active(&self) -> bool {
        self.free_mode.is_some() || self.overlay_visible() || self.grab.is_some()
    }

    /// Whether a multi-click window is still open at `now_ms`.
    pub fn click_pending(&self, now_ms: u64) -> bool {
        self.pending_click_deadline_ms
            .is_some_and(|deadline| now_ms < deadline)
    }

    /// Finish a pending multi-click window immediately (tests/session exit).
    pub fn finish_pending_click(&mut self) {
        if self.pending_click_deadline_ms.take().is_some() {
            if self.config.continuous_mode {
                self.reset_to_overlay();
            } else {
                self.end_overlay();
            }
        }
    }

    /// Current logical multi-click count (0 before the first click).
    pub fn click_count(&self) -> u8 {
        self.click_counter.count()
    }

    /// Current monitor the overlay/free session targets.
    pub fn active_monitor(&self) -> Option<Monitor> {
        self.active_monitor
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

    /// Mutable access to the overlay backend (tests simulate cursor reports).
    pub fn overlay_mut(&mut self) -> &mut O {
        &mut self.overlay
    }

    /// Access the active config (diagnostics/tests).
    pub fn config(&self) -> &DaemonConfig {
        &self.config
    }

    fn now_ms(&self) -> u64 {
        self.clock_origin
            .elapsed()
            .as_millis()
            .min(u64::MAX as u128) as u64
    }
}

/// One damped REL component for [`Daemon::move_to`].
///
/// Halves the error (stable under the compositor's ~2× acceleration) and
/// enforces a two-pixel floor so a sub-unity gain cannot swallow the last few
/// pixels and masquerade as a stalled cursor.
fn damped_step(delta: i32) -> i32 {
    let half = delta / MOVE_DAMPING_DIVISOR;
    let step = if delta == 0 {
        0
    } else if half.saturating_abs() < MOVE_MIN_STEP {
        delta.signum() * MOVE_MIN_STEP
    } else {
        half
    };
    step.clamp(-MAX_MOVE_STEP, MAX_MOVE_STEP)
}

fn map_button(button: Button) -> MouseButton {
    match button {
        Button::Left => MouseButton::Left,
        Button::Right => MouseButton::Right,
        Button::Middle => MouseButton::Middle,
        Button::Back => MouseButton::Back,
        Button::Forward => MouseButton::Forward,
    }
}

fn free_button(key: FreeModeKey) -> MouseButton {
    match key {
        FreeModeKey::LeftButton => MouseButton::Left,
        FreeModeKey::RightButton => MouseButton::Right,
        FreeModeKey::MiddleButton => MouseButton::Middle,
        FreeModeKey::BackButton => MouseButton::Back,
        FreeModeKey::ForwardButton => MouseButton::Forward,
        // Non-button keys never reach this helper.
        _ => MouseButton::Left,
    }
}

fn free_mode_key(key: CellKey) -> Option<FreeModeKey> {
    Some(match key {
        CellKey::Char('i') => FreeModeKey::MoveUp,
        CellKey::Char('k') => FreeModeKey::MoveDown,
        CellKey::Char('j') => FreeModeKey::MoveLeft,
        CellKey::Char('l') => FreeModeKey::MoveRight,
        CellKey::Char('s') => FreeModeKey::Boost1,
        CellKey::Char('d') => FreeModeKey::Boost2,
        CellKey::Char('f') => FreeModeKey::Boost3,
        CellKey::Char('a') => FreeModeKey::Decel,
        CellKey::Char('m') => FreeModeKey::WheelUp,
        CellKey::Char(',') => FreeModeKey::WheelDown,
        CellKey::Char('.') => FreeModeKey::WheelLeft,
        CellKey::Char('/') => FreeModeKey::WheelRight,
        CellKey::Space => FreeModeKey::LeftButton,
        CellKey::Char('r') => FreeModeKey::RightButton,
        CellKey::Char('e') => FreeModeKey::MiddleButton,
        CellKey::Char('q') => FreeModeKey::BackButton,
        CellKey::Char('w') => FreeModeKey::ForwardButton,
        CellKey::Escape => FreeModeKey::Exit,
        _ => return None,
    })
}

fn clamp_point(monitor: &Monitor, point: Point) -> Point {
    let max_x = (monitor.x + monitor.w as i32).saturating_sub(1) as f64;
    let max_y = (monitor.y + monitor.h as i32).saturating_sub(1) as f64;
    Point {
        x: point.x.clamp(monitor.x as f64, max_x),
        y: point.y.clamp(monitor.y as f64, max_y),
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

/// Build the primary two-level wire model from pure geometry.
///
/// Every cell carries the concatenated column+row label (e.g. `AQ`); the
/// extension only draws what is sent (never render/derive in JS).
pub fn build_grid_model(config: &PrimaryGrid, monitor: &Monitor) -> GridModel {
    let cells = (0..config.cell_count())
        .map(|i| {
            let cell = config.cell_id(i);
            GridCell::new(i, config.label(cell), config.bounds(monitor, cell))
        })
        .collect();
    GridModel {
        monitor: *monitor,
        cells,
        highlight: None,
    }
}

/// Build a subgrid model nested inside primary cell `parent`.
pub fn build_subgrid_model(
    config: &PrimaryGrid,
    sub: &GridConfig,
    monitor: &Monitor,
    parent: u32,
) -> Result<GridModel, DaemonError> {
    if parent >= config.cell_count() {
        return Err(DaemonError::InvalidCell(parent));
    }
    let parent_rect = config.bounds(monitor, config.cell_id(parent));
    let cells = sub
        .keys
        .iter()
        .enumerate()
        .map(|(i, label)| {
            GridCell::new(
                i as u32,
                label.to_ascii_uppercase().to_string(),
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
    use mouseless_kbd::impls::{FakeCellGrab, FakeKeyboard};
    use mouseless_mouse::impls::FakeMouse;
    use mouseless_overlay::impls::{FakeOverlay, SharedCursor, SharedCursorReplies};

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

    fn layout_dual() -> MonitorLayout {
        MonitorLayout {
            monitors: vec![
                MonitorInfo {
                    monitor: Monitor {
                        x: 0,
                        y: 0,
                        w: 1920,
                        h: 1080,
                        scale: 1.0,
                    },
                    primary: true,
                },
                MonitorInfo {
                    monitor: Monitor {
                        x: 1920,
                        y: 0,
                        w: 1080,
                        h: 1920,
                        scale: 1.0,
                    },
                    primary: false,
                },
            ],
        }
    }

    /// Live dual-monitor layout: external HDMI primary on the right,
    /// built-in eDP secondary on the left (the regression setup).
    fn layout_live_dual() -> MonitorLayout {
        MonitorLayout {
            monitors: vec![
                MonitorInfo {
                    monitor: Monitor {
                        x: 1920,
                        y: 0,
                        w: 1920,
                        h: 1080,
                        scale: 1.0,
                    },
                    primary: true,
                },
                MonitorInfo {
                    monitor: Monitor {
                        x: 0,
                        y: 0,
                        w: 1920,
                        h: 1080,
                        scale: 1.0,
                    },
                    primary: false,
                },
            ],
        }
    }

    fn fake_grab() -> Result<FakeCellGrab, KbdError> {
        Ok(FakeCellGrab::new())
    }

    /// Mouse fake that moves the shared cursor, modeling the compositor loop.
    ///
    /// The live compositor applies pointer acceleration and clamps large
    /// one-shot REL deltas at monitor edges. Both behaviors are configurable
    /// so regression tests can reproduce them deterministically.
    struct CursorMouse {
        cursor: SharedCursor,
        /// Maximum movement applied by one `move_rel` event.
        max_step: i32,
        /// Acceleration factor as `numerator / denominator`.
        scale: (i32, i32),
        /// Monitor boundary that a one-shot delta larger than `max_step`
        /// cannot cross (the live "cursor trapped at x=1920" bug).
        barrier_x: Option<i32>,
        /// When true, `move_rel` is recorded but never changes the cursor
        /// (stale/broken compositor report).
        stuck: bool,
        /// When set, every effective `move_rel` pushes the pre-move position
        /// as a one-shot reply, modeling the extension answering `QueryCursor`
        /// before the compositor applied the motion.
        stale_replies: Option<SharedCursorReplies>,
        moves: Vec<(i32, i32)>,
        clicks: Vec<MouseButton>,
        /// Cursor position at each `click`, in order.
        click_positions: Vec<(i32, i32)>,
        button_downs: Vec<MouseButton>,
        button_ups: Vec<MouseButton>,
        scrolls: Vec<(i32, i32)>,
    }

    impl CursorMouse {
        fn new(cursor: SharedCursor) -> Self {
            Self {
                cursor,
                max_step: i32::MAX,
                scale: (1, 1),
                barrier_x: None,
                stuck: false,
                stale_replies: None,
                moves: Vec::new(),
                clicks: Vec::new(),
                click_positions: Vec::new(),
                button_downs: Vec::new(),
                button_ups: Vec::new(),
                scrolls: Vec::new(),
            }
        }

        fn position(&self) -> (i32, i32) {
            self.cursor.borrow().map(|c| (c.x, c.y)).unwrap_or_default()
        }

        /// Every REL event must stay inside the daemon's bounded-step budget.
        fn assert_bounded(&self) {
            assert!(
                self.moves
                    .iter()
                    .all(|(dx, dy)| dx.saturating_abs() <= MAX_MOVE_STEP
                        && dy.saturating_abs() <= MAX_MOVE_STEP),
                "unbounded REL step: {:?}",
                self.moves
            );
        }
    }

    impl MouseBackend for CursorMouse {
        type Error = MouseError;

        fn move_rel(&mut self, dx: i32, dy: i32) -> Result<(), Self::Error> {
            self.moves.push((dx, dy));
            if self.stuck {
                return Ok(());
            }
            let mut guard = self.cursor.borrow_mut();
            let Some(pos) = guard.as_mut() else {
                return Ok(());
            };
            let before = (pos.x, pos.y);
            let mut moved = false;
            if let Some(barrier) = self.barrier_x {
                // One-shot large delta: stop at the monitor edge (live bug).
                if dx < -self.max_step && pos.x >= barrier && pos.x.saturating_add(dx) < barrier {
                    pos.x = barrier;
                    moved = true;
                } else if dx > self.max_step
                    && pos.x < barrier
                    && pos.x.saturating_add(dx) >= barrier
                {
                    pos.x = barrier - 1;
                    moved = true;
                }
            }
            if !moved {
                let sx = dx.clamp(-self.max_step, self.max_step) * self.scale.0 / self.scale.1;
                let sy = dy.clamp(-self.max_step, self.max_step) * self.scale.0 / self.scale.1;
                pos.x = pos.x.saturating_add(sx);
                pos.y = pos.y.saturating_add(sy);
            }
            let after = (pos.x, pos.y);
            drop(guard);
            if after != before {
                if let Some(replies) = &self.stale_replies {
                    replies.borrow_mut().push_back(CursorPos {
                        x: before.0,
                        y: before.1,
                    });
                }
            }
            Ok(())
        }

        fn click(&mut self, button: MouseButton) -> Result<(), Self::Error> {
            self.clicks.push(button);
            let pos = self.position();
            self.click_positions.push(pos);
            Ok(())
        }

        fn button_down(&mut self, button: MouseButton) -> Result<(), Self::Error> {
            self.button_downs.push(button);
            Ok(())
        }

        fn button_up(&mut self, button: MouseButton) -> Result<(), Self::Error> {
            self.button_ups.push(button);
            Ok(())
        }

        fn scroll(&mut self, dx_lines: i32, dy_lines: i32) -> Result<(), Self::Error> {
            self.scrolls.push((dx_lines, dy_lines));
            Ok(())
        }
    }

    type TestDaemon = Daemon<FakeKeyboard, CursorMouse, FakeOverlay, FakeCellGrab>;

    fn daemon_with(config: DaemonConfig, layout: MonitorLayout) -> TestDaemon {
        daemon_with_mouse(config, layout, CursorPos { x: 320, y: 270 }, |overlay| {
            CursorMouse::new(overlay.cursor_handle())
        })
    }

    fn daemon_with_mouse(
        config: DaemonConfig,
        layout: MonitorLayout,
        cursor: CursorPos,
        make_mouse: impl FnOnce(&FakeOverlay) -> CursorMouse,
    ) -> TestDaemon {
        let mut overlay = FakeOverlay::default();
        overlay.set_layout(layout);
        overlay.set_cursor(cursor);
        let mouse = make_mouse(&overlay);
        Daemon::with_grab(
            FakeKeyboard::new(BASE_LAYER),
            mouse,
            overlay,
            config,
            fake_grab,
        )
    }

    fn daemon() -> TestDaemon {
        daemon_with(
            DaemonConfig {
                multi_click_threshold_ms: 0,
                ..Default::default()
            },
            layout_1080p(),
        )
    }

    fn assert_near(actual: (i32, i32), expected: (i32, i32)) {
        assert!(
            (actual.0 - expected.0).abs() <= MOVE_TOLERANCE_PX
                && (actual.1 - expected.1).abs() <= MOVE_TOLERANCE_PX,
            "cursor {actual:?} is not within {MOVE_TOLERANCE_PX}px of {expected:?}"
        );
    }

    fn key(c: char) -> KeyEvent {
        KeyEvent {
            key: CellKey::Char(c),
            pressed: true,
            modifiers: Modifiers::default(),
        }
    }

    fn key_mods(c: char, modifiers: Modifiers) -> KeyEvent {
        KeyEvent {
            key: CellKey::Char(c),
            pressed: true,
            modifiers,
        }
    }

    fn release(c: char) -> KeyEvent {
        KeyEvent {
            key: CellKey::Char(c),
            pressed: false,
            modifiers: Modifiers::default(),
        }
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
        assert_eq!(model.cells.len(), 300);
        assert_eq!(model.cells[0].label, "AQ");
        assert_eq!((model.cells[0].w, model.cells[0].h), (192.0, 36.0));
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
    fn two_key_cell_selection_opens_subgrid_then_clicks_subcell_center() {
        let mut d = daemon();
        d.on_layer_event(LayerEvent::LayerChanged(OVERLAY_LAYER.into()))
            .unwrap();
        // 'a' picks column 0 (top-left 192×36 cell slice), 'q' picks row 0.
        // Only after both level keys is the cell selected and the subgrid up.
        assert_eq!(
            d.handle_cell_key(CellKey::Char('a')).unwrap(),
            SessionStep::Continue
        );
        assert_eq!(d.phase(), Phase::Overlay);
        assert_eq!(
            d.handle_cell_key(CellKey::Char('q')).unwrap(),
            SessionStep::Continue
        );
        assert_eq!(d.phase(), Phase::Subgrid);
        // Subgrid key 'd' is sub-cell (2, 1): center = (48, 18).
        assert_eq!(
            d.handle_cell_key(CellKey::Char('d')).unwrap(),
            SessionStep::Clicked
        );
        // Target (48, 18); the closed loop settles within tolerance.
        assert_near(d.mouse.position(), (48, 18));
        assert_eq!(d.mouse.click_positions.len(), 1);
        assert_near(d.mouse.click_positions[0], (48, 18));
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
        d.handle_cell_key(CellKey::Char('q')).unwrap();
        assert_eq!(
            d.handle_cell_key(CellKey::Space).unwrap(),
            SessionStep::Clicked
        );
        // Cell (0, 0) is 192×36 at the top-left: center (96, 18).
        assert_near(d.mouse.position(), (96, 18));
        assert!(!d.overlay_visible());
    }

    #[test]
    fn space_without_selection_clicks_screen_center() {
        let mut d = daemon();
        d.on_layer_event(LayerEvent::LayerChanged(OVERLAY_LAYER.into()))
            .unwrap();
        assert_eq!(
            d.handle_cell_key(CellKey::Space).unwrap(),
            SessionStep::Clicked
        );
        // Screen center (960, 540), within the closed-loop tolerance.
        assert_near(d.mouse.position(), (960, 540));
        assert_eq!(d.mouse.clicks, vec![MouseButton::Left]);
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
        let model = build_subgrid_model(&default_grid(), &default_subgrid(), &monitor, 0).unwrap();
        assert_eq!(model.cells.len(), 30);
        assert_eq!(model.cells[0].label, "Q");
        assert_eq!(
            (
                model.cells[0].x,
                model.cells[0].y,
                model.cells[0].w,
                model.cells[0].h
            ),
            (0.0, 0.0, 19.2, 12.0)
        );
        let last = &model.cells[29];
        assert!((last.x - 172.8).abs() < 1e-6);
        assert!((last.y - 24.0).abs() < 1e-6);
        assert!((last.w - 19.2).abs() < 1e-6);
        assert!((last.h - 12.0).abs() < 1e-6);
        assert_eq!(model.cells[29].label, "/");
    }

    #[test]
    fn multi_click_counts_within_threshold_then_finishes() {
        let mut d = daemon_with(
            DaemonConfig {
                multi_click_threshold_ms: 500,
                ..Default::default()
            },
            layout_1080p(),
        );
        d.on_layer_event(LayerEvent::LayerChanged(OVERLAY_LAYER.into()))
            .unwrap();
        d.handle_key_event_at(key('a'), 10).unwrap();
        d.handle_key_event_at(key('q'), 20).unwrap();
        assert_eq!(
            d.handle_key_event_at(key('d'), 100).unwrap(),
            SessionStep::Clicked
        );
        assert!(d.overlay_visible());
        assert!(d.click_pending(100));
        assert_eq!(
            d.handle_key_event_at(key('d'), 300).unwrap(),
            SessionStep::Clicked
        );
        assert_eq!(d.click_count(), 2);
        assert_eq!(d.mouse.clicks.len(), 2);
        assert!(d.click_pending(300));
        // The deadline is 800 ms; finishing early hides the overlay.
        d.finish_pending_click();
        assert!(!d.overlay_visible());
        assert!(!d.grab_held());
    }

    #[test]
    fn multi_click_window_expires_on_tick() {
        let mut d = daemon_with(
            DaemonConfig {
                multi_click_threshold_ms: 500,
                ..Default::default()
            },
            layout_1080p(),
        );
        d.on_layer_event(LayerEvent::LayerChanged(OVERLAY_LAYER.into()))
            .unwrap();
        d.handle_key_event_at(key('a'), 0).unwrap();
        d.handle_key_event_at(key('q'), 0).unwrap();
        d.handle_key_event_at(key('d'), 100).unwrap();
        d.tick(599).unwrap();
        assert!(d.overlay_visible());
        d.tick(600).unwrap();
        assert!(!d.overlay_visible());
        assert!(!d.grab_held());
    }

    #[test]
    fn continuous_mode_resets_to_overlay_after_click() {
        let mut d = daemon_with(
            DaemonConfig {
                continuous_mode: true,
                multi_click_threshold_ms: 0,
                ..Default::default()
            },
            layout_1080p(),
        );
        d.on_layer_event(LayerEvent::LayerChanged(OVERLAY_LAYER.into()))
            .unwrap();
        d.handle_cell_key(CellKey::Char('a')).unwrap();
        d.handle_cell_key(CellKey::Char('q')).unwrap();
        assert_eq!(
            d.handle_cell_key(CellKey::Char('d')).unwrap(),
            SessionStep::Clicked
        );
        assert!(d.overlay_visible());
        assert_eq!(d.phase(), Phase::Overlay);
        // The next level-1 key starts a fresh two-key selection.
        assert_eq!(
            d.handle_cell_key(CellKey::Char('f')).unwrap(),
            SessionStep::Continue
        );
        assert_eq!(d.phase(), Phase::Overlay);
        assert_eq!(
            d.handle_cell_key(CellKey::Char('q')).unwrap(),
            SessionStep::Continue
        );
        assert_eq!(d.phase(), Phase::Subgrid);
        assert_eq!(
            d.handle_cell_key(CellKey::Escape).unwrap(),
            SessionStep::Exit
        );
        assert!(!d.grab_held());
    }

    #[test]
    fn drag_begin_continue_and_drop() {
        let mut d = daemon_with(
            DaemonConfig {
                multi_click_threshold_ms: 0,
                ..Default::default()
            },
            layout_1080p(),
        );
        d.on_layer_event(LayerEvent::LayerChanged(OVERLAY_LAYER.into()))
            .unwrap();
        d.handle_cell_key(CellKey::Char('a')).unwrap();
        d.handle_cell_key(CellKey::Char('q')).unwrap();
        let alt = Modifiers {
            alt: true,
            ..Default::default()
        };
        assert_eq!(
            d.handle_key_event_at(key_mods('d', alt), 100).unwrap(),
            SessionStep::Continue
        );
        assert!(d.drag_active());
        assert_eq!(d.mouse.button_downs, vec![MouseButton::Left]);
        assert!(d.overlay_visible());
        // Continue the drag to another sub-cell without releasing.
        assert_eq!(
            d.handle_key_event_at(key_mods('k', alt), 200).unwrap(),
            SessionStep::Continue
        );
        assert!(d.drag_active());
        assert_eq!(d.mouse.button_downs.len(), 1);
        // Drop without Alt: move + button-up, then end.
        assert_eq!(
            d.handle_key_event_at(key('k'), 300).unwrap(),
            SessionStep::Clicked
        );
        assert!(!d.drag_active());
        assert_eq!(d.mouse.button_ups, vec![MouseButton::Left]);
        assert!(!d.overlay_visible());
        assert!(!d.grab_held());
    }

    #[test]
    fn escape_cancels_drag_at_current_cursor() {
        let mut d = daemon();
        d.on_layer_event(LayerEvent::LayerChanged(OVERLAY_LAYER.into()))
            .unwrap();
        d.handle_cell_key(CellKey::Char('a')).unwrap();
        d.handle_cell_key(CellKey::Char('q')).unwrap();
        let alt = Modifiers {
            alt: true,
            ..Default::default()
        };
        d.handle_key_event_at(key_mods('d', alt), 100).unwrap();
        assert!(d.drag_active());
        assert_eq!(
            d.handle_key_event_at(
                KeyEvent {
                    key: CellKey::Escape,
                    pressed: true,
                    modifiers: Modifiers::default(),
                },
                200
            )
            .unwrap(),
            SessionStep::Exit
        );
        assert!(!d.drag_active());
        assert_eq!(d.mouse.button_ups, vec![MouseButton::Left]);
        assert!(!d.overlay_visible());
    }

    #[test]
    fn layer_change_during_drag_releases_button() {
        // Esc in `lyr-mouseless` is a kanata layer-switch, so the daemon sees
        // a LayerChange rather than a CellKey::Escape. Teardown must still
        // release the drag button or it stays down in the compositor.
        let mut d = daemon_with(
            DaemonConfig {
                multi_click_threshold_ms: 0,
                ..Default::default()
            },
            layout_1080p(),
        );
        d.on_layer_event(LayerEvent::LayerChanged(OVERLAY_LAYER.into()))
            .unwrap();
        d.handle_cell_key(CellKey::Char('a')).unwrap();
        d.handle_cell_key(CellKey::Char('q')).unwrap();
        let alt = Modifiers {
            alt: true,
            ..Default::default()
        };
        d.handle_key_event_at(key_mods('d', alt), 100).unwrap();
        assert!(d.drag_active());
        d.on_layer_event(LayerEvent::LayerChanged(BASE_LAYER.into()))
            .unwrap();
        assert!(!d.drag_active());
        assert_eq!(d.mouse.button_ups, vec![MouseButton::Left]);
        assert!(!d.grab_held());
    }

    #[test]
    fn nudge_with_alt_starts_drag_on_release() {
        let mut d = daemon_with(
            DaemonConfig {
                hold_subgrid_for_nudge: true,
                multi_click_threshold_ms: 0,
                ..Default::default()
            },
            layout_1080p(),
        );
        d.on_layer_event(LayerEvent::LayerChanged(OVERLAY_LAYER.into()))
            .unwrap();
        d.handle_cell_key(CellKey::Char('a')).unwrap();
        d.handle_cell_key(CellKey::Char('q')).unwrap();
        d.handle_key_event_at(key('d'), 100).unwrap();
        assert!(d.held_subcell.is_some());
        let alt = Modifiers {
            alt: true,
            ..Default::default()
        };
        assert_eq!(
            d.handle_key_event_at(
                KeyEvent {
                    key: CellKey::Char('d'),
                    pressed: false,
                    modifiers: alt,
                },
                200
            )
            .unwrap(),
            SessionStep::Continue
        );
        assert!(d.drag_active());
        assert_eq!(d.mouse.button_downs, vec![MouseButton::Left]);
        assert_eq!(
            d.handle_key_event_at(
                KeyEvent {
                    key: CellKey::Escape,
                    pressed: true,
                    modifiers: Modifiers::default(),
                },
                300
            )
            .unwrap(),
            SessionStep::Exit
        );
        assert_eq!(d.mouse.button_ups, vec![MouseButton::Left]);
        assert!(!d.drag_active());
    }

    #[test]
    fn space_during_drag_drops_instead_of_clicking() {
        let mut d = daemon_with(
            DaemonConfig {
                multi_click_threshold_ms: 0,
                ..Default::default()
            },
            layout_1080p(),
        );
        d.on_layer_event(LayerEvent::LayerChanged(OVERLAY_LAYER.into()))
            .unwrap();
        d.handle_cell_key(CellKey::Char('a')).unwrap();
        d.handle_cell_key(CellKey::Char('q')).unwrap();
        let alt = Modifiers {
            alt: true,
            ..Default::default()
        };
        d.handle_key_event_at(key_mods('d', alt), 100).unwrap();
        assert!(d.drag_active());
        assert_eq!(
            d.handle_key_event_at(
                KeyEvent {
                    key: CellKey::Space,
                    pressed: true,
                    modifiers: Modifiers::default(),
                },
                200
            )
            .unwrap(),
            SessionStep::Clicked
        );
        assert!(!d.drag_active());
        assert_eq!(d.mouse.clicks, Vec::<MouseButton>::new());
        assert_eq!(d.mouse.button_ups, vec![MouseButton::Left]);
        assert!(!d.overlay_visible());
    }

    #[test]
    fn shift_tap_switches_monitor_by_x_order() {
        let mut d = daemon_with(
            DaemonConfig {
                multi_click_threshold_ms: 0,
                ..Default::default()
            },
            layout_dual(),
        );
        d.on_layer_event(LayerEvent::LayerChanged(OVERLAY_LAYER.into()))
            .unwrap();
        assert_eq!(d.active_monitor().unwrap().x, 0);
        d.handle_key_event_at(
            KeyEvent {
                key: CellKey::ShiftRight,
                pressed: true,
                modifiers: Modifiers {
                    shift: true,
                    ..Default::default()
                },
            },
            100,
        )
        .unwrap();
        d.handle_key_event_at(
            KeyEvent {
                key: CellKey::ShiftRight,
                pressed: false,
                modifiers: Modifiers::default(),
            },
            150,
        )
        .unwrap();
        assert_eq!(d.active_monitor().unwrap().x, 1920);
        // Wraps back to the first monitor.
        d.handle_key_event_at(
            KeyEvent {
                key: CellKey::ShiftRight,
                pressed: true,
                modifiers: Modifiers {
                    shift: true,
                    ..Default::default()
                },
            },
            200,
        )
        .unwrap();
        d.handle_key_event_at(
            KeyEvent {
                key: CellKey::ShiftRight,
                pressed: false,
                modifiers: Modifiers::default(),
            },
            250,
        )
        .unwrap();
        assert_eq!(d.active_monitor().unwrap().x, 0);
    }

    #[test]
    fn damped_step_halves_and_floors() {
        assert_eq!(damped_step(80), 40);
        assert_eq!(damped_step(-80), -40);
        assert_eq!(damped_step(3), 2);
        assert_eq!(damped_step(-3), -2);
        assert_eq!(damped_step(1000), MAX_MOVE_STEP);
        assert_eq!(damped_step(-1000), -MAX_MOVE_STEP);
        assert_eq!(damped_step(0), 0);
    }

    #[test]
    fn move_to_crosses_to_builtin_when_large_deltas_are_clamped() {
        // Regression: a single large REL delta stopped at the external
        // monitor's left edge (x=1920), so overlay clicks on the built-in
        // never landed. Small free-mode steps crossed fine.
        let mut d = daemon_with_mouse(
            DaemonConfig {
                multi_click_threshold_ms: 0,
                ..Default::default()
            },
            layout_live_dual(),
            CursorPos { x: 3060, y: 405 },
            |overlay| CursorMouse {
                max_step: 64,
                barrier_x: Some(1920),
                ..CursorMouse::new(overlay.cursor_handle())
            },
        );
        d.on_layer_event(LayerEvent::LayerChanged(OVERLAY_LAYER.into()))
            .unwrap();
        assert_eq!(d.active_monitor().unwrap().x, 1920);

        // Tap Left Shift: external (right) -> built-in (left).
        d.handle_key_event_at(
            KeyEvent {
                key: CellKey::ShiftLeft,
                pressed: true,
                modifiers: Modifiers {
                    shift: true,
                    ..Default::default()
                },
            },
            100,
        )
        .unwrap();
        d.handle_key_event_at(
            KeyEvent {
                key: CellKey::ShiftLeft,
                pressed: false,
                modifiers: Modifiers::default(),
            },
            150,
        )
        .unwrap();
        assert_eq!(d.active_monitor().unwrap().x, 0);

        // Cell (a, q) + subcell 'd' => (48, 18) on the built-in.
        d.handle_cell_key(CellKey::Char('a')).unwrap();
        d.handle_cell_key(CellKey::Char('q')).unwrap();
        assert_eq!(
            d.handle_cell_key(CellKey::Char('d')).unwrap(),
            SessionStep::Clicked
        );

        let clicked = d.mouse.click_positions[0];
        assert!(
            clicked.0 < 1920,
            "click landed on the wrong monitor: {clicked:?}"
        );
        assert!((clicked.0 - 48).abs() <= 2, "x off target: {clicked:?}");
        assert!((clicked.1 - 18).abs() <= 2, "y off target: {clicked:?}");
        assert_eq!(d.mouse.clicks, vec![MouseButton::Left]);
        d.mouse.assert_bounded();
    }

    #[test]
    fn move_to_converges_on_external_monitor_under_pointer_acceleration() {
        // Regression: the compositor applied ~0.9x acceleration, so the
        // one-shot delta from (2903, 923) landed ~16 px short in x and
        // ~52 px short in y of the (3060, 405) target.
        let mut d = daemon_with_mouse(
            DaemonConfig::default(),
            layout_live_dual(),
            CursorPos { x: 2903, y: 923 },
            |overlay| CursorMouse {
                scale: (9, 10),
                ..CursorMouse::new(overlay.cursor_handle())
            },
        );
        d.execute_action(ActionIntent::Move {
            to: Point {
                x: 3060.0,
                y: 405.0,
            },
        })
        .unwrap();
        let (x, y) = d.mouse.position();
        assert!((x - 3060).abs() <= 2, "x off target: {x}");
        assert!((y - 405).abs() <= 2, "y off target: {y}");
        assert!(
            d.mouse.moves.len() > 1,
            "expected closed-loop corrections, got {:?}",
            d.mouse.moves
        );
        d.mouse.assert_bounded();
    }

    #[test]
    fn move_to_reports_stalled_cursor_instead_of_clicking() {
        // A stale/broken cursor report must fail loudly after a few
        // no-progress steps, not fire hundreds of blind REL events or click.
        let mut d = daemon_with_mouse(
            DaemonConfig::default(),
            layout_1080p(),
            CursorPos { x: 320, y: 270 },
            |overlay| CursorMouse {
                stuck: true,
                ..CursorMouse::new(overlay.cursor_handle())
            },
        );
        let err = d
            .execute_action(ActionIntent::Move {
                to: Point { x: 800.0, y: 600.0 },
            })
            .unwrap_err();
        assert!(matches!(
            err,
            DaemonError::MoveStalled {
                target: (800, 600),
                cursor: (320, 270),
            }
        ));
        assert_eq!(d.mouse.moves.len(), MAX_STALLED_STEPS as usize);
        assert!(d.mouse.clicks.is_empty());
    }

    #[test]
    fn move_to_does_not_oscillate_under_two_times_pointer_acceleration() {
        // Live regression: the compositor applied ~2× acceleration, so
        // sending the full remaining error overshot to the other side of the
        // target, reversed forever, and the post-click cursor visibly danced.
        // Half-error (damped) steps must land instead of alternating.
        let mut d = daemon_with_mouse(
            DaemonConfig::default(),
            layout_1080p(),
            CursorPos { x: 320, y: 270 },
            |overlay| CursorMouse {
                scale: (2, 1),
                ..CursorMouse::new(overlay.cursor_handle())
            },
        );
        d.execute_action(ActionIntent::Move {
            to: Point { x: 400.0, y: 270.0 },
        })
        .unwrap();
        let (x, y) = d.mouse.position();
        assert!((x - 400).abs() <= MOVE_TOLERANCE_PX, "x off target: {x}");
        assert!((y - 270).abs() <= MOVE_TOLERANCE_PX, "y off target: {y}");
        assert!(
            d.mouse.moves.len() < MAX_MOVE_ITERATIONS as usize,
            "did not converge: {:?}",
            d.mouse.moves
        );
        d.mouse.assert_bounded();
    }

    #[test]
    fn move_to_waits_out_a_stale_cursor_reply() {
        // Regression: `QueryCursor` was answered before the compositor
        // applied the last REL step. Reusing that value made the closed loop
        // reverse direction and oscillate around the target (the live
        // post-click ~50 px "dance"). Every step must wait for a changed
        // report instead of trusting the first reply.
        let mut d = daemon_with_mouse(
            DaemonConfig::default(),
            layout_1080p(),
            CursorPos { x: 320, y: 270 },
            |overlay| CursorMouse {
                stale_replies: Some(overlay.cursor_reply_handle()),
                ..CursorMouse::new(overlay.cursor_handle())
            },
        );
        d.execute_action(ActionIntent::Move {
            to: Point { x: 400.0, y: 270.0 },
        })
        .unwrap();
        let (x, y) = d.mouse.position();
        assert!((x - 400).abs() <= MOVE_TOLERANCE_PX, "x off target: {x}");
        assert!((y - 270).abs() <= MOVE_TOLERANCE_PX, "y off target: {y}");
        assert!(
            d.mouse.moves.iter().all(|(dx, _)| *dx > 0),
            "cursor reversed direction: {:?}",
            d.mouse.moves
        );
        assert!(
            d.mouse.moves.len() > 1,
            "expected damped corrections, got {:?}",
            d.mouse.moves
        );
        d.mouse.assert_bounded();
    }

    #[test]
    fn shift_used_for_right_click_does_not_switch_monitor() {
        let mut d = daemon_with(
            DaemonConfig {
                multi_click_threshold_ms: 500,
                ..Default::default()
            },
            layout_dual(),
        );
        d.on_layer_event(LayerEvent::LayerChanged(OVERLAY_LAYER.into()))
            .unwrap();
        d.handle_key_event_at(
            KeyEvent {
                key: CellKey::ShiftLeft,
                pressed: true,
                modifiers: Modifiers {
                    shift: true,
                    ..Default::default()
                },
            },
            100,
        )
        .unwrap();
        d.handle_cell_key(CellKey::Char('a')).unwrap();
        d.handle_cell_key(CellKey::Char('q')).unwrap();
        let shift = Modifiers {
            shift: true,
            ..Default::default()
        };
        d.handle_key_event_at(key_mods('d', shift), 200).unwrap();
        assert_eq!(d.mouse.clicks, vec![MouseButton::Right]);
        d.handle_key_event_at(
            KeyEvent {
                key: CellKey::ShiftLeft,
                pressed: false,
                modifiers: Modifiers::default(),
            },
            250,
        )
        .unwrap();
        assert_eq!(d.active_monitor().unwrap().x, 0);
    }

    #[test]
    fn nudge_moves_by_cell_fraction_and_release_clicks() {
        let mut d = daemon_with(
            DaemonConfig {
                hold_subgrid_for_nudge: true,
                nudges_per_cell: 4,
                multi_click_threshold_ms: 0,
                ..Default::default()
            },
            layout_1080p(),
        );
        d.on_layer_event(LayerEvent::LayerChanged(OVERLAY_LAYER.into()))
            .unwrap();
        d.handle_cell_key(CellKey::Char('a')).unwrap();
        d.handle_cell_key(CellKey::Char('q')).unwrap();
        // 'd' is sub-cell (2, 1): 19.2×12 at (38.4, 12); center (48, 18).
        assert_eq!(
            d.handle_key_event_at(key('d'), 100).unwrap(),
            SessionStep::Continue
        );
        assert!(d.held_subcell.is_some());
        assert_near(d.mouse.position(), (48, 18));
        // Right nudge: 19.2 / 4 = 4.8 px.
        assert_eq!(
            d.handle_key_event_at(
                KeyEvent {
                    key: CellKey::ArrowRight,
                    pressed: true,
                    modifiers: Modifiers::default(),
                },
                200
            )
            .unwrap(),
            SessionStep::Continue
        );
        // The nudge target is the sub-cell center plus one quarter cell.
        assert_near(d.mouse.position(), (53, 18));
        // Release the held subgrid key: click at the nudged target.
        assert_eq!(
            d.handle_key_event_at(release('d'), 300).unwrap(),
            SessionStep::Clicked
        );
        assert_eq!(d.mouse.clicks, vec![MouseButton::Left]);
        assert!(!d.overlay_visible());
    }

    #[test]
    fn hide_cursor_moves_to_configured_corner_after_click() {
        let mut d = daemon_with(
            DaemonConfig {
                hide_cursor_on_click: true,
                cursor_hide_corner: Corner::TopRight,
                multi_click_threshold_ms: 0,
                ..Default::default()
            },
            layout_1080p(),
        );
        d.on_layer_event(LayerEvent::LayerChanged(OVERLAY_LAYER.into()))
            .unwrap();
        d.handle_cell_key(CellKey::Char('a')).unwrap();
        d.handle_cell_key(CellKey::Char('q')).unwrap();
        d.handle_cell_key(CellKey::Char('d')).unwrap();
        // Closed-loop moves settle on the sub-cell, then on the corner.
        assert!(d.mouse.moves.len() > 2);
        assert_eq!(d.mouse.click_positions.len(), 1);
        assert_near(d.mouse.click_positions[0], (48, 18));
        assert_near(d.mouse.position(), (1919, 1));
    }

    #[test]
    fn free_mode_layer_starts_custom_session_without_overlay() {
        let mut d = daemon();
        d.on_layer_event(LayerEvent::LayerChanged(FREE_MODE_LAYER.into()))
            .unwrap();
        assert!(d.free_mode_active());
        assert!(d.grab_held());
        assert!(!d.overlay_visible());
        assert!(!d.overlay.visible);
        d.on_layer_event(LayerEvent::LayerChanged(BASE_LAYER.into()))
            .unwrap();
        assert!(!d.free_mode_active());
        assert!(!d.grab_held());
        assert_eq!(d.kbd.set_layer_calls, vec![BASE_LAYER.to_string()]);
    }

    #[test]
    fn free_mode_moves_wheels_and_holds_buttons() {
        let mut d = daemon_with(
            DaemonConfig {
                free_mode: FreeModeParams {
                    easing: mouseless_state::Easing::Instant,
                    ..Default::default()
                },
                wheel: WheelParams {
                    easing: mouseless_state::Easing::Instant,
                    ..Default::default()
                },
                multi_click_threshold_ms: 0,
                ..Default::default()
            },
            layout_1080p(),
        );
        d.on_layer_event(LayerEvent::LayerChanged(FREE_MODE_LAYER.into()))
            .unwrap();
        d.handle_key_event_at(key('l'), 0).unwrap(); // move right
        d.tick(8).unwrap();
        assert!(d.mouse.moves.iter().any(|(dx, _)| *dx > 0));
        d.handle_key_event_at(release('l'), 16).unwrap();

        d.handle_key_event_at(key('m'), 20).unwrap(); // wheel up
                                                      // The first Instant tick is 0.9 lines, so a whole detent needs two
                                                      // ticks (fractional carry is covered in mouseless-state).
        d.tick(28).unwrap();
        d.tick(36).unwrap();
        assert!(d.mouse.scrolls.iter().any(|(_, dy)| *dy > 0));
        d.handle_key_event_at(release('m'), 44).unwrap();

        d.handle_key_event_at(
            KeyEvent {
                key: CellKey::Space,
                pressed: true,
                modifiers: Modifiers::default(),
            },
            48,
        )
        .unwrap();
        assert_eq!(d.mouse.button_downs, vec![MouseButton::Left]);
        d.handle_key_event_at(
            KeyEvent {
                key: CellKey::Space,
                pressed: false,
                modifiers: Modifiers::default(),
            },
            56,
        )
        .unwrap();
        assert_eq!(d.mouse.button_ups, vec![MouseButton::Left]);
        assert!(d.free_mode_active());
    }

    #[test]
    fn free_mode_auto_off_releases_buttons_and_restores_base() {
        let mut d = daemon_with(
            DaemonConfig {
                free_mode: FreeModeParams {
                    auto_off_secs: 1,
                    ..Default::default()
                },
                multi_click_threshold_ms: 0,
                ..Default::default()
            },
            layout_1080p(),
        );
        d.on_layer_event(LayerEvent::LayerChanged(FREE_MODE_LAYER.into()))
            .unwrap();
        d.handle_key_event_at(
            KeyEvent {
                key: CellKey::Space,
                pressed: true,
                modifiers: Modifiers::default(),
            },
            0,
        )
        .unwrap();
        d.tick(1_000).unwrap();
        assert!(!d.free_mode_active());
        assert_eq!(d.mouse.button_ups, vec![MouseButton::Left]);
        assert!(!d.grab_held());
        assert_eq!(d.kbd.set_layer_calls, vec![BASE_LAYER.to_string()]);
    }

    #[test]
    fn escape_exits_free_mode_and_releases_grab() {
        let mut d = daemon();
        d.on_layer_event(LayerEvent::LayerChanged(FREE_MODE_LAYER.into()))
            .unwrap();
        assert_eq!(
            d.handle_key_event_at(
                KeyEvent {
                    key: CellKey::Escape,
                    pressed: true,
                    modifiers: Modifiers::default(),
                },
                10
            )
            .unwrap(),
            SessionStep::Exit
        );
        assert!(!d.free_mode_active());
        assert!(!d.grab_held());
        assert_eq!(d.kbd.set_layer_calls, vec![BASE_LAYER.to_string()]);
    }

    #[test]
    fn free_mode_ignores_kanata_fallback_layer_without_grab() {
        let mut d = daemon();
        d.on_layer_event(LayerEvent::LayerChanged(KANATA_FREE_MODE_LAYER.into()))
            .unwrap();
        assert!(!d.grab_held());
        assert!(!d.free_mode_active());
        // Kanata already chose its own layer: do not force base.
        assert!(d.kbd.set_layer_calls.is_empty());
    }
}
