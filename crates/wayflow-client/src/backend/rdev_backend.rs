// Input injection on macOS and Windows via rdev (CGEventPost / SendInput).

use super::InjectBackend;
use anyhow::Result;
use rdev::{Button, EventType, simulate};
use wayflow_core::keymap::rdev_keys;
use wayflow_proto::{Modifiers, MouseButton};

pub struct RdevInject {
    left_down: bool,
    right_down: bool,
    /// macOS: union of all active displays' bounds at construction time.
    /// origin is the top-left of the union in CG global coords (can be
    /// negative if a display sits to the left of the main display).
    /// size is the full union extent. screen_size() reports size; move_abs
    /// translates incoming server-relative coords by adding origin.
    #[cfg(target_os = "macos")]
    display_origin: (f64, f64),
    #[cfg(target_os = "macos")]
    display_size: (u16, u16),
}

impl RdevInject {
    pub fn new() -> Result<Self> {
        #[cfg(target_os = "macos")]
        {
            let (origin, size) = mac_display_union();
            return Ok(Self {
                left_down: false,
                right_down: false,
                display_origin: origin,
                display_size: size,
            });
        }
        #[cfg(not(target_os = "macos"))]
        Ok(Self { left_down: false, right_down: false })
    }
}

/// Compute the union bounding rectangle of all active macOS displays.
/// Returns (origin (top-left in CG global coords), (width, height)).
#[cfg(target_os = "macos")]
fn mac_display_union() -> ((f64, f64), (u16, u16)) {
    use core_graphics::display::CGDisplay;

    let ids = CGDisplay::active_displays().unwrap_or_default();
    if ids.is_empty() {
        let b = CGDisplay::main().bounds();
        return (
            (b.origin.x, b.origin.y),
            (b.size.width as u16, b.size.height as u16),
        );
    }

    let mut min_x = f64::INFINITY;
    let mut min_y = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut max_y = f64::NEG_INFINITY;
    for id in &ids {
        let b = CGDisplay::new(*id).bounds();
        min_x = min_x.min(b.origin.x);
        min_y = min_y.min(b.origin.y);
        max_x = max_x.max(b.origin.x + b.size.width);
        max_y = max_y.max(b.origin.y + b.size.height);
    }
    let w = (max_x - min_x).max(1.0) as u16;
    let h = (max_y - min_y).max(1.0) as u16;
    tracing::info!(
        "mac displays: {} active, union origin=({}, {}) size={}x{}",
        ids.len(), min_x, min_y, w, h
    );
    ((min_x, min_y), (w, h))
}

fn sim(event: &EventType) -> Result<()> {
    // Don't propagate simulation errors -- rdev returns SimulateError for
    // unsupported events (e.g. Middle/Back/Forward buttons on macOS) and we
    // must not crash the client connection over an injected event that failed.
    if simulate(event).is_err() {
        tracing::warn!("rdev simulate failed (event may be unsupported on this platform)");
    }
    Ok(())
}

fn map_button(button: MouseButton) -> Button {
    match button {
        MouseButton::Left => Button::Left,
        MouseButton::Right => Button::Right,
        MouseButton::Middle => Button::Middle,
        MouseButton::Back => Button::Unknown(4),
        MouseButton::Forward => Button::Unknown(5),
        MouseButton::Other(n) => Button::Unknown(n),
    }
}

impl InjectBackend for RdevInject {
    fn move_abs(&mut self, x: u16, y: u16) -> Result<()> {
        // On macOS, the Window Server tracks window drags via kCGEventLeftMouseDragged,
        // not kCGEventMouseMoved. rdev always emits MouseMoved, so we bypass it when
        // a button is held and post the drag event type directly via core-graphics.
        #[cfg(target_os = "macos")]
        {
            // Server sends coords in client-relative space (0..union_w, 0..union_h).
            // Translate to CG global coords by adding the union origin.
            let gx = x as f64 + self.display_origin.0;
            let gy = y as f64 + self.display_origin.1;
            return move_abs_macos(gx, gy, self.left_down, self.right_down);
        }

        #[cfg(not(target_os = "macos"))]
        sim(&EventType::MouseMove { x: x as f64, y: y as f64 })
    }

    fn mouse_button(&mut self, button: MouseButton, pressed: bool) -> Result<()> {
        match button {
            MouseButton::Left  => self.left_down  = pressed,
            MouseButton::Right => self.right_down = pressed,
            _ => {}
        }
        let btn = map_button(button);
        if pressed {
            sim(&EventType::ButtonPress(btn))
        } else {
            sim(&EventType::ButtonRelease(btn))
        }
    }

    fn scroll(&mut self, dx: i16, dy: i16) -> Result<()> {
        #[cfg(target_os = "macos")]
        return scroll_macos(dx, dy);

        #[cfg(not(target_os = "macos"))]
        sim(&EventType::Wheel { delta_x: dx as i64, delta_y: dy as i64 })
    }

    fn screen_size(&self) -> (u16, u16) {
        #[cfg(target_os = "macos")]
        return self.display_size;

        #[cfg(not(target_os = "macos"))]
        (1920, 1080)
    }

    fn refresh_screen_size(&mut self) -> (u16, u16) {
        #[cfg(target_os = "macos")]
        {
            let (origin, size) = mac_display_union();
            self.display_origin = origin;
            self.display_size = size;
            return size;
        }
        #[cfg(not(target_os = "macos"))]
        (1920, 1080)
    }

    fn key_event(&mut self, keycode: u32, pressed: bool, _modifiers: Modifiers) -> Result<()> {
        let key = match rdev_keys::hid_to_rdev(keycode) {
            Some(k) => k,
            None => {
                tracing::warn!("unmapped HID keycode {:#x}, skipping", keycode);
                return Ok(());
            }
        };
        if pressed {
            sim(&EventType::KeyPress(key))
        } else {
            sim(&EventType::KeyRelease(key))
        }
    }
}

/// Post a scroll CGEvent using LINE units so macOS applies its native scroll acceleration.
/// rdev uses PIXEL units with our small values (~1), which produces imperceptible scroll.
/// Deskflow uses 3 lines/click with kCGScrollEventUnitLine -- matches native wheel feel.
#[cfg(target_os = "macos")]
fn scroll_macos(dx: i16, dy: i16) -> Result<()> {
    use core_graphics::event::{CGEvent, CGEventTapLocation, ScrollEventUnit};
    use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};

    const LINES_PER_CLICK: i32 = 3;
    let vy = dy as i32 * LINES_PER_CLICK;
    let vx = dx as i32 * LINES_PER_CLICK;
    if vx == 0 && vy == 0 {
        return Ok(());
    }
    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|_| anyhow::anyhow!("CGEventSource failed"))?;
    // wheel1 = vertical axis, wheel2 = horizontal axis
    let event = CGEvent::new_scroll_event(source, ScrollEventUnit::LINE, 2, vy, vx, 0)
        .map_err(|_| anyhow::anyhow!("CGEvent::new_scroll_event failed"))?;
    event.post(CGEventTapLocation::HID);
    Ok(())
}

/// Post a mouse move (or drag) CGEvent with the correct event type for the current button state.
#[cfg(target_os = "macos")]
fn move_abs_macos(x: f64, y: f64, left_down: bool, right_down: bool) -> Result<()> {
    use core_graphics::event::{CGEvent, CGEventTapLocation, CGEventType, CGMouseButton};
    use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
    use core_graphics::geometry::CGPoint;

    let point = CGPoint { x, y };
    let (etype, btn) = if left_down {
        (CGEventType::LeftMouseDragged, CGMouseButton::Left)
    } else if right_down {
        (CGEventType::RightMouseDragged, CGMouseButton::Right)
    } else {
        (CGEventType::MouseMoved, CGMouseButton::Left)
    };

    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|_| anyhow::anyhow!("CGEventSource failed"))?;
    let event = CGEvent::new_mouse_event(source, etype, point, btn)
        .map_err(|_| anyhow::anyhow!("CGEvent::new_mouse_event failed"))?;
    event.post(CGEventTapLocation::HID);
    Ok(())
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
    fn map_button_all_variants() {
        assert!(matches!(map_button(MouseButton::Left), Button::Left));
        assert!(matches!(map_button(MouseButton::Right), Button::Right));
        assert!(matches!(map_button(MouseButton::Middle), Button::Middle));
        assert!(matches!(map_button(MouseButton::Back), Button::Unknown(4)));
        assert!(matches!(map_button(MouseButton::Forward), Button::Unknown(5)));
        assert!(matches!(map_button(MouseButton::Other(9)), Button::Unknown(9)));
    }

    #[test]
    fn key_event_unmapped_hid_returns_ok() {
        // Unmapped HID codes should warn and return Ok rather than error.
        let mut b = backend().unwrap();
        assert!(b.key_event(0x00, true, Modifiers::default()).is_ok());
        assert!(b.key_event(0xA0, false, Modifiers::default()).is_ok());
        assert!(b.key_event(u32::MAX, true, Modifiers::default()).is_ok());
    }

    #[test]
    fn button_state_tracks_left_down() {
        let mut b = RdevInject::new().unwrap();
        b.left_down = true;
        assert!(b.left_down);
        b.left_down = false;
        assert!(!b.left_down);
    }

    // The tests below call rdev::simulate -- they require a display server
    // and may need Accessibility permission on macOS. They pass on standard
    // macOS CI runners where CGEventPost works without TCC restrictions.

    #[test]
    fn move_abs_returns_ok() {
        let mut b = backend().unwrap();
        assert!(b.move_abs(0, 0).is_ok());
        assert!(b.move_abs(1920, 1080).is_ok());
    }

    #[test]
    fn mouse_button_returns_ok() {
        let mut b = backend().unwrap();
        assert!(b.mouse_button(MouseButton::Left, true).is_ok());
        assert!(b.mouse_button(MouseButton::Left, false).is_ok());
    }

    #[test]
    fn scroll_returns_ok() {
        let mut b = backend().unwrap();
        assert!(b.scroll(0, 0).is_ok());
        assert!(b.scroll(3, -3).is_ok());
    }

    #[test]
    fn key_event_known_key_returns_ok() {
        let mut b = backend().unwrap();
        // HID 0x04 = KeyA -- must be in the map
        assert!(b.key_event(0x04, true, Modifiers::default()).is_ok());
        assert!(b.key_event(0x04, false, Modifiers::default()).is_ok());
    }
}
