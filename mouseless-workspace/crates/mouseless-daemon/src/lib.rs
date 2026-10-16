//! `mouseless-daemon`: thin orchestration over trait backends.
//!
//! The daemon owns NO backend logic (plan §1.2): it wires a [`KeyboardBackend`],
//! [`MouseBackend`], and [`OverlayBackend`] together behind a single-flight
//! overlay machine (one mutex, idempotent show/hide — plan §5).
//!
//! Kill switch (plan §7, audit W2): kanata TCP EOF/reset means kanata died —
//! release any held [`CellGrab`], best-effort overlay-hide, then co-exit with
//! a failure code so the supervisor restarts the stack in order.

use mouseless_ipc::CursorPos;
use mouseless_kbd::{CellGrab, KbdError, KeyboardBackend, LayerEvent};
use mouseless_mouse::{MouseBackend, MouseButton, MouseError};
use mouseless_overlay::{OverlayBackend, OverlayError};
use mouseless_state::OverlayMachine;
use serde::{Deserialize, Serialize};
use thiserror::Error;

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
    /// Kanata died (TCP EOF/reset) — caller must release + co-exit (plan §7).
    #[error("kanata connection lost; co-exit for supervisor restart")]
    KanataLost,
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
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            keyboard_backend: "kanata".into(),
            overlay_backend: "gnome".into(),
            kanata_addr: "127.0.0.1:7070".into(),
            socket_path: mouseless_ipc::SOCKET_PATH.into(),
        }
    }
}

/// Thin orchestrator, generic over backends for testability.
pub struct Daemon<K, M, O> {
    kbd: K,
    mouse: M,
    overlay: O,
    machine: OverlayMachine,
    grab: Option<CellGrab>,
}

impl<K, M, O> Daemon<K, M, O>
where
    K: KeyboardBackend<Error = KbdError>,
    M: MouseBackend<Error = MouseError>,
    O: OverlayBackend<Error = OverlayError>,
{
    /// Wire already-constructed backends together.
    pub fn new(kbd: K, mouse: M, overlay: O) -> Self {
        Self {
            kbd,
            mouse,
            overlay,
            machine: OverlayMachine::new(),
            grab: None,
        }
    }

    /// Handle one layer event. `lyr-mouseless` shows the overlay (acquiring
    /// the [`CellGrab`); anything else hides it (releasing the grab).
    ///
    /// Duplicate events are safe: show/hide are idempotent (plan §5).
    pub fn on_layer_event(&mut self, ev: LayerEvent) -> Result<(), DaemonError> {
        match ev {
            LayerEvent::LayerChanged(name) if name == "lyr-mouseless" => {
                self.machine
                    .show()
                    .expect("show is infallible from any phase");
                if self.grab.is_none() {
                    self.grab = Some(CellGrab::acquire()?);
                }
                Ok(())
            }
            LayerEvent::LayerChanged(_) => {
                self.machine
                    .hide()
                    .expect("hide is infallible from any phase");
                self.grab = None; // Drop releases (plan §5 rule 1)
                Ok(())
            }
        }
    }

    /// Click at the extension-reported cursor offset by `(dx, dy)` and hide.
    ///
    /// Used by the Phase-1.2 cell path: target cell center minus reported
    /// cursor pos → [`mouseless_mouse::compute_delta`] → `move_rel` + click.
    pub fn click_relative(
        &mut self,
        button: MouseButton,
        cursor: CursorPos,
        target: (i32, i32),
    ) -> Result<(), DaemonError> {
        let (dx, dy) =
            mouseless_mouse::compute_delta(target, (cursor.x, cursor.y));
        self.mouse.move_rel(dx, dy)?;
        self.mouse.click(button)?;
        self.machine
            .hide()
            .expect("hide is infallible from any phase");
        self.grab = None;
        Ok(())
    }

    /// Handle kanata TCP EOF/reset: release grab, best-effort hide, signal
    /// co-exit so the supervisor restarts the stack in order (plan §7).
    pub fn on_kanata_lost(&mut self) -> DaemonError {
        self.grab = None;
        let _ = self.overlay.hide(); // best-effort, idempotent
        DaemonError::KanataLost
    }

    /// Whether the overlay is currently shown.
    pub fn overlay_visible(&self) -> bool {
        !matches!(
            self.machine.phase(),
            mouseless_state::Phase::Normal
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mouseless_ipc::CursorPos;
    use mouseless_kbd::impls::FakeKeyboard;
    use mouseless_mouse::impls::FakeMouse;
    use mouseless_overlay::impls::FakeOverlay;

    fn daemon() -> Daemon<FakeKeyboard, FakeMouse, FakeOverlay> {
        Daemon::new(
            FakeKeyboard::new("base"),
            FakeMouse::default(),
            FakeOverlay::default(),
        )
    }

    #[test]
    fn layer_switch_drives_grab_lifecycle() {
        let mut d = daemon();
        d.on_layer_event(LayerEvent::LayerChanged("lyr-mouseless".into()))
            .unwrap();
        assert!(d.overlay_visible());
        assert!(d.grab.is_some());
        // Duplicate LayerChange: still exactly one grab (idempotent).
        d.on_layer_event(LayerEvent::LayerChanged("lyr-mouseless".into()))
            .unwrap();
        assert!(d.grab.is_some());
        d.on_layer_event(LayerEvent::LayerChanged("base".into()))
            .unwrap();
        assert!(!d.overlay_visible());
        assert!(d.grab.is_none());
    }

    #[test]
    fn click_moves_then_releases() {
        let mut d = daemon();
        d.on_layer_event(LayerEvent::LayerChanged("lyr-mouseless".into()))
            .unwrap();
        d.click_relative(
            MouseButton::Left,
            CursorPos { x: 320, y: 270 },
            (1600, 810),
        )
        .unwrap();
        assert_eq!(d.mouse.moves, vec![(1280, 540)]);
        assert_eq!(d.mouse.clicks, vec![MouseButton::Left]);
        assert!(!d.overlay_visible());
        assert!(d.grab.is_none());
    }

    #[test]
    fn kanata_loss_releases_and_signals_coexit() {
        // Plan §8 test #10 (scaffold level): fake TCP drop mid-overlay.
        let mut d = daemon();
        d.on_layer_event(LayerEvent::LayerChanged("lyr-mouseless".into()))
            .unwrap();
        let err = d.on_kanata_lost();
        assert!(matches!(err, DaemonError::KanataLost));
        assert!(d.grab.is_none());
        assert!(!d.overlay.visible);
    }
}
