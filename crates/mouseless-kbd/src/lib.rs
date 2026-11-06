//! `mouseless-kbd`: keyboard backend abstraction.
//!
//! Kanata owns physical keyboards, tap-hold/HRM, base layers, and `lyr-mouse`
//! (plan SKILL-01). This crate defines the [`KeyboardBackend`] trait so kanata
//! is swappable (`impls/kanata.rs` today; `keyd`, `evdev_raw` tomorrow).
//!
//! No `cmd` anywhere (audit W1): layer events arrive over kanata TCP only.
//! [`EvdevCellGrab`] implements the temp-grab contract (fresh fd per overlay,
//! `EVIOCGKEY` pre-check, drain on release — audits G6/G7); [`CellGrab`] is
//! the hermetic scaffold used by non-production wiring and tests.

pub mod grab;
pub mod impls;
pub mod tcp;

pub use grab::{
    find_output_device, translate_key, CellKey, EvdevCellGrab, KeyEvent, Modifiers,
    KANATA_OUTPUT_NAME,
};
pub use impls::{FakeCellGrab, FakeKeyboard, KanataTcpConfig};

pub use tcp::{
    backoff_for_attempt, parse_server_line, query_current_layer, send_layer_change,
    KanataSubscriber,
};

use thiserror::Error;

/// Base layer restored on every exit path (idempotent).
pub const BASE_LAYER: &str = "lyr-base";
/// Overlay layer (kanata include).
pub const OVERLAY_LAYER: &str = "lyr-mouseless";
/// Custom free-mode layer (kanata include, Phase 2).
pub const FREE_MODE_LAYER: &str = "lyr-free";
/// Kanata-native free-mode fallback layer (`free_mode_engine = kanata`).
pub const KANATA_FREE_MODE_LAYER: &str = "lyr-mouse";

/// Keyboard backend errors (timeout, reconnect, grab contention).
#[derive(Debug, Error)]
pub enum KbdError {
    /// TCP or device I/O failure (reconnect with backoff, plan §5).
    #[error("keyboard backend I/O: {0}")]
    Io(String),
    /// Grab attempt hit `EBUSY` (retry with timeout, never fatal).
    #[error("device busy (EBUSY); retry with timeout")]
    Busy,
    /// Overlay unavailable; typing must be unaffected (degraded mode).
    #[error("overlay unavailable: {0}")]
    Unavailable(String),
}

/// Layer events from the keyboard backend (kanata TCP `LayerChange`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LayerEvent {
    /// Kanata switched to `name` (e.g. `lyr-mouseless`).
    LayerChanged(String),
}

/// Swappable keyboard backend (plan §1.8, §3).
///
/// Implementations must reconnect with exponential backoff, resync the current
/// layer on every (re)connect (kanata pushes `LayerChange` on connect; a
/// backend MAY also send `RequestCurrentLayerName`), and treat duplicate or
/// missed `LayerChange` events as idempotent (plan §5).
pub trait KeyboardBackend {
    /// Error type.
    type Error: std::error::Error;
    /// Blocking poll for the next layer event (with timeout).
    fn next_layer_event(&mut self) -> Result<LayerEvent, Self::Error>;
    /// Request a layer change (`ChangeLayer`).
    fn set_layer(&mut self, name: &str) -> Result<(), Self::Error>;
    /// Resync: query the current layer name (`RequestCurrentLayerName`).
    fn current_layer(&mut self) -> Result<String, Self::Error>;
    /// Reload configuration (`Reload*`).
    fn reload(&mut self) -> Result<(), Self::Error>;
}

/// Polling interface for the RAII grab used by the daemon session loop.
///
/// [`EvdevCellGrab`] is the production implementation; [`CellGrab`] (the
/// scaffold) and `FakeCellGrab` implement it so daemon tests stay hermetic.
pub trait CellKeySource {
    /// Non-blocking poll for one cell key press.
    fn next_key_press(&mut self) -> Result<Option<CellKey>, KbdError>;
    /// Non-blocking poll for one press/release event with modifier state.
    ///
    /// The default preserves press-only backends; evdev and the fake override
    /// it with real release tracking.
    fn next_key_event(&mut self) -> Result<Option<KeyEvent>, KbdError> {
        Ok(self.next_key_press()?.map(|key| KeyEvent {
            key,
            pressed: true,
            modifiers: Modifiers::default(),
        }))
    }
    /// Whether the exclusive grab is currently held.
    fn held(&self) -> bool;
}

impl CellKeySource for CellGrab {
    fn next_key_press(&mut self) -> Result<Option<CellKey>, KbdError> {
        Ok(None)
    }

    fn held(&self) -> bool {
        self.held()
    }
}

/// RAII temporary cell-key grab (plan §5 rule 1).
///
/// Contract (audits G6/G7), enforced by the Phase-1.2 evdev implementation:
/// 1. Opens the stable `kanata-mouseless` output device **fresh on each
///    overlay invocation** (never reuse an fd — the kernel per-client buffer
///    would replay stale typing / `SYN_DROPPED`).
/// 2. On acquire, snapshots `EVIOCGKEY`; if a key is already down, waits for
///    release or synthesizes the matching key-up before grabbing (no stuck keys).
/// 3. On release (`Drop`), drains the fd before close (no phantom key-ups).
/// 4. Kernel guarantees grab release on fd close, even on SIGKILL.
///
/// This scaffold always reports "held" and reads no keys; it keeps non-evdev
/// unit tests hermetic. Production wiring uses [`EvdevCellGrab`].
#[derive(Debug, Default)]
pub struct CellGrab {
    held: bool,
}

impl CellGrab {
    /// Acquire the grab (Phase 1.2 performs the real `EVIOCGRAB`).
    pub fn acquire() -> Result<Self, KbdError> {
        Ok(Self { held: true })
    }

    /// Acquire by device name; the scaffold ignores the name so tests and
    /// non-evdev wiring can share the production grab factory signature.
    pub fn acquire_named(_name: &str) -> Result<Self, KbdError> {
        Self::acquire()
    }

    /// Whether the grab is currently held.
    pub fn held(&self) -> bool {
        self.held
    }

    /// Release explicitly (equivalent to dropping).
    pub fn release(mut self) {
        self.held = false;
    }
}

impl Drop for CellGrab {
    fn drop(&mut self) {
        // Real impl: EVIOCGRAB(0) + drain + close. Always runs, even on panic.
        self.held = false;
    }
}

#[cfg(test)]
mod tests {
    use super::impls::FakeKeyboard;
    use super::*;

    #[test]
    fn grab_release_on_every_path_incl_drop() {
        // Plan §8 test #2 (scaffold level): Drop always releases.
        let grab = CellGrab::acquire().unwrap();
        assert!(grab.held());
        drop(grab);
        let grab = CellGrab::acquire().unwrap();
        grab.release();
    }

    #[test]
    fn fake_backend_resync_idempotent() {
        // Plan §8 test #3 (scaffold level): missed/duplicate LayerChange safe.
        let mut kbd = FakeKeyboard::new("base");
        kbd.push_event(LayerEvent::LayerChanged("lyr-mouseless".into()));
        kbd.push_event(LayerEvent::LayerChanged("lyr-mouseless".into())); // dupe
        assert_eq!(
            kbd.next_layer_event().unwrap(),
            LayerEvent::LayerChanged("lyr-mouseless".into())
        );
        assert_eq!(
            kbd.next_layer_event().unwrap(),
            LayerEvent::LayerChanged("lyr-mouseless".into())
        );
        kbd.set_layer("base").unwrap();
        assert_eq!(kbd.current_layer().unwrap(), "base");
    }
}
