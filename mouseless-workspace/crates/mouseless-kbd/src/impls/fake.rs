//! In-memory fake backend for deterministic tests (plan §8 #1–#3, #10).

use crate::{CellKey, KbdError, KeyEvent, KeyboardBackend, LayerEvent, Modifiers};
use std::collections::VecDeque;

/// Fake cell grab: scripted keys, no `/dev/input` access (daemon tests).
#[derive(Debug, Default)]
pub struct FakeCellGrab {
    held: bool,
    events: VecDeque<KeyEvent>,
}

impl FakeCellGrab {
    /// Create a held fake grab.
    pub fn new() -> Self {
        Self {
            held: true,
            events: VecDeque::new(),
        }
    }

    /// Queue a key press for the daemon session loop to poll.
    pub fn push_key(&mut self, key: CellKey) {
        self.push_event(KeyEvent {
            key,
            pressed: true,
            modifiers: Modifiers::default(),
        });
    }

    /// Queue a full press/release event with modifiers.
    pub fn push_event(&mut self, event: KeyEvent) {
        self.events.push_back(event);
    }
}

impl crate::CellKeySource for FakeCellGrab {
    fn next_key_press(&mut self) -> Result<Option<CellKey>, KbdError> {
        while let Some(event) = self.events.pop_front() {
            if event.pressed {
                return Ok(Some(event.key));
            }
        }
        Ok(None)
    }

    fn next_key_event(&mut self) -> Result<Option<KeyEvent>, KbdError> {
        Ok(self.events.pop_front())
    }

    fn held(&self) -> bool {
        self.held
    }
}

impl Drop for FakeCellGrab {
    fn drop(&mut self) {
        self.held = false;
    }
}

/// Fake keyboard: scripted events, inspectable layer commands.
#[derive(Debug, Default)]
pub struct FakeKeyboard {
    current: String,
    events: VecDeque<LayerEvent>,
    /// Layers requested via `set_layer` (test assertions).
    pub set_layer_calls: Vec<String>,
}

impl FakeKeyboard {
    /// Create on `initial_layer`.
    pub fn new(initial_layer: &str) -> Self {
        Self {
            current: initial_layer.into(),
            events: VecDeque::new(),
            set_layer_calls: vec![],
        }
    }

    /// Enqueue a scripted event.
    pub fn push_event(&mut self, ev: LayerEvent) {
        self.events.push_back(ev);
    }
}

impl KeyboardBackend for FakeKeyboard {
    type Error = KbdError;

    fn next_layer_event(&mut self) -> Result<LayerEvent, Self::Error> {
        self.events
            .pop_front()
            .ok_or_else(|| KbdError::Unavailable("no scripted events left".into()))
    }

    fn set_layer(&mut self, name: &str) -> Result<(), Self::Error> {
        self.current = name.into();
        self.set_layer_calls.push(name.into());
        Ok(())
    }

    fn current_layer(&mut self) -> Result<String, Self::Error> {
        Ok(self.current.clone())
    }

    fn reload(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}
