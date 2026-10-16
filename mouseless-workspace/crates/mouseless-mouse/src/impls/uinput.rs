//! uinput mouse identity (Phase 1.2 creates the device; see plan §2.4).
//!
//! The device MUST be covered by kanata `linux-dev-names-exclude`
//! (loop prevention, plan §8 test #6) and by the flat-accel udev rule
//! (audit G1).

/// Stable uinput device name.
pub const UINPUT_DEVICE_NAME: &str = "mouseless-mouse";

/// Sonuscape USB vendor id, matching upstream Mouseless.
pub const VENDOR_ID_SONUSCAPE: u16 = 0x736e;
