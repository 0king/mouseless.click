//! `mouseless-state`: overlay state machine + action semantics.
//!
//! I/O-free by construction (plan §2.2 key rule): geometry arrives as
//! parameters, time arrives via the [`Clock`] trait. Deterministic tests use
//! [`ManualClock`]; production uses [`SystemClock`].
//!
//! State model (plan §4): `Normal → Overlay → Subgrid → Click`. The overlay
//! machine is single-flight with idempotent show/hide so duplicate or missed
//! `LayerChange` events cannot desync it (plan §5).
//!
//! Module split (SPEC-state project structure):
//! - [`machine`] — overlay phase machine.
//! - [`actions`] — buttons, multi-click cadence, drag/drop, nudges, the
//!   backend-neutral [`ActionIntent`] boundary, and [`Vec2`] geometry.

mod actions;
mod machine;

pub use actions::{
    ActionIntent, Button, ClickCounter, DragState, NudgeDirection, NudgeState, Vec2,
};
pub use machine::{OverlayMachine, Phase, StateError};

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
