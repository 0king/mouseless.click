//! `mouseless-state`: overlay state machine + free-mode physics + action
//! semantics.
//!
//! I/O-free by construction (plan §2.2 key rule): geometry arrives as
//! parameters, time arrives via the [`Clock`] trait. Deterministic tests use
//! [`ManualClock`]; production uses [`SystemClock`].
//!
//! State model (plan §4): `Normal → Overlay → Subgrid → Click`, plus the
//! separate [`FreeModeSession`] session. The overlay machine is single-flight
//! with idempotent show/hide so duplicate or missed `LayerChange` events
//! cannot desync it (plan §5).
//!
//! Module split (SPEC-state project structure):
//! - [`machine`] — overlay phase machine.
//! - [`physics`] — free-mode movement/wheel physics, key state, auto-off.
//! - [`actions`] — buttons, multi-click cadence, drag/drop, nudges, and the
//!   backend-neutral [`ActionIntent`] boundary.

mod actions;
mod machine;
mod physics;

pub use actions::{ActionIntent, Button, ClickCounter, DragState, NudgeDirection, NudgeState};
pub use machine::{OverlayMachine, Phase, StateError};
pub use physics::{
    Easing, FreeMode, FreeModeFrame, FreeModeKey, FreeModeKeys, FreeModeParams, FreeModeSession,
    Vec2, WheelParams,
};

use serde::{Deserialize, Serialize};

/// Injectable time source. Production uses [`SystemClock`], tests use
/// [`ManualClock`]. Milliseconds since an arbitrary epoch.
pub trait Clock {
    /// Current time in milliseconds.
    fn now_ms(&self) -> u64;
}

/// Wall-clock implementation.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> u64 {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }
}

/// Deterministic fake clock for tests (plan §8 test #1, #7).
#[derive(Debug, Clone, Copy, Default)]
pub struct ManualClock {
    /// Current fake time in ms.
    pub now: u64,
}

impl ManualClock {
    /// Advance the fake clock.
    pub fn advance(&mut self, ms: u64) {
        self.now = self.now.saturating_add(ms);
    }
}

impl Clock for ManualClock {
    fn now_ms(&self) -> u64 {
        self.now
    }
}

/// Which mouse engine drives free mode (plan §1.7).
///
/// Both engines coexist in MVP; kanata's `lyr-mouse` is the fallback preset and
/// its removal is explicitly post-MVP.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum FreeModeEngine {
    /// Custom physics in `mouseless-state` (default).
    #[default]
    Custom,
    /// Kanata's built-in `lyr-mouse` layer.
    Kanata,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manual_clock_advances_saturating() {
        let mut clock = ManualClock::default();
        clock.advance(8);
        assert_eq!(clock.now_ms(), 8);
        clock.now = u64::MAX;
        clock.advance(1);
        assert_eq!(clock.now_ms(), u64::MAX);
    }
}
