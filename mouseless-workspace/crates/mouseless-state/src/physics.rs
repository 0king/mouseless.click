//! Free-mode physics: 2-D movement, wheel dynamics, key state, and auto-off.
//!
//! The core is sync and uncolored (audit G5): callers own the loop and pass
//! fixed-step ticks. Every time-dependent rule takes an explicit `now_ms`, so
//! tests drive it with [`ManualClock`](crate::ManualClock) instead of sleeping.
//!
//! Upstream mapping (mouseless.click docs):
//! - move `i/k/j/l`, boost `s/d/f`, decel `a`
//! - buttons `space/r/e/q/w` (left/right/middle/back/forward)
//! - wheel `m/,/.//` (up/down/left/right)

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

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

/// Wheel physics parameters (upstream `Free mode` wheel options).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WheelParams {
    /// Base wheel speed in lines per tick.
    pub base_lines_per_tick: f64,
    /// Speed multiplier per held boost key / fast wheel action.
    pub speed_multiplier: f64,
    /// Wheel easing curve.
    pub easing: Easing,
    /// Discrete wheel step size (lines).
    pub step_size: i32,
    /// Large discrete wheel step size (lines).
    pub step_size_large: i32,
}

impl Default for WheelParams {
    fn default() -> Self {
        Self {
            base_lines_per_tick: 1.0,
            speed_multiplier: 2.0,
            easing: Easing::Natural,
            step_size: 3,
            step_size_large: 10,
        }
    }
}

/// A key understood by the custom free-mode engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FreeModeKey {
    /// `i` — move up.
    MoveUp,
    /// `k` — move down.
    MoveDown,
    /// `j` — move left.
    MoveLeft,
    /// `l` — move right.
    MoveRight,
    /// `s` — first speed boost.
    Boost1,
    /// `d` — second speed boost.
    Boost2,
    /// `f` — third speed boost.
    Boost3,
    /// `a` — speed decrease.
    Decel,
    /// `m` — wheel up.
    WheelUp,
    /// `,` — wheel down.
    WheelDown,
    /// `.` — wheel left.
    WheelLeft,
    /// `/` — wheel right.
    WheelRight,
    /// `space` — left button (hold to drag).
    LeftButton,
    /// `r` — right button.
    RightButton,
    /// `e` — middle button.
    MiddleButton,
    /// `q` — back button.
    BackButton,
    /// `w` — forward button.
    ForwardButton,
    /// `esc` — exit free mode.
    Exit,
}

impl FreeModeKey {
    /// Whether this key is one of the five mouse buttons.
    pub fn is_button(self) -> bool {
        matches!(
            self,
            FreeModeKey::LeftButton
                | FreeModeKey::RightButton
                | FreeModeKey::MiddleButton
                | FreeModeKey::BackButton
                | FreeModeKey::ForwardButton
        )
    }
}

/// Currently held free-mode keys.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FreeModeKeys {
    pressed: BTreeSet<FreeModeKey>,
}

impl FreeModeKeys {
    /// Press a key (idempotent).
    pub fn press(&mut self, key: FreeModeKey) {
        self.pressed.insert(key);
    }

    /// Release a key (idempotent).
    pub fn release(&mut self, key: FreeModeKey) {
        self.pressed.remove(&key);
    }

    /// Whether a key is currently held.
    pub fn is_pressed(&self, key: FreeModeKey) -> bool {
        self.pressed.contains(&key)
    }

    /// Release every key.
    pub fn clear(&mut self) {
        self.pressed.clear();
    }

    /// Number of held speed-boost keys (`s/d/f`).
    pub fn boosts(&self) -> u32 {
        [
            FreeModeKey::Boost1,
            FreeModeKey::Boost2,
            FreeModeKey::Boost3,
        ]
        .iter()
        .filter(|key| self.pressed.contains(key))
        .count() as u32
    }

    /// Whether the decel key (`a`) is held.
    pub fn decel(&self) -> bool {
        self.is_pressed(FreeModeKey::Decel)
    }

    /// Movement axis: x positive right, y positive down.
    pub fn movement_axis(&self) -> Vec2 {
        let x = self.is_pressed(FreeModeKey::MoveRight) as i8
            - self.is_pressed(FreeModeKey::MoveLeft) as i8;
        let y = self.is_pressed(FreeModeKey::MoveDown) as i8
            - self.is_pressed(FreeModeKey::MoveUp) as i8;
        Vec2::new(x as f64, y as f64)
    }

    /// Wheel axis: x positive right, y positive up (matches `scroll`).
    pub fn wheel_axis(&self) -> Vec2 {
        let x = self.is_pressed(FreeModeKey::WheelRight) as i8
            - self.is_pressed(FreeModeKey::WheelLeft) as i8;
        let y = self.is_pressed(FreeModeKey::WheelUp) as i8
            - self.is_pressed(FreeModeKey::WheelDown) as i8;
        Vec2::new(x as f64, y as f64)
    }

    /// Whether any motion/wheel/boost/decel key is held.
    pub fn any_motion(&self) -> bool {
        !self.movement_axis().is_zero()
            || !self.wheel_axis().is_zero()
            || self.boosts() > 0
            || self.decel()
    }

    /// Held mouse buttons.
    pub fn held_buttons(&self) -> Vec<FreeModeKey> {
        self.pressed
            .iter()
            .copied()
            .filter(|key| key.is_button())
            .collect()
    }
}

/// Free-mode cursor dynamics: exponential approach to target velocity.
///
/// `advance` is the raw physics step; `move_delta`/`wheel_delta` apply key
/// state and carry fractional remainders so slow motion does not stall.
#[derive(Debug, Clone, PartialEq)]
pub struct FreeMode {
    /// Current movement velocity in px/tick.
    pub vel: Vec2,
    /// Current wheel velocity in lines/tick.
    pub wheel_vel: Vec2,
    /// Movement parameters.
    pub params: FreeModeParams,
    /// Wheel parameters.
    pub wheel: WheelParams,
    move_accum: Vec2,
    wheel_accum: Vec2,
    last_activity_ms: u64,
}

impl FreeMode {
    /// Create at rest with default wheel parameters.
    pub fn new(params: FreeModeParams) -> Self {
        Self::with_wheel(params, WheelParams::default())
    }

    /// Create at rest with explicit movement and wheel parameters.
    pub fn with_wheel(params: FreeModeParams, wheel: WheelParams) -> Self {
        Self {
            vel: Vec2::ZERO,
            wheel_vel: Vec2::ZERO,
            params,
            wheel,
            move_accum: Vec2::ZERO,
            wheel_accum: Vec2::ZERO,
            last_activity_ms: 0,
        }
    }

    /// One scalar fixed-step tick toward `target_vel` (px/tick), scaled by
    /// held boost count and decel.
    ///
    /// Kept as a scalar helper for per-axis tests and callers that only move
    /// along one axis.
    pub fn tick_axis(&mut self, target_vel: f64, boosts: u32, decel: bool) -> f64 {
        let target = scale_target(target_vel, self.params.speed_multiplier, boosts, decel);
        let k = self.params.easing.factor();
        self.vel.x += (target - self.vel.x) * k;
        self.vel.x
    }

    /// One 2-D fixed-step movement tick toward `target`.
    pub fn advance(&mut self, target: Vec2, boosts: u32, decel: bool) -> Vec2 {
        let target = scale_vec(target, self.params.speed_multiplier, boosts, decel);
        let k = self.params.easing.factor();
        self.vel.x += (target.x - self.vel.x) * k;
        self.vel.y += (target.y - self.vel.y) * k;
        self.vel
    }

    /// One 2-D fixed-step wheel tick toward `target` (lines/tick).
    pub fn advance_wheel(&mut self, target: Vec2, boosts: u32, decel: bool) -> Vec2 {
        let target = scale_vec(target, self.wheel.speed_multiplier, boosts, decel);
        let k = self.wheel.easing.factor();
        self.wheel_vel.x += (target.x - self.wheel_vel.x) * k;
        self.wheel_vel.y += (target.y - self.wheel_vel.y) * k;
        self.wheel_vel
    }

    /// Advance movement from key state and return whole-pixel deltas.
    pub fn move_delta(&mut self, keys: &FreeModeKeys) -> (i32, i32) {
        let target = keys.movement_axis().scale(self.params.base_px_per_tick);
        let vel = self.advance(target, keys.boosts(), keys.decel());
        carry(&mut self.move_accum, vel)
    }

    /// Advance wheel from key state and return whole-line detents.
    pub fn wheel_delta(&mut self, keys: &FreeModeKeys) -> (i32, i32) {
        let target = keys.wheel_axis().scale(self.wheel.base_lines_per_tick);
        let vel = self.advance_wheel(target, keys.boosts(), keys.decel());
        carry(&mut self.wheel_accum, vel)
    }

    /// Mark user activity for auto-off.
    pub fn mark_activity(&mut self, now_ms: u64) {
        self.last_activity_ms = now_ms;
    }

    /// Time of the last marked activity.
    pub fn last_activity_ms(&self) -> u64 {
        self.last_activity_ms
    }

    /// Whether the idle auto-off timer has elapsed (0 seconds disables it).
    pub fn auto_off_due(&self, now_ms: u64) -> bool {
        if self.params.auto_off_secs == 0 {
            return false;
        }
        now_ms.saturating_sub(self.last_activity_ms)
            >= self.params.auto_off_secs.saturating_mul(1000)
    }

    /// Return to rest (used when entering/exiting free mode).
    pub fn reset(&mut self) {
        self.vel = Vec2::ZERO;
        self.wheel_vel = Vec2::ZERO;
        self.move_accum = Vec2::ZERO;
        self.wheel_accum = Vec2::ZERO;
        self.last_activity_ms = 0;
    }
}

fn scale_target(target: f64, multiplier: f64, boosts: u32, decel: bool) -> f64 {
    let mut target = target * multiplier.powi(boosts.min(8) as i32);
    if decel {
        target /= multiplier;
    }
    target
}

fn scale_vec(target: Vec2, multiplier: f64, boosts: u32, decel: bool) -> Vec2 {
    Vec2::new(
        scale_target(target.x, multiplier, boosts, decel),
        scale_target(target.y, multiplier, boosts, decel),
    )
}

/// Add `vel` to the accumulator, return whole units, and keep the remainder.
fn carry(accum: &mut Vec2, vel: Vec2) -> (i32, i32) {
    accum.x += vel.x;
    accum.y += vel.y;
    let dx = accum.x.trunc() as i32;
    let dy = accum.y.trunc() as i32;
    accum.x -= dx as f64;
    accum.y -= dy as f64;
    (dx, dy)
}

impl Vec2 {
    /// Scale both components.
    pub fn scale(&self, factor: f64) -> Self {
        Self::new(self.x * factor, self.y * factor)
    }
}

/// Result of one engine tick (whole units ready for the backend).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FreeModeFrame {
    /// Pixel delta to emit.
    pub move_delta: (i32, i32),
    /// Wheel detents to emit (x right, y up).
    pub wheel_delta: (i32, i32),
}

/// Combines held-key state with the physics model for one free-mode session.
#[derive(Debug, Clone, PartialEq)]
pub struct FreeModeSession {
    /// Held keys.
    pub keys: FreeModeKeys,
    /// Physics model.
    pub motion: FreeMode,
}

impl FreeModeSession {
    /// Create an idle engine.
    pub fn new(params: FreeModeParams, wheel: WheelParams) -> Self {
        Self {
            keys: FreeModeKeys::default(),
            motion: FreeMode::with_wheel(params, wheel),
        }
    }

    /// Record a key press and reset the idle timer.
    pub fn press(&mut self, key: FreeModeKey, now_ms: u64) {
        self.keys.press(key);
        self.motion.mark_activity(now_ms);
    }

    /// Record a key release (activity is counted on the next tick too).
    pub fn release(&mut self, key: FreeModeKey) {
        self.keys.release(key);
    }

    /// Advance one fixed step, marking activity while motion keys are held.
    pub fn tick(&mut self, now_ms: u64) -> FreeModeFrame {
        if self.keys.any_motion() {
            self.motion.mark_activity(now_ms);
        }
        FreeModeFrame {
            move_delta: self.motion.move_delta(&self.keys),
            wheel_delta: self.motion.wheel_delta(&self.keys),
        }
    }

    /// Whether the idle auto-off timer has elapsed.
    pub fn auto_off_due(&self, now_ms: u64) -> bool {
        self.motion.auto_off_due(now_ms)
    }

    /// Buttons still held (must be released on session teardown).
    pub fn held_buttons(&self) -> Vec<FreeModeKey> {
        self.keys.held_buttons()
    }

    /// Release all keys and stop motion.
    pub fn clear(&mut self) {
        self.keys.clear();
        self.motion.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn instant() -> FreeModeParams {
        FreeModeParams {
            easing: Easing::Instant,
            ..Default::default()
        }
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
        // converge monotonically toward the target at a fixed 8 ms step.
        for easing in [
            Easing::Loose,
            Easing::Natural,
            Easing::Snappy,
            Easing::Instant,
        ] {
            let mut fm = FreeMode::new(FreeModeParams {
                easing,
                ..Default::default()
            });
            let mut prev = 0.0;
            for _ in 0..200 {
                let v = fm.advance(Vec2::new(100.0, 0.0), 0, false).x;
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
        let mut fm = FreeMode::new(instant());
        let v = fm.advance(Vec2::new(10.0, 0.0), 2, false).x; // S+D held: ×2²
        assert!((v - 10.0 * 4.0 * 0.9).abs() < 1e-6);
        let mut fm = FreeMode::new(instant());
        let v = fm.advance(Vec2::new(10.0, 0.0), 0, true).x; // A held: ÷2
        assert!((v - 5.0 * 0.9).abs() < 1e-6);
    }

    #[test]
    fn movement_is_two_dimensional_and_diagonal_is_not_faster() {
        let mut keys = FreeModeKeys::default();
        keys.press(FreeModeKey::MoveDown);
        keys.press(FreeModeKey::MoveRight);
        assert_eq!(keys.movement_axis(), Vec2::new(1.0, 1.0));
        let mut fm = FreeMode::new(instant());
        let v = fm.advance(Vec2::new(10.0, 10.0), 0, false);
        assert!((v.x - 9.0).abs() < 1e-6);
        assert!((v.y - 9.0).abs() < 1e-6);
        // Each axis is scaled independently (no normalized diagonal).
        assert!((v.length() - 9.0 * 2.0f64.sqrt()).abs() < 1e-6);
    }

    #[test]
    fn wheel_axis_matches_scroll_sign_convention() {
        let mut keys = FreeModeKeys::default();
        keys.press(FreeModeKey::WheelUp);
        keys.press(FreeModeKey::WheelRight);
        assert_eq!(keys.wheel_axis(), Vec2::new(1.0, 1.0));
        keys.release(FreeModeKey::WheelUp);
        keys.press(FreeModeKey::WheelDown);
        assert_eq!(keys.wheel_axis(), Vec2::new(1.0, -1.0));
    }

    #[test]
    fn wheel_emits_whole_detents_and_keeps_remainder() {
        let mut fm = FreeMode::with_wheel(
            instant(),
            WheelParams {
                base_lines_per_tick: 1.0,
                easing: Easing::Instant,
                ..Default::default()
            },
        );
        let mut keys = FreeModeKeys::default();
        keys.press(FreeModeKey::WheelUp);
        let first = fm.wheel_delta(&keys);
        let second = fm.wheel_delta(&keys);
        assert_eq!(first, (0, 0)); // 0.9 lines: not a whole detent yet
        assert_eq!(second, (0, 1)); // 0.9 + 0.99 = 1.89 → 1 detent
    }

    #[test]
    fn auto_off_boundaries_and_disable() {
        let mut fm = FreeMode::new(FreeModeParams {
            auto_off_secs: 10,
            ..Default::default()
        });
        fm.mark_activity(1_000);
        assert!(!fm.auto_off_due(10_999));
        assert!(fm.auto_off_due(11_000));
        let fm = FreeMode::new(FreeModeParams {
            auto_off_secs: 0,
            ..Default::default()
        });
        assert!(!fm.auto_off_due(u64::MAX));
    }

    #[test]
    fn engine_marks_activity_while_moving_and_clears_buttons() {
        let mut engine = FreeModeSession::new(
            FreeModeParams {
                auto_off_secs: 1,
                ..Default::default()
            },
            WheelParams::default(),
        );
        engine.press(FreeModeKey::MoveRight, 0);
        engine.press(FreeModeKey::LeftButton, 0);
        let frame = engine.tick(500);
        assert!(frame.move_delta.0 > 0);
        assert!(!engine.auto_off_due(1_400));
        assert_eq!(engine.held_buttons(), vec![FreeModeKey::LeftButton]);
        engine.release(FreeModeKey::MoveRight);
        engine.release(FreeModeKey::LeftButton);
        engine.clear();
        assert!(engine.held_buttons().is_empty());
        assert_eq!(engine.motion.vel, Vec2::ZERO);
    }
}
