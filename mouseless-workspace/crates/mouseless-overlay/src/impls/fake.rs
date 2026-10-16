//! In-memory fake overlay for deterministic tests.

use crate::{GridModel, OverlayBackend, OverlayError};
use mouseless_ipc::CursorPos;

/// Fake overlay: records visibility and cursor for assertions.
#[derive(Debug, Default)]
pub struct FakeOverlay {
    /// Whether the overlay is currently visible.
    pub visible: bool,
    /// Last shown model.
    pub last_model: Option<GridModel>,
    cursor: Option<CursorPos>,
}

impl FakeOverlay {
    /// Inject a cursor report (as the extension would send).
    pub fn set_cursor(&mut self, pos: CursorPos) {
        self.cursor = Some(pos);
    }
}

impl OverlayBackend for FakeOverlay {
    type Error = OverlayError;

    fn show(&mut self, model: &GridModel) -> Result<(), Self::Error> {
        self.visible = true;
        self.last_model = Some(model.clone());
        Ok(())
    }

    fn highlight(&mut self, index: u32) -> Result<(), Self::Error> {
        if let Some(m) = self.last_model.as_mut() {
            m.highlight = Some(index);
        }
        Ok(())
    }

    fn hide(&mut self) -> Result<(), Self::Error> {
        self.visible = false;
        Ok(())
    }

    fn last_cursor(&self) -> Option<CursorPos> {
        self.cursor
    }
}
