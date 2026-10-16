//! uinput mouse backend (Phase 1.2; plan §2.4).
//!
//! Creates `mouseless-mouse` (vendor [`VENDOR_ID_SONUSCAPE`]) with REL X/Y,
//! vertical/horizontal wheel, and all five buttons. The device MUST stay
//! covered by kanata `linux-dev-names-exclude` (loop prevention, plan §8 #6)
//! and by the flat-accel udev rule (audit G1).
//!
//! Creation needs `/dev/uinput` write access (`mouseless` group + udev rule,
//! plan §10.6). Without it [`UinputMouse::create`] fails and the daemon
//! degrades to "overlay unavailable, typing unaffected" (plan §6).

use crate::{MouseBackend, MouseButton, MouseError};
use uinput::event::controller::Mouse as UMouse;
use uinput::event::relative::{Position, Wheel};

/// Stable uinput device name (kanata-excluded).
pub const UINPUT_DEVICE_NAME: &str = "mouseless-mouse";

/// Sonuscape USB vendor id, matching upstream Mouseless.
pub const VENDOR_ID_SONUSCAPE: u16 = 0x736e;

/// Live uinput mouse. Destroys the kernel device on [`Drop`].
pub struct UinputMouse {
    dev: uinput::Device,
}

impl UinputMouse {
    /// Create the kernel device. Fails without `/dev/uinput` access.
    pub fn create() -> Result<Self, MouseError> {
        let mut dev = uinput::default()
            .map_err(|e| MouseError::Io(format!("open /dev/uinput: {e}")))?
            .name(UINPUT_DEVICE_NAME)
            .map_err(|e| MouseError::Io(e.to_string()))?
            .vendor(VENDOR_ID_SONUSCAPE)
            .event(uinput::event::Relative::Position(Position::X))
            .map_err(|e| MouseError::Io(e.to_string()))?
            .event(uinput::event::Relative::Position(Position::Y))
            .map_err(|e| MouseError::Io(e.to_string()))?
            .event(uinput::event::Relative::Wheel(Wheel::Vertical))
            .map_err(|e| MouseError::Io(e.to_string()))?
            .event(uinput::event::Relative::Wheel(Wheel::Horizontal))
            .map_err(|e| MouseError::Io(e.to_string()))?
            .event(uinput::event::Controller::Mouse(UMouse::Left))
            .map_err(|e| MouseError::Io(e.to_string()))?
            .event(uinput::event::Controller::Mouse(UMouse::Right))
            .map_err(|e| MouseError::Io(e.to_string()))?
            .event(uinput::event::Controller::Mouse(UMouse::Middle))
            .map_err(|e| MouseError::Io(e.to_string()))?
            .event(uinput::event::Controller::Mouse(UMouse::Back))
            .map_err(|e| MouseError::Io(e.to_string()))?
            .event(uinput::event::Controller::Mouse(UMouse::Forward))
            .map_err(|e| MouseError::Io(e.to_string()))?
            .create()
            .map_err(|e| MouseError::Io(format!("create device: {e}")))?;
        dev.synchronize()
            .map_err(|e| MouseError::Io(e.to_string()))?;
        Ok(Self { dev })
    }
}

fn to_uinput_button(button: MouseButton) -> UMouse {
    match button {
        MouseButton::Left => UMouse::Left,
        MouseButton::Right => UMouse::Right,
        MouseButton::Middle => UMouse::Middle,
        MouseButton::Back => UMouse::Back,
        MouseButton::Forward => UMouse::Forward,
    }
}

impl MouseBackend for UinputMouse {
    type Error = MouseError;

    fn move_rel(&mut self, dx: i32, dy: i32) -> Result<(), Self::Error> {
        self.dev
            .send(Position::X, dx)
            .and_then(|()| self.dev.send(Position::Y, dy))
            .and_then(|()| self.dev.synchronize())
            .map_err(|e| MouseError::Io(e.to_string()))
    }

    fn click(&mut self, button: MouseButton) -> Result<(), Self::Error> {
        // `Mouse` implements Kind+Code but not Press/Release, so emit the
        // down/up pair explicitly (equivalent to Device::click).
        let btn = to_uinput_button(button);
        self.dev
            .send(btn, 1)
            .and_then(|()| self.dev.synchronize())
            .and_then(|()| self.dev.send(btn, 0))
            .and_then(|()| self.dev.synchronize())
            .map_err(|e| MouseError::Io(e.to_string()))
    }

    fn scroll(&mut self, dx_lines: i32, dy_lines: i32) -> Result<(), Self::Error> {
        self.dev
            .send(Wheel::Horizontal, dx_lines)
            .and_then(|()| self.dev.send(Wheel::Vertical, dy_lines))
            .and_then(|()| self.dev.synchronize())
            .map_err(|e| MouseError::Io(e.to_string()))
    }
}
