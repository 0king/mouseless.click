//! Real evdev temp-grab (Phase 1.2; plan §5 rule 1, audits G6/G7).
//!
//! [`EvdevCellGrab`] owns its fd end to end:
//! 1. Discovers the stable `kanata-mouseless` output by device name via
//!    [`find_output_device`] — never an `eventN` hardcode (plan §5 rule 3).
//! 2. Opens the fd **fresh on each overlay invocation** and closes on [`Drop`]
//!    (audit G7 — a reused fd would replay stale typing / `SYN_DROPPED`).
//! 3. On acquire, snapshots `EVIOCGKEY` ([`evdev::Device::get_key_state`]); if
//!    any key is already down it returns [`KbdError::Busy`] so the caller
//!    retries instead of creating a stuck key (audit G6).
//! 4. [`Drop`] drains pending events, then `ungrab`s (best-effort) before the
//!    fd closes. The kernel also guarantees grab release on fd close, even on
//!    SIGKILL — defense in depth.

use crate::KbdError;

/// Stable kanata virtual-output name (must match `linux-output-device-name`).
pub const KANATA_OUTPUT_NAME: &str = "kanata-mouseless";

/// A key press read from the grabbed device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CellKey {
    /// Cell character (`a s d f j k l ;`).
    Char(char),
    /// Execute at virtual cursor.
    Space,
    /// Exit overlay.
    Escape,
}

/// Translate a kernel key code to a [`CellKey`]. `None` = not an overlay key
/// (modifiers, function keys, … — consumed silently while grabbed).
pub fn translate_key(code: evdev::KeyCode) -> Option<CellKey> {
    use evdev::KeyCode;
    Some(match code {
        KeyCode::KEY_A => CellKey::Char('a'),
        KeyCode::KEY_S => CellKey::Char('s'),
        KeyCode::KEY_D => CellKey::Char('d'),
        KeyCode::KEY_F => CellKey::Char('f'),
        KeyCode::KEY_J => CellKey::Char('j'),
        KeyCode::KEY_K => CellKey::Char('k'),
        KeyCode::KEY_L => CellKey::Char('l'),
        KeyCode::KEY_SEMICOLON => CellKey::Char(';'),
        KeyCode::KEY_SPACE => CellKey::Space,
        KeyCode::KEY_ESC => CellKey::Escape,
        _ => return None,
    })
}

/// Find the `/dev/input/eventN` path of the device named `name`.
pub fn find_output_device(name: &str) -> Result<std::path::PathBuf, KbdError> {
    for entry in evdev::enumerate() {
        let (path, device) = entry;
        if device.name().is_some_and(|n| n == name) {
            return Ok(path);
        }
    }
    Err(KbdError::Unavailable(format!(
        "device {name:?} not found (kanata running with linux-output-device-name?)"
    )))
}

/// RAII exclusive grab of kanata's virtual output, held only while the
/// overlay is visible. See module docs for the full contract.
pub struct EvdevCellGrab {
    device: Option<evdev::Device>,
}

impl EvdevCellGrab {
    /// Open `name` fresh, verify no keys are held, and grab exclusively.
    pub fn acquire_named(name: &str) -> Result<Self, KbdError> {
        let path = find_output_device(name)?;
        let device = evdev::Device::open(&path)
            .map_err(|e| KbdError::Io(format!("open {}: {e}", path.display())))?;
        let mut grab = Self {
            device: Some(device),
        };
        // G6: never grab over a held key — that key's key-up would be lost and
        // the compositor would see a stuck key. Caller retries (EBUSY policy).
        let held = grab
            .device
            .as_ref()
            .expect("present")
            .get_key_state()
            .map_err(|e| KbdError::Io(format!("EVIOCGKEY: {e}")))?;
        if held.iter().next().is_some() {
            return Err(KbdError::Busy);
        }
        grab.device
            .as_mut()
            .expect("present")
            .grab()
            .map_err(|_| KbdError::Busy)?;
        Ok(grab)
    }

    /// Acquire the standard kanata output.
    pub fn acquire() -> Result<Self, KbdError> {
        Self::acquire_named(KANATA_OUTPUT_NAME)
    }

    /// Drain one pending key-press event (non-blocking). Returns `None` when
    /// no complete press is pending. Releases (value 0) and repeats (value 2)
    /// are consumed silently; unknown keys map to `None` via [`translate_key`].
    pub fn next_key_press(&mut self) -> Result<Option<CellKey>, KbdError> {
        let device = self
            .device
            .as_mut()
            .ok_or_else(|| KbdError::Unavailable("grab released".into()))?;
        let mut out = None;
        for ev in device
            .fetch_events()
            .map_err(|e| KbdError::Io(format!("fetch: {e}")))?
        {
            if let evdev::EventSummary::Key(_, code, 1) = ev.destructure() {
                if out.is_none() {
                    out = translate_key(code);
                    // A press of an unknown key still counts as consumed; keep
                    // draining the batch but report None for it.
                    if translate_key(code).is_none() {
                        out = None;
                        break;
                    }
                }
            }
        }
        Ok(out)
    }
}

impl Drop for EvdevCellGrab {
    fn drop(&mut self) {
        if let Some(mut device) = self.device.take() {
            // Drain first so no partial press/release pair survives us (G6),
            // then ungrab best-effort; fd close releases regardless.
            let _ = device.fetch_events().map(|it| it.count());
            let _ = device.ungrab();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translates_cell_keys() {
        use evdev::KeyCode;
        assert_eq!(translate_key(KeyCode::KEY_A), Some(CellKey::Char('a')));
        assert_eq!(translate_key(KeyCode::KEY_SEMICOLON), Some(CellKey::Char(';')));
        assert_eq!(translate_key(KeyCode::KEY_SPACE), Some(CellKey::Space));
        assert_eq!(translate_key(KeyCode::KEY_ESC), Some(CellKey::Escape));
        assert_eq!(translate_key(KeyCode::KEY_Q), None); // not an overlay key
        assert_eq!(translate_key(KeyCode::KEY_LEFTSHIFT), None);
    }

    #[test]
    fn missing_device_is_degraded_not_fatal() {
        // Hermetic: a name that cannot exist degrades to Unavailable.
        let err = find_output_device("mouseless-test-no-such-device").unwrap_err();
        assert!(matches!(err, KbdError::Unavailable(_)));
    }
}
