//! Overlay phase machine: `Normal → Overlay → Subgrid → Click`.
//!
//! Single-flight with idempotent show/hide so duplicate or missed
//! `LayerChange` events cannot desync it (plan §5).

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

    /// Return to the level-1 overlay without leaving the session.
    ///
    /// Used by continuous mode and multi-monitor switching: the grab and
    /// overlay stay alive, but the selection/subgrid resets.
    pub fn reset_to_overlay(&mut self) -> Result<(), StateError> {
        self.phase = Phase::Overlay;
        Ok(())
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
    fn reset_to_overlay_keeps_session_alive() {
        let mut m = OverlayMachine::new();
        m.show().unwrap();
        m.open_subgrid().unwrap();
        m.click().unwrap();
        m.reset_to_overlay().unwrap();
        assert_eq!(m.phase(), Phase::Overlay);
    }
}
