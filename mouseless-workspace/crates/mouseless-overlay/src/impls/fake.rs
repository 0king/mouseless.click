//! In-memory fake overlay for deterministic tests.

use crate::{GridModel, OverlayBackend, OverlayError};
use mouseless_ipc::{CursorPos, MonitorLayout};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

/// Shared cursor cell: the fake mouse writes, the fake overlay reports.
pub type SharedCursor = Rc<RefCell<Option<CursorPos>>>;

/// Shared queue of one-shot cursor replies.
///
/// The fake mouse can push a pre-move position here to model a `QueryCursor`
/// that the extension answered before the compositor applied the motion; the
/// fake overlay consumes it before reporting the shared cursor.
pub type SharedCursorReplies = Rc<RefCell<VecDeque<CursorPos>>>;

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
    cursor: SharedCursor,
    layout: Option<MonitorLayout>,
    events: Vec<crate::OverlayEvent>,
    /// One-shot replies consumed by the next `query_cursor` calls before the
    /// shared cursor is used (models a reply answered before the compositor
    /// applied the last motion).
    queued_cursors: SharedCursorReplies,
}

impl Default for FakeOverlay {
    fn default() -> Self {
        Self {
            visible: false,
            last_model: None,
            connected: true,
            fail_show: false,
            cursor: Rc::new(RefCell::new(None)),
            layout: None,
            events: Vec::new(),
            queued_cursors: Rc::new(RefCell::new(VecDeque::new())),
        }
    }
}

impl FakeOverlay {
    /// Inject a cursor report (as the extension would send).
    pub fn set_cursor(&mut self, pos: CursorPos) {
        *self.cursor.borrow_mut() = Some(pos);
    }

    /// Clone the shared cursor cell so a fake mouse can move it.
    ///
    /// This models the compositor loop: `move_rel` changes the pointer, and
    /// the next `QueryCursor` reports the new position.
    pub fn cursor_handle(&self) -> SharedCursor {
        Rc::clone(&self.cursor)
    }

    /// Clone the shared one-shot reply queue.
    ///
    /// Lets a fake mouse model the compositor race by pushing the pre-move
    /// position immediately after `move_rel`, so the first `QueryCursor`
    /// returns a stale value.
    pub fn cursor_reply_handle(&self) -> SharedCursorReplies {
        Rc::clone(&self.queued_cursors)
    }

    /// Queue a one-shot `CursorReply`, consumed before the shared cursor.
    ///
    /// Use this to model a reply that the extension answered before the
    /// compositor applied the preceding `move_rel`.
    pub fn queue_cursor_reply(&mut self, pos: CursorPos) {
        self.queued_cursors.borrow_mut().push_back(pos);
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
        let queued = self.queued_cursors.borrow_mut().pop_front();
        let cursor = queued.or_else(|| *self.cursor.borrow());
        if let Some(cursor) = cursor {
            self.events
                .push(crate::OverlayEvent::CursorReply { cursor });
        }
        Ok(())
    }

    fn poll_events(&mut self) -> Vec<crate::OverlayEvent> {
        std::mem::take(&mut self.events)
    }

    fn last_cursor(&self) -> Option<CursorPos> {
        *self.cursor.borrow()
    }

    fn last_layout(&self) -> Option<MonitorLayout> {
        self.layout.clone()
    }

    fn is_connected(&self) -> bool {
        self.connected
    }
}
