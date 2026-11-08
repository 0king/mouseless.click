//! Mouse-action semantics: buttons, multi-click cadence, drag/drop, nudges,
//! and the backend-neutral [`ActionIntent`] boundary.
//!
//! State emits intents; the daemon executes them through `MouseBackend`
//! (SPEC-state boundary rule). Nothing here touches I/O or a concrete backend.

use mouseless_core::{Corner, Point, Rect};
use serde::{Deserialize, Serialize};

/// Two-dimensional value in physical pixels (movement) or wheel lines.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Vec2 {
    /// X component (positive = right).
    pub x: f64,
    /// Y component (positive = down for movement, up for wheel).
    pub y: f64,
}

impl Vec2 {
    /// Zero vector.
    pub const ZERO: Self = Self { x: 0.0, y: 0.0 };

    /// Construct a vector.
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    /// Whether both components are exactly zero.
    pub fn is_zero(&self) -> bool {
        self.x == 0.0 && self.y == 0.0
    }

    /// Euclidean length.
    pub fn length(&self) -> f64 {
        self.x.hypot(self.y)
    }

    /// Round to integer deltas (saturating on the `i32` edges).
    pub fn rounded_i32(&self) -> (i32, i32) {
        (self.x.round() as i32, self.y.round() as i32)
    }

    /// Scale both components.
    pub fn scale(&self, factor: f64) -> Self {
        Self::new(self.x * factor, self.y * factor)
    }
}

/// Mouse buttons mirrored without a dependency on `mouseless-mouse`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Button {
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

impl Button {
    /// All five buttons in stable order (tests/diagnostics).
    pub const ALL: [Button; 5] = [
        Button::Left,
        Button::Right,
        Button::Middle,
        Button::Back,
        Button::Forward,
    ];
}

/// Backend-neutral action produced by the state layer.
///
/// `Click.count` is the logical click ordinal (1..=3) for the current
/// multi-click cadence. The daemon emits one native down/up pair per intent:
/// the compositor coalesces consecutive clicks within its own double-click
/// interval, which is exactly the upstream "native double/triple click"
/// behavior and avoids delaying every single click by the threshold.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum ActionIntent {
    /// Click at the target with the given logical click count.
    Click {
        /// Button to click.
        button: Button,
        /// Logical click ordinal, 1..=3.
        count: u8,
    },
    /// Move to an absolute physical-pixel target.
    Move {
        /// Target in compositor physical pixels.
        to: Point,
    },
    /// Press and hold a button at the target (drag start).
    DragBegin {
        /// Button held for the drag.
        button: Button,
    },
    /// Move while the drag button stays held.
    DragTo {
        /// Target in compositor physical pixels.
        to: Point,
    },
    /// Release the held drag button at the current cursor.
    DragRelease,
    /// Emit wheel detents (x right, y up).
    Wheel {
        /// Horizontal detents.
        dx: i32,
        /// Vertical detents.
        dy: i32,
    },
    /// Hide the cursor by moving to a monitor corner.
    CursorHide {
        /// Configured corner.
        corner: Corner,
    },
}

/// Tracks consecutive clicks on the same target for native multi-click.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ClickCounter {
    last_ms: Option<u64>,
    count: u8,
}

impl ClickCounter {
    /// Register a click at `now_ms` and return its logical ordinal (1..=3).
    ///
    /// A threshold of 0 disables coalescing. A fourth click inside the same
    /// window starts a new cadence at 1 (matching a 1→2→3→1 cycle).
    pub fn register(&mut self, now_ms: u64, threshold_ms: u64) -> u8 {
        let within = match self.last_ms {
            Some(last) if threshold_ms > 0 => now_ms >= last && now_ms - last <= threshold_ms,
            _ => false,
        };
        self.count = if within && self.count < 3 {
            self.count + 1
        } else {
            1
        };
        self.last_ms = Some(now_ms);
        self.count
    }

    /// Reset the cadence (new target, new selection, or session end).
    pub fn reset(&mut self) {
        self.last_ms = None;
        self.count = 0;
    }

    /// Current logical count (0 before the first click).
    pub fn count(&self) -> u8 {
        self.count
    }

    /// Whether a click is still inside the multi-click window.
    pub fn pending(&self, now_ms: u64, threshold_ms: u64) -> bool {
        match self.last_ms {
            Some(last) if threshold_ms > 0 => now_ms >= last && now_ms - last <= threshold_ms,
            _ => false,
        }
    }
}

/// Drag/drop lifecycle: idle, or holding one button.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DragState {
    button: Option<Button>,
}

impl DragState {
    /// Begin (or continue) a drag with `button`.
    pub fn begin(&mut self, button: Button) {
        self.button = Some(button);
    }

    /// Whether a drag is in progress.
    pub fn is_dragging(&self) -> bool {
        self.button.is_some()
    }

    /// Button held by the drag, if any.
    pub fn button(&self) -> Option<Button> {
        self.button
    }

    /// Finish the drag and return the released button.
    pub fn end(&mut self) -> Option<Button> {
        self.button.take()
    }
}

/// Nudge hotkey direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NudgeDirection {
    /// Up arrow.
    Up,
    /// Down arrow.
    Down,
    /// Left arrow.
    Left,
    /// Right arrow.
    Right,
}

/// Cursor nudge math: one step is `cell_size / nudges_per_cell`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NudgeState {
    /// Nudges per cell edge (minimum 1).
    pub nudges_per_cell: u32,
}

impl NudgeState {
    /// Create with the configured precision.
    pub fn new(nudges_per_cell: u32) -> Self {
        Self {
            nudges_per_cell: nudges_per_cell.max(1),
        }
    }

    /// One nudge step for a cell of the given size.
    pub fn step(&self, cell: &Rect) -> Vec2 {
        let n = self.nudges_per_cell.max(1) as f64;
        Vec2::new(cell.w / n, cell.h / n)
    }

    /// Delta for one nudge in `direction`.
    pub fn delta(&self, direction: NudgeDirection, cell: &Rect) -> Vec2 {
        let step = self.step(cell);
        match direction {
            NudgeDirection::Up => Vec2::new(0.0, -step.y),
            NudgeDirection::Down => Vec2::new(0.0, step.y),
            NudgeDirection::Left => Vec2::new(-step.x, 0.0),
            NudgeDirection::Right => Vec2::new(step.x, 0.0),
        }
    }
}

impl Default for NudgeState {
    fn default() -> Self {
        Self::new(4)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn click_cadence_counts_to_three_then_wraps() {
        let mut c = ClickCounter::default();
        assert_eq!(c.register(1_000, 500), 1);
        assert_eq!(c.register(1_500, 500), 2); // boundary inclusive
        assert_eq!(c.register(1_700, 500), 3);
        assert_eq!(c.register(1_800, 500), 1); // fourth starts over
    }

    #[test]
    fn click_cadence_resets_after_threshold_or_with_zero_threshold() {
        let mut c = ClickCounter::default();
        assert_eq!(c.register(1_000, 500), 1);
        assert_eq!(c.register(1_501, 500), 1); // just outside
        let mut c = ClickCounter::default();
        assert_eq!(c.register(1_000, 0), 1);
        assert_eq!(c.register(1_001, 0), 1);
        assert_eq!(c.register(1_002, 0), 1);
    }

    #[test]
    fn click_cadence_handles_clock_going_backwards() {
        let mut c = ClickCounter::default();
        assert_eq!(c.register(1_000, 500), 1);
        assert_eq!(c.register(900, 500), 1);
    }

    #[test]
    fn click_pending_window() {
        let mut c = ClickCounter::default();
        assert!(!c.pending(1_000, 500));
        c.register(1_000, 500);
        assert!(c.pending(1_500, 500));
        assert!(!c.pending(1_501, 500));
        c.reset();
        assert!(!c.pending(1_000, 500));
        assert_eq!(c.count(), 0);
    }

    #[test]
    fn drag_lifecycle() {
        let mut d = DragState::default();
        assert!(!d.is_dragging());
        d.begin(Button::Left);
        assert!(d.is_dragging());
        assert_eq!(d.button(), Some(Button::Left));
        d.begin(Button::Right); // continue with a different button
        assert_eq!(d.end(), Some(Button::Right));
        assert!(!d.is_dragging());
        assert_eq!(d.end(), None);
    }

    #[test]
    fn nudge_steps_are_cell_size_over_precision() {
        let cell = Rect {
            x: 0.0,
            y: 0.0,
            w: 480.0,
            h: 540.0,
        };
        let n = NudgeState::new(4);
        assert_eq!(n.step(&cell), Vec2::new(120.0, 135.0));
        assert_eq!(n.delta(NudgeDirection::Up, &cell), Vec2::new(0.0, -135.0));
        assert_eq!(n.delta(NudgeDirection::Down, &cell), Vec2::new(0.0, 135.0));
        assert_eq!(n.delta(NudgeDirection::Left, &cell), Vec2::new(-120.0, 0.0));
        assert_eq!(n.delta(NudgeDirection::Right, &cell), Vec2::new(120.0, 0.0));
    }

    #[test]
    fn nudge_precision_zero_is_clamped_to_one() {
        let n = NudgeState::new(0);
        assert_eq!(n.nudges_per_cell, 1);
        let cell = Rect {
            x: 0.0,
            y: 0.0,
            w: 480.0,
            h: 540.0,
        };
        assert_eq!(n.step(&cell), Vec2::new(480.0, 540.0));
    }

    #[test]
    fn action_intent_shapes_cover_phase_two_flows() {
        let click = ActionIntent::Click {
            button: Button::Right,
            count: 2,
        };
        assert!(matches!(
            click,
            ActionIntent::Click {
                button: Button::Right,
                count: 2
            }
        ));
        let move_intent = ActionIntent::Move {
            to: Point { x: 10.0, y: 20.0 },
        };
        assert!(matches!(move_intent, ActionIntent::Move { .. }));
        assert!(matches!(
            ActionIntent::DragBegin {
                button: Button::Left
            },
            ActionIntent::DragBegin { .. }
        ));
        assert!(matches!(
            ActionIntent::CursorHide {
                corner: Corner::TopRight
            },
            ActionIntent::CursorHide { .. }
        ));
    }
}
