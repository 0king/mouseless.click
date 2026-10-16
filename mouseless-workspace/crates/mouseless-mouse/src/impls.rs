//! Concrete [`MouseBackend`](crate::MouseBackend) implementations.
//!
//! `uinput` ships in Phase 1.2; `libei`/portal follows in Phase 4 (plan §4.1).

mod fake;
mod uinput;

pub use fake::FakeMouse;
pub use uinput::{UINPUT_DEVICE_NAME, UinputMouse, VENDOR_ID_SONUSCAPE};
