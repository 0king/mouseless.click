//! `mouseless-state`: overlay state machine + free-mode physics.
//!
//! I/O-free by construction (plan §2.2 key rule): geometry arrives as
//! parameters, time arrives via the [`Clock`] trait. Deterministic tests use
//! [`ManualClock`]; production uses [`SystemClock`].
//!
//! State model (plan §4): `Normal → Overlay → Subgrid → Click`, plus
//! `FreeMode`. The machine is single-flight with idempotent show/hide so
//! duplicate or missed `LayerChange` events cannot desync it (plan §5).

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Errors from state transitions.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum StateError {
    /// Event is invalid in the current phase (ignored, never fatal).
    #[error("event {event} invalid in phase {phase}")]
    InvalidEvent {
        /// Current phase name.
        phase: &'static str,
        /// Event name.
        event: &'static str,
    },
}

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

/// Movement easing curve (upstream `Movement easing factor`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub enum Easing {
    /// 0.1 — loose.
    Loose,
    /// 0.2 — natural (default).
    #[default]
    Natural,
    /// 0.3 — snappy.
    Snappy,
    /// 0.9 — instant.
    Instant,
}

impl Easing {
    /// Per-tick smoothing factor.
    pub fn factor(&self) -> f64 {
        match self {
            Easing::Loose => 0.1,
            Easing::Natural => 0.2,
            Easing::Snappy => 0.3,
            Easing::Instant => 0.9,
        }
    }
}

/// Free-mode motion parameters (upstream `Free mode` behavior options).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct FreeModeParams {
    /// Base move speed in px per tick.
    pub base_px_per_tick: f64,
    /// Speed multiplier per held `S/D/F` key.
    pub speed_multiplier: f64,
    /// Movement easing curve.
    pub easing: Easing,
    /// Auto-off after N idle seconds; 0 disables.
    pub auto_off_secs: u64,
}

impl Default for FreeModeParams {
    fn default() -> Self {
        Self {
            base_px_per_tick: 10.0,
            speed_multiplier: 2.0,
            easing: Easing::Natural,
            auto_off_secs: 10,
        }
    }
}

/// Free-mode cursor dynamics: exponential approach to target velocity.
///
/// `tick(dt)` is fixed-step; callers own the loop (tokio timer or dedicated
/// thread — the physics core stays sync and uncolored, audit G5).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FreeMode {
    /// Current velocity in px/tick.
    pub vel: f64,
    /// Parameters.
    pub params: FreeModeParams,
}

impl FreeMode {
    /// Create at rest.
    pub fn new(params: FreeModeParams) -> Self {
        Self { vel: 0.0, params }
    }

    /// One fixed-step tick toward `target_vel` (px/tick), scaled by held
    /// speed-boost count (`S/D/F`) and `decel` (`A` held).
    pub fn tick(&mut self, target_vel: f64, boosts: u32, decel: bool) -> f64 {
        let mut target = target_vel * self.params.speed_multiplier.powi(boosts as i32);
        if decel {
            target /= self.params.speed_multiplier;
        }
        let k = self.params.easing.factor();
        self.vel += (target - self.vel) * k;
        self.vel
    }
}

/// Overlay phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// No overlay visible.
    Normal,
    /// Overlay visible, selecting a level-1/2 cell.
    Overlay,
    /// Subgrid open inside a cell.
    Subgrid,
    /// A click was executed (transient; machine returns to Normal or stays
    /// visible in continuous mode — Phase 2).
    Click,
}

/// Single-flight overlay machine: idempotent show/hide (plan §5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OverlayMachine {
    phase: Phase,
}

impl OverlayMachine {
    /// Start in [`Phase::Normal`].
    pub fn new() -> Self {
        Self {
            phase: Phase::Normal,
        }
    }

    /// Current phase.
    pub fn phase(&self) -> Phase {
        self.phase
    }

    /// Show the overlay. Idempotent: showing while visible is a no-op `Ok`.
    pub fn show(&mut self) -> Result<(), StateError> {
        match self.phase {
            Phase::Normal | Phase::Click => {
                self.phase = Phase::Overlay;
                Ok(())
            }
            Phase::Overlay | Phase::Subgrid => Ok(()),
        }
    }

    /// Hide the overlay. Idempotent: hiding while hidden is a no-op `Ok`.
    pub fn hide(&mut self) -> Result<(), StateError> {
        match self.phase {
            Phase::Overlay | Phase::Subgrid | Phase::Click => {
                self.phase = Phase::Normal;
                Ok(())
            }
            Phase::Normal => Ok(()),
        }
    }

    /// Open the subgrid for the selected cell.
    pub fn open_subgrid(&mut self) -> Result<(), StateError> {
        match self.phase {
            Phase::Overlay => {
                self.phase = Phase::Subgrid;
                Ok(())
            }
            _ => Err(StateError::InvalidEvent {
                phase: self.phase_name(),
                event: "open_subgrid",
            }),
        }
    }

    /// Record a click executed from the current phase.
    pub fn click(&mut self) -> Result<(), StateError> {
        match self.phase {
            Phase::Overlay | Phase::Subgrid => {
                self.phase = Phase::Click;
                Ok(())
            }
            _ => Err(StateError::InvalidEvent {
                phase: self.phase_name(),
                event: "click",
            }),
        }
    }

    fn phase_name(&self) -> &'static str {
        match self.phase {
            Phase::Normal => "Normal",
            Phase::Overlay => "Overlay",
            Phase::Subgrid => "Subgrid",
            Phase::Click => "Click",
        }
    }
}

impl Default for OverlayMachine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn show_hide_idempotent() {
        // Duplicate/missed LayerChange events must not desync (plan §5).
        let mut m = OverlayMachine::new();
        assert_eq!(m.phase(), Phase::Normal);
        m.hide().unwrap(); // no-op while hidden
        m.show().unwrap();
        m.show().unwrap(); // duplicate show: no-op
        assert_eq!(m.phase(), Phase::Overlay);
        m.hide().unwrap();
        m.hide().unwrap(); // duplicate hide: no-op
        assert_eq!(m.phase(), Phase::Normal);
    }

    #[test]
    fn full_flow_overlay_subgrid_click_hide() {
        let mut m = OverlayMachine::new();
        m.show().unwrap();
        m.open_subgrid().unwrap();
        m.click().unwrap();
        assert_eq!(m.phase(), Phase::Click);
        m.hide().unwrap();
        assert_eq!(m.phase(), Phase::Normal);
    }

    #[test]
    fn invalid_events_rejected_not_fatal() {
        let mut m = OverlayMachine::new();
        assert!(m.open_subgrid().is_err());
        assert!(m.click().is_err());
        assert_eq!(m.phase(), Phase::Normal);
    }

    #[test]
    fn easing_factors_match_upstream() {
        assert_eq!(Easing::Loose.factor(), 0.1);
        assert_eq!(Easing::Natural.factor(), 0.2);
        assert_eq!(Easing::Snappy.factor(), 0.3);
        assert_eq!(Easing::Instant.factor(), 0.9);
    }

    #[test]
    fn physics_converges_per_curve_with_fake_clock() {
        // Golden test per easing curve (plan §8 test #7): velocity must
        // converge monotonically toward the target.
        for easing in [Easing::Loose, Easing::Natural, Easing::Snappy, Easing::Instant] {
            let mut fm = FreeMode::new(FreeModeParams {
                easing,
                ..Default::default()
            });
            let mut clock = ManualClock::default();
            let mut prev = 0.0;
            for _ in 0..200 {
                clock.advance(8); // ~120 Hz fixed-step tick
                let v = fm.tick(100.0, 0, false);
                assert!(v >= prev, "{easing:?} diverged");
                prev = v;
            }
            assert!(
                (prev - 100.0).abs() < 1.0,
                "{easing:?} did not converge: {prev}"
            );
        }
    }

    #[test]
    fn boosts_and_decel_scale_target() {
        let mut fm = FreeMode::new(FreeModeParams {
            easing: Easing::Instant,
            ..Default::default()
        });
        let v = fm.tick(10.0, 2, false); // S+D held: ×2²
        assert!((v - 10.0 * 4.0 * 0.9).abs() < 1e-6);
        let mut fm = FreeMode::new(FreeModeParams {
            easing: Easing::Instant,
            ..Default::default()
        });
        let v = fm.tick(10.0, 0, true); // A held: ÷2
        assert!((v - 5.0 * 0.9).abs() < 1e-6);
    }
}
