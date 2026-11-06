//! In-memory fake mouse for deterministic tests.

use crate::{MouseBackend, MouseButton, MouseError};

/// Fake mouse: records events for assertions.
#[derive(Debug, Default)]
pub struct FakeMouse {
    /// Recorded `move_rel` calls.
    pub moves: Vec<(i32, i32)>,
    /// Recorded `click` calls.
    pub clicks: Vec<MouseButton>,
    /// Recorded `button_down` calls.
    pub button_downs: Vec<MouseButton>,
    /// Recorded `button_up` calls.
    pub button_ups: Vec<MouseButton>,
    /// Recorded `scroll` calls.
    pub scrolls: Vec<(i32, i32)>,
}

impl MouseBackend for FakeMouse {
    type Error = MouseError;

    fn move_rel(&mut self, dx: i32, dy: i32) -> Result<(), Self::Error> {
        self.moves.push((dx, dy));
        Ok(())
    }

    fn click(&mut self, button: MouseButton) -> Result<(), Self::Error> {
        self.clicks.push(button);
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
