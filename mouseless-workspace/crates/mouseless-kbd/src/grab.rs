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

/// A key read from the grabbed device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CellKey {
    /// Cell/free-mode character.
    ///
    /// The overlay needs the full QWERTY letter block
    /// (`qwertyuiop asdfghjkl; zxcvbnm,./`); free mode uses a subset
    /// (`i k j l s d f a r e q w m , . /`).
    Char(char),
    /// Execute at virtual cursor / left mouse button in free mode.
    Space,
    /// Exit overlay or free mode.
    Escape,
    /// Left Shift press/release (monitor switching, right-click modifier).
    ShiftLeft,
    /// Right Shift press/release (monitor switching).
    ShiftRight,
    /// Up arrow (subgrid nudge).
    ArrowUp,
    /// Down arrow (subgrid nudge).
    ArrowDown,
    /// Left arrow (subgrid nudge).
    ArrowLeft,
    /// Right arrow (subgrid nudge).
    ArrowRight,
}

/// Modifier state at the time of a key event.
///
/// Physical modifiers are already held on the seat; the daemon only needs
/// them to choose drag/right-click behavior and to avoid treating a Shift
/// used as a modifier as a monitor-switch tap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Modifiers {
    /// Shift held.
    pub shift: bool,
    /// Control held.
    pub ctrl: bool,
    /// Alt held.
    pub alt: bool,
    /// Super/Meta held.
    pub meta: bool,
}

impl Modifiers {
    /// Whether any modifier is held.
    pub fn any(&self) -> bool {
        self.shift || self.ctrl || self.alt || self.meta
    }
}

/// One press or release from the grabbed device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyEvent {
    /// Translated key.
    pub key: CellKey,
    /// `true` = press, `false` = release.
    pub pressed: bool,
    /// Modifier state after applying this event.
    pub modifiers: Modifiers,
}

/// Translate a kernel key code to a [`CellKey`]. `None` = not an overlay key
/// (modifiers, function keys, … — consumed silently while grabbed).
pub fn translate_key(code: evdev::KeyCode) -> Option<CellKey> {
    use evdev::KeyCode;
    Some(match code {
        // Overlay level-2 rows: the complete QWERTY letter block, so every
        // cell label is typeable while the overlay grab is held.
        KeyCode::KEY_Q => CellKey::Char('q'),
        KeyCode::KEY_W => CellKey::Char('w'),
        KeyCode::KEY_E => CellKey::Char('e'),
        KeyCode::KEY_R => CellKey::Char('r'),
        KeyCode::KEY_T => CellKey::Char('t'),
        KeyCode::KEY_Y => CellKey::Char('y'),
        KeyCode::KEY_U => CellKey::Char('u'),
        KeyCode::KEY_I => CellKey::Char('i'),
        KeyCode::KEY_O => CellKey::Char('o'),
        KeyCode::KEY_P => CellKey::Char('p'),
        KeyCode::KEY_A => CellKey::Char('a'),
        KeyCode::KEY_S => CellKey::Char('s'),
        KeyCode::KEY_D => CellKey::Char('d'),
        KeyCode::KEY_F => CellKey::Char('f'),
        KeyCode::KEY_G => CellKey::Char('g'),
        KeyCode::KEY_H => CellKey::Char('h'),
        KeyCode::KEY_J => CellKey::Char('j'),
        KeyCode::KEY_K => CellKey::Char('k'),
        KeyCode::KEY_L => CellKey::Char('l'),
        KeyCode::KEY_SEMICOLON => CellKey::Char(';'),
        KeyCode::KEY_Z => CellKey::Char('z'),
        KeyCode::KEY_X => CellKey::Char('x'),
        KeyCode::KEY_C => CellKey::Char('c'),
        KeyCode::KEY_V => CellKey::Char('v'),
        KeyCode::KEY_B => CellKey::Char('b'),
        KeyCode::KEY_N => CellKey::Char('n'),
        KeyCode::KEY_M => CellKey::Char('m'),
        KeyCode::KEY_COMMA => CellKey::Char(','),
        KeyCode::KEY_DOT => CellKey::Char('.'),
        KeyCode::KEY_SLASH => CellKey::Char('/'),
        KeyCode::KEY_SPACE => CellKey::Space,
        KeyCode::KEY_ESC => CellKey::Escape,
        KeyCode::KEY_LEFTSHIFT => CellKey::ShiftLeft,
        KeyCode::KEY_RIGHTSHIFT => CellKey::ShiftRight,
        KeyCode::KEY_UP => CellKey::ArrowUp,
        KeyCode::KEY_DOWN => CellKey::ArrowDown,
        KeyCode::KEY_LEFT => CellKey::ArrowLeft,
        KeyCode::KEY_RIGHT => CellKey::ArrowRight,
        _ => return None,
    })
}

fn update_modifiers(modifiers: &mut Modifiers, code: evdev::KeyCode, pressed: bool) {
    use evdev::KeyCode;
    match code {
        KeyCode::KEY_LEFTSHIFT | KeyCode::KEY_RIGHTSHIFT => modifiers.shift = pressed,
        KeyCode::KEY_LEFTCTRL | KeyCode::KEY_RIGHTCTRL => modifiers.ctrl = pressed,
        KeyCode::KEY_LEFTALT | KeyCode::KEY_RIGHTALT => modifiers.alt = pressed,
        KeyCode::KEY_LEFTMETA | KeyCode::KEY_RIGHTMETA => modifiers.meta = pressed,
        _ => {}
    }
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
    pending: std::collections::VecDeque<KeyEvent>,
    modifiers: Modifiers,
}

impl EvdevCellGrab {
    /// Open `name` fresh, verify no keys are held, and grab exclusively.
    pub fn acquire_named(name: &str) -> Result<Self, KbdError> {
        let path = find_output_device(name)?;
        let device = evdev::Device::open(&path)
            .map_err(|e| KbdError::Io(format!("open {}: {e}", path.display())))?;
        let mut grab = Self {
            device: Some(device),
            pending: std::collections::VecDeque::new(),
            modifiers: Modifiers::default(),
        };
        let device = grab
            .device
            .as_mut()
            .ok_or_else(|| KbdError::Unavailable("grab released".into()))?;
        // G6: never grab over a held key — that key's key-up would be lost and
        // the compositor would see a stuck key. Caller retries (EBUSY policy).
        let held = device
            .get_key_state()
            .map_err(|e| KbdError::Io(format!("EVIOCGKEY: {e}")))?;
        if held.iter().next().is_some() {
            return Err(KbdError::Busy);
        }
        device.grab().map_err(|_| KbdError::Busy)?;
        Ok(grab)
    }

    /// Acquire the standard kanata output.
    pub fn acquire() -> Result<Self, KbdError> {
        Self::acquire_named(KANATA_OUTPUT_NAME)
    }

    /// Drain one pending key event (non-blocking). Presses and releases are
    /// both returned; autorepeat (value 2) is ignored because the free-mode
    /// loop ticks continuously while a key is held.
    pub fn next_key_event(&mut self) -> Result<Option<KeyEvent>, KbdError> {
        if let Some(ev) = self.pending.pop_front() {
            return Ok(Some(ev));
        }
        let device = self
            .device
            .as_mut()
            .ok_or_else(|| KbdError::Unavailable("grab released".into()))?;
        let events = device
            .fetch_events()
            .map_err(|e| KbdError::Io(format!("fetch: {e}")))?;
        for ev in events {
            if let evdev::EventSummary::Key(_, code, value) = ev.destructure() {
                match value {
                    0 => {
                        update_modifiers(&mut self.modifiers, code, false);
                        if let Some(key) = translate_key(code) {
                            self.pending.push_back(KeyEvent {
                                key,
                                pressed: false,
                                modifiers: self.modifiers,
                            });
                        }
                    }
                    1 => {
                        update_modifiers(&mut self.modifiers, code, true);
                        if let Some(key) = translate_key(code) {
                            self.pending.push_back(KeyEvent {
                                key,
                                pressed: true,
                                modifiers: self.modifiers,
                            });
                        }
                    }
                    // Autorepeat: ignored (the daemon ticks while held).
                    _ => {}
                }
            }
        }
        Ok(self.pending.pop_front())
    }

    /// Legacy press-only poll. Releases are consumed and discarded.
    pub fn next_key_press(&mut self) -> Result<Option<CellKey>, KbdError> {
        while let Some(ev) = self.next_key_event()? {
            if ev.pressed {
                return Ok(Some(ev.key));
            }
        }
        Ok(None)
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

impl crate::CellKeySource for EvdevCellGrab {
    fn next_key_press(&mut self) -> Result<Option<CellKey>, KbdError> {
        self.next_key_press()
    }

    fn next_key_event(&mut self) -> Result<Option<KeyEvent>, KbdError> {
        self.next_key_event()
    }

    fn held(&self) -> bool {
        self.device.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translates_cell_keys() {
        use evdev::KeyCode;
        assert_eq!(translate_key(KeyCode::KEY_A), Some(CellKey::Char('a')));
        assert_eq!(
            translate_key(KeyCode::KEY_SEMICOLON),
            Some(CellKey::Char(';'))
        );
        // Every key in the level-2 QWERTY block must translate, or its cells
        // would be unreachable while the overlay grab is held.
        for (code, expected) in [
            (KeyCode::KEY_Q, 'q'),
            (KeyCode::KEY_W, 'w'),
            (KeyCode::KEY_E, 'e'),
            (KeyCode::KEY_R, 'r'),
            (KeyCode::KEY_T, 't'),
            (KeyCode::KEY_Y, 'y'),
            (KeyCode::KEY_U, 'u'),
            (KeyCode::KEY_I, 'i'),
            (KeyCode::KEY_O, 'o'),
            (KeyCode::KEY_P, 'p'),
            (KeyCode::KEY_A, 'a'),
            (KeyCode::KEY_S, 's'),
            (KeyCode::KEY_D, 'd'),
            (KeyCode::KEY_F, 'f'),
            (KeyCode::KEY_G, 'g'),
            (KeyCode::KEY_H, 'h'),
            (KeyCode::KEY_J, 'j'),
            (KeyCode::KEY_K, 'k'),
            (KeyCode::KEY_L, 'l'),
            (KeyCode::KEY_SEMICOLON, ';'),
            (KeyCode::KEY_Z, 'z'),
            (KeyCode::KEY_X, 'x'),
            (KeyCode::KEY_C, 'c'),
            (KeyCode::KEY_V, 'v'),
            (KeyCode::KEY_B, 'b'),
            (KeyCode::KEY_N, 'n'),
            (KeyCode::KEY_M, 'm'),
            (KeyCode::KEY_COMMA, ','),
            (KeyCode::KEY_DOT, '.'),
            (KeyCode::KEY_SLASH, '/'),
        ] {
            assert_eq!(translate_key(code), Some(CellKey::Char(expected)));
        }
        assert_eq!(translate_key(KeyCode::KEY_SPACE), Some(CellKey::Space));
        assert_eq!(translate_key(KeyCode::KEY_ESC), Some(CellKey::Escape));
        assert_eq!(translate_key(KeyCode::KEY_Q), Some(CellKey::Char('q')));
        assert_eq!(translate_key(KeyCode::KEY_1), None); // not an overlay key
        assert_eq!(
            translate_key(KeyCode::KEY_LEFTSHIFT),
            Some(CellKey::ShiftLeft)
        );
        assert_eq!(
            translate_key(KeyCode::KEY_RIGHTSHIFT),
            Some(CellKey::ShiftRight)
        );
        assert_eq!(translate_key(KeyCode::KEY_UP), Some(CellKey::ArrowUp));
        assert_eq!(translate_key(KeyCode::KEY_SLASH), Some(CellKey::Char('/')));
    }

    #[test]
    fn missing_device_is_degraded_not_fatal() {
        // Hermetic: a name that cannot exist degrades to Unavailable.
        let err = find_output_device("mouseless-test-no-such-device").unwrap_err();
        assert!(matches!(err, KbdError::Unavailable(_)));
    }
}
