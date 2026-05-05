// Input injection on macOS and Windows via rdev.
//
// TODO: implement. On macOS rdev uses CGEventPost; on Windows SendInput.
// Both are straightforward once the EIS-based Wayland path is validated.

use super::InjectBackend;
use anyhow::Result;
use wayflow_proto::{Modifiers, MouseButton};

pub struct RdevInject;

impl RdevInject {
    pub fn new() -> Result<Self> {
        tracing::warn!("rdev inject backend not yet implemented");
        Ok(Self {})
    }
}

impl InjectBackend for RdevInject {
    fn move_abs(&mut self, _x: u16, _y: u16) -> Result<()> {
        Ok(())
    }

    fn mouse_button(&mut self, _button: MouseButton, _pressed: bool) -> Result<()> {
        Ok(())
    }

    fn scroll(&mut self, _dx: i16, _dy: i16) -> Result<()> {
        Ok(())
    }

    fn key_event(&mut self, _keycode: u32, _pressed: bool, _modifiers: Modifiers) -> Result<()> {
        Ok(())
    }
}

pub fn backend() -> Result<RdevInject> {
    RdevInject::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use wayflow_proto::{Modifiers, MouseButton};

    #[test]
    fn new_returns_ok() {
        assert!(RdevInject::new().is_ok());
    }

    #[test]
    fn backend_fn_returns_ok() {
        assert!(backend().is_ok());
    }

    #[test]
    fn move_abs_returns_ok() {
        let mut b = backend().unwrap();
        assert!(b.move_abs(0, 0).is_ok());
        assert!(b.move_abs(1920, 1080).is_ok());
        assert!(b.move_abs(u16::MAX, u16::MAX).is_ok());
    }

    #[test]
    fn mouse_button_returns_ok() {
        let mut b = backend().unwrap();
        for btn in [
            MouseButton::Left,
            MouseButton::Right,
            MouseButton::Middle,
            MouseButton::Back,
            MouseButton::Forward,
            MouseButton::Other(9),
        ] {
            assert!(b.mouse_button(btn, true).is_ok());
            assert!(b.mouse_button(btn, false).is_ok());
        }
    }

    #[test]
    fn scroll_returns_ok() {
        let mut b = backend().unwrap();
        assert!(b.scroll(0, 0).is_ok());
        assert!(b.scroll(120, -120).is_ok());
        assert!(b.scroll(i16::MIN, i16::MAX).is_ok());
    }

    #[test]
    fn key_event_returns_ok() {
        let mut b = backend().unwrap();
        assert!(b.key_event(0, false, Modifiers::default()).is_ok());
        assert!(b.key_event(65, true, Modifiers { shift: true, ..Default::default() }).is_ok());
        assert!(b.key_event(u32::MAX, true, Modifiers { shift: true, ctrl: true, alt: true, meta: true }).is_ok());
    }
}
