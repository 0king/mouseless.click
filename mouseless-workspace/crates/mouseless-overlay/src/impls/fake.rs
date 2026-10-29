//! In-memory fake overlay for deterministic tests.

use crate::{GridModel, OverlayBackend, OverlayError};
use mouseless_ipc::{CursorPos, MonitorLayout};

/// Fake overlay: records visibility and cursor for assertions.
#[derive(Debug)]
pub struct FakeOverlay {
    /// Whether the overlay is currently visible.
    pub visible: bool,
    /// Last shown model.
    pub last_model: Option<GridModel>,
    /// Whether a client is connected (defaults to true for tests).
    pub connected: bool,
    /// When set, `show` fails to exercise daemon rollback paths.
    pub fail_show: bool,
    cursor: Option<CursorPos>,
    layout: Option<MonitorLayout>,
    events: Vec<crate::OverlayEvent>,
}

impl Default for FakeOverlay {
    fn default() -> Self {
        Self {
            visible: false,
            last_model: None,
            connected: true,
            fail_show: false,
            cursor: None,
            layout: None,
            events: Vec::new(),
        }
    }
}

impl FakeOverlay {
    /// Inject a cursor report (as the extension would send).
    pub fn set_cursor(&mut self, pos: CursorPos) {
        self.cursor = Some(pos);
    }

    /// Inject a monitor layout (as the extension would send).
    pub fn set_layout(&mut self, layout: MonitorLayout) {
        self.layout = Some(layout);
    }

    /// Simulate extension disconnect (degraded-mode tests).
    pub fn disconnect(&mut self) {
        self.connected = false;
    }
}

impl OverlayBackend for FakeOverlay {
    type Error = OverlayError;

    fn show(&mut self, model: &GridModel) -> Result<(), Self::Error> {
        if self.fail_show {
            return Err(OverlayError::Connection("injected show failure".into()));
        }
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

    fn show_subgrid(&mut self, _parent: u32, _model: &GridModel) -> Result<(), Self::Error> {
        // Fake keeps the level-1 model; tests assert daemon geometry directly.
        Ok(())
    }

    fn hide(&mut self) -> Result<(), Self::Error> {
        self.visible = false;
        Ok(())
    }

    fn query_cursor(&mut self) -> Result<(), Self::Error> {
        if let Some(cursor) = self.cursor {
            self.events
                .push(crate::OverlayEvent::CursorReply { cursor });
        }
        Ok(())
    }

    fn poll_events(&mut self) -> Vec<crate::OverlayEvent> {
        std::mem::take(&mut self.events)
    }

    fn last_cursor(&self) -> Option<CursorPos> {
        self.cursor
    }

    fn last_layout(&self) -> Option<MonitorLayout> {
        self.layout.clone()
    }

    fn is_connected(&self) -> bool {
        self.connected
    }
}
