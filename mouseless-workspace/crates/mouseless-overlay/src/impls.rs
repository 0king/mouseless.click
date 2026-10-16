//! Concrete [`OverlayBackend`](crate::OverlayBackend) implementations.
//!
//! `gnome` (Unix-socket JSON, plan §4) ships in Phase 1.3; `wlr` and `x11`
//! follow in Phase 4.

mod fake;
mod gnome;

pub use fake::FakeOverlay;
pub use gnome::{GNOME_SHELL_VERSIONS, SOCKET_PATH};
