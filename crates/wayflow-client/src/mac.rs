//! Injection through Quartz event services.
//!
//! Requires Accessibility and Input Monitoring permission. macOS grants those to the
//! *responsible process*, which for a binary launched from a terminal is the terminal
//! itself, so granting Terminal once survives every rebuild instead of re-prompting for
//! each freshly compiled binary.

use core_graphics::display::CGDisplay;
use core_graphics::event::{
    CGEvent, CGEventFlags, CGEventTapLocation, CGEventType, CGMouseButton, ScrollEventUnit,
};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use core_graphics::geometry::CGPoint;
use core_graphics::sys;
use foreign_types::ForeignType;
use wayflow_keymap::ModifierState;
use wayflow_proto::Input;

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    /// Whether this process may post synthetic events.
    ///
    /// Worth calling explicitly because nothing else fails: `CGEventSource::new` succeeds
    /// without permission, the socket binds, connections are accepted, and every posted
    /// event is then discarded in silence. Launching over SSH does exactly that, which
    /// looks identical to a broken protocol until you check.
    fn AXIsProcessTrusted() -> u8;
}

unsafe extern "C" {
    /// The fixed-argument form of `CGEventCreateScrollWheelEvent`.
    ///
    /// The original is variadic, and on Apple ARM64 variadic arguments are passed on the
    /// stack while fixed ones go in registers. Declaring the variadic function with fixed
    /// parameters would therefore read the wrong locations and scroll by garbage, so the
    /// `2` variant is the only safe one to bind.
    fn CGEventCreateScrollWheelEvent2(
        source: sys::CGEventSourceRef,
        units: u32,
        wheel_count: u32,
        wheel1: i32,
        wheel2: i32,
        wheel3: i32,
    ) -> sys::CGEventRef;
}

/// evdev button codes.
const BTN_LEFT: u16 = 272;
const BTN_RIGHT: u16 = 273;
const BTN_MIDDLE: u16 = 274;

pub struct Injector {
    source: CGEventSource,
    /// Where we believe the cursor is, in global display coordinates.
    ///
    /// Tracked rather than queried: reading the real cursor before every motion would
    /// cost a round trip per event, and the position we just set is authoritative anyway.
    cursor: (f64, f64),
    bounds: Bounds,
    mods: ModifierState,
}

#[derive(Debug, Clone, Copy)]
struct Bounds {
    min_x: f64,
    min_y: f64,
    max_x: f64,
    max_y: f64,
}

impl Injector {
    /// # Errors
    /// Fails if Quartz refuses an event source, which in practice means the process has
    /// not been granted Accessibility permission.
    pub fn new() -> Result<Self, String> {
        // SAFETY: a plain predicate call into ApplicationServices with no arguments.
        if unsafe { AXIsProcessTrusted() } == 0 {
            return Err(concat!(
                "this process is not trusted for Accessibility, so every injected event ",
                "would be discarded without error.\n",
                "  Launch from a Terminal window on the Mac itself -- permission cannot ",
                "be granted to an SSH session, because the prompt needs a GUI login.\n",
                "  Grant Accessibility and Input Monitoring to Terminal, not to this ",
                "binary: the grant follows the responsible process, so it survives every ",
                "rebuild instead of re-prompting for each new binary."
            )
            .to_owned());
        }
        let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
            .map_err(|()| "cannot create a CGEventSource; grant Accessibility permission")?;
        let bounds = Bounds::from_displays();
        Ok(Self {
            source,
            cursor: (bounds.min_x, bounds.min_y),
            bounds,
            mods: ModifierState::new(),
        })
    }

    /// Place the cursor at the far edge, `ratio` of the way down the display.
    pub fn enter(&mut self, ratio: f64) {
        let height = self.bounds.max_y - self.bounds.min_y;
        // The host's edge is the client's opposite edge: leaving the left of the host
        // means arriving at the right of the client.
        self.cursor = (
            self.bounds.max_x - 1.0,
            self.bounds.min_y + height * ratio.clamp(0.0, 1.0),
        );
        self.warp();
    }

    /// Release every held modifier.
    ///
    /// Without this, a Shift still down when control returns to the host would never see
    /// its release event, and the Mac would behave as though Shift were held forever.
    pub fn leave(&mut self) {
        for evdev in [29u16, 42, 54, 56, 97, 100, 125, 126] {
            if let Some(key) = wayflow_keymap::evdev_to_mac(evdev) {
                self.post_key(key, false);
            }
        }
        self.mods.clear();
    }

    pub fn apply(&mut self, input: &Input) {
        match *input {
            Input::Motion { dx, dy } => {
                self.cursor.0 = (self.cursor.0 + dx).clamp(self.bounds.min_x, self.bounds.max_x);
                self.cursor.1 = (self.cursor.1 + dy).clamp(self.bounds.min_y, self.bounds.max_y);
                self.warp();
            }
            Input::Button { code, pressed } => self.post_button(code, pressed),
            Input::Scroll { dx, dy } => self.post_scroll(dx, dy),
            Input::Key { code, pressed } => {
                self.mods.update(code, pressed);
                if let Some(key) = wayflow_keymap::evdev_to_mac(code) {
                    self.post_key(key, pressed);
                }
            }
        }
    }

    fn warp(&self) {
        let point = CGPoint::new(self.cursor.0, self.cursor.1);
        if let Ok(event) = CGEvent::new_mouse_event(
            self.source.clone(),
            CGEventType::MouseMoved,
            point,
            CGMouseButton::Left,
        ) {
            event.post(CGEventTapLocation::HID);
        }
    }

    fn post_button(&self, code: u16, pressed: bool) {
        let (button, down, up) = match code {
            BTN_LEFT => (
                CGMouseButton::Left,
                CGEventType::LeftMouseDown,
                CGEventType::LeftMouseUp,
            ),
            BTN_RIGHT => (
                CGMouseButton::Right,
                CGEventType::RightMouseDown,
                CGEventType::RightMouseUp,
            ),
            BTN_MIDDLE => (
                CGMouseButton::Center,
                CGEventType::OtherMouseDown,
                CGEventType::OtherMouseUp,
            ),
            // Side buttons have no portable meaning; dropping beats guessing.
            _ => return,
        };
        let point = CGPoint::new(self.cursor.0, self.cursor.1);
        let kind = if pressed { down } else { up };
        if let Ok(event) = CGEvent::new_mouse_event(self.source.clone(), kind, point, button) {
            event.post(CGEventTapLocation::HID);
        }
    }

    fn post_scroll(&self, dx: f64, dy: f64) {
        #[allow(clippy::cast_possible_truncation)]
        let (y, x) = (dy.round() as i32, dx.round() as i32);
        if x == 0 && y == 0 {
            return;
        }
        // SAFETY: the source pointer is valid for the duration of the call, and
        // `from_ptr` takes ownership of a +1 reference exactly as CoreGraphics'
        // create-rule requires, so the wrapper releases it on drop.
        let event = unsafe {
            let raw = CGEventCreateScrollWheelEvent2(
                self.source.as_ptr(),
                ScrollEventUnit::LINE,
                2,
                y,
                x,
                0,
            );
            if raw.is_null() {
                return;
            }
            CGEvent::from_ptr(raw)
        };
        event.post(CGEventTapLocation::HID);
    }

    fn post_key(&self, mac_key: u16, pressed: bool) {
        let Ok(event) = CGEvent::new_keyboard_event(self.source.clone(), mac_key, pressed) else {
            return;
        };
        // Flags must be stamped explicitly. Quartz does not consult the real keyboard
        // state for synthesized events, so a shifted character needs the bit set here or
        // it arrives lowercase.
        event.set_flags(CGEventFlags::from_bits_truncate(self.mods.flags()));
        event.post(CGEventTapLocation::HID);
    }
}

/// Where the system thinks the cursor is, in global display coordinates.
#[must_use]
pub fn cursor_position() -> (f64, f64) {
    // A null-source event carries the current cursor location, which is the documented
    // way to read it without any permission.
    CGEvent::new(CGEventSource::new(CGEventSourceStateID::HIDSystemState).unwrap())
        .map(|e| {
            let p = e.location();
            (p.x, p.y)
        })
        .unwrap_or((-1.0, -1.0))
}

impl Bounds {
    /// Union of every active display.
    fn from_displays() -> Self {
        let mut bounds = Self {
            min_x: 0.0,
            min_y: 0.0,
            max_x: 1920.0,
            max_y: 1080.0,
        };
        let Ok(ids) = CGDisplay::active_displays() else {
            return bounds;
        };
        let mut first = true;
        for id in ids {
            let rect = CGDisplay::new(id).bounds();
            let (x0, y0) = (rect.origin.x, rect.origin.y);
            let (x1, y1) = (x0 + rect.size.width, y0 + rect.size.height);
            if first {
                bounds = Self {
                    min_x: x0,
                    min_y: y0,
                    max_x: x1,
                    max_y: y1,
                };
                first = false;
            } else {
                bounds.min_x = bounds.min_x.min(x0);
                bounds.min_y = bounds.min_y.min(y0);
                bounds.max_x = bounds.max_x.max(x1);
                bounds.max_y = bounds.max_y.max(y1);
            }
        }
        bounds
    }
}
