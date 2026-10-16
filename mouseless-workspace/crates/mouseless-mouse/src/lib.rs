//! `mouseless-mouse`: mouse backend abstraction.
//!
//! The daemon owns a uinput **mouse** (`mouseless-mouse`, vendor `0x736e`,
//! kanata-excluded via `linux-dev-names-exclude`). It emits REL deltas +
//! `BTN_*`/wheel computed from extension-supplied cursor position, because
//! Linux kanata `setmouse`/`SetMouse` cannot absolute-position (plan §2.1).
//!
//! Accuracy contract (audit G1): the `mouseless-mouse` device MUST use a flat
//! pointer-acceleration profile (udev
//! `ENV{LIBINPUT_ATTR_POINTER_ACCEL}="flat"` in `platform/`), otherwise
//! libinput distorts large deltas and clicks land off-target.

pub mod impls;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Mouse backend errors.
#[derive(Debug, Error)]
pub enum MouseError {
    /// uinput device I/O failure.
    #[error("mouse backend I/O: {0}")]
    Io(String),
}

/// Mouse buttons (upstream global mouse buttons + overlay actions).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MouseButton {
    /// Left button.
    Left,
    /// Right button.
    Right,
    /// Middle button.
    Middle,
    /// Back button.
    Back,
    /// Forward button.
    Forward,
}

/// Swappable mouse backend (plan §1.8).
///
/// `uinput` today; `libei`/portal tomorrow (plan §4.1, Phase 4).
pub trait MouseBackend {
    /// Error type.
    type Error: std::error::Error;
    /// Emit a relative motion event.
    fn move_rel(&mut self, dx: i32, dy: i32) -> Result<(), Self::Error>;
    /// Emit a button click (down + up).
    fn click(&mut self, button: MouseButton) -> Result<(), Self::Error>;
    /// Emit a wheel scroll in lines (positive = up/right).
    fn scroll(&mut self, dx_lines: i32, dy_lines: i32) -> Result<(), Self::Error>;
}

/// Compute the REL delta from the extension-reported cursor position to a
/// click target, clamping to the `i32` range the evdev `REL_X`/`REL_Y` fields
/// carry (plan §8 test #5: overflow/clamp/multi-monitor offsets).
///
/// Returns `(dx, dy)` with `target - current` per axis, saturated.
pub fn compute_delta(target: (i32, i32), current: (i32, i32)) -> (i32, i32) {
    (
        target.0.saturating_sub(current.0),
        target.1.saturating_sub(current.1),
    )
}

#[cfg(test)]
mod tests {
    use super::impls::FakeMouse;
    use super::*;

    #[test]
    fn delta_basic_and_negative() {
        assert_eq!(compute_delta((1600, 810), (320, 270)), (1280, 540));
        assert_eq!(compute_delta((0, 0), (100, 200)), (-100, -200));
        assert_eq!(compute_delta((5, 5), (5, 5)), (0, 0));
    }

    #[test]
    fn delta_saturates_at_i32_edges() {
        assert_eq!(compute_delta((i32::MAX, i32::MAX), (i32::MIN, 0)).0, i32::MAX);
        assert_eq!(
            compute_delta((i32::MIN, i32::MIN), (i32::MAX, 0)).0,
            i32::MIN
        );
    }

    #[test]
    fn fake_mouse_records_events() {
        let mut m = FakeMouse::default();
        m.move_rel(10, -5).unwrap();
        m.click(MouseButton::Left).unwrap();
        m.scroll(0, 3).unwrap();
        assert_eq!(m.moves, vec![(10, -5)]);
        assert_eq!(m.clicks, vec![MouseButton::Left]);
        assert_eq!(m.scrolls, vec![(0, 3)]);
    }
}
