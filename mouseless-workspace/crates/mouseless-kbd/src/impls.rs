//! Concrete [`KeyboardBackend`](crate::KeyboardBackend) implementations.
//!
//! `kanata` (TCP + temp-grab) ships in Phase 1; `keyd` / `evdev_raw` are
//! future config-selected backends (plan §4.1). Test fakes live here so unit
//! tests never touch real devices or sockets.

mod fake;
mod kanata;

pub use fake::{FakeCellGrab, FakeKeyboard};
pub use kanata::KanataTcpConfig;
