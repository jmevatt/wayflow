// Input capture on macOS and Windows via rdev (CGEventTap / SetWindowsHookEx).
//
// rdev::listen() blocks the calling thread and fires a callback for every input event.
// The callback is FnMut so we can track modifier state in a local variable.
//
// macOS note: CGEventTap requires the Accessibility permission (System Settings ->
// Privacy & Security -> Accessibility). The process is observed (not consumed), so
// events still reach the focused application; routing suppression happens because the
// client-side machine receives the events via Wayflow and re-injects them there.
//
// The `release_rx` parameter is received but not used here: rdev does not hold a
// compositor-level grab, so there is nothing to release.

use std::sync::Arc;

use super::{CaptureBackend, InputEvent};
use anyhow::Result;
use rdev::{listen, Button, EventType, Key};
use tokio::sync::mpsc;
use wayflow_core::keymap::rdev_keys;
use wayflow_proto::{Modifiers, MouseButton};

use crate::telemetry::Telemetry;

pub struct RdevCapture;

impl CaptureBackend for RdevCapture {
    fn start(
        self,
        tx: mpsc::Sender<InputEvent>,
        release_rx: mpsc::Receiver<()>,
        monitors_tx: tokio::sync::watch::Sender<Vec<wayflow_proto::ScreenInfo>>,
        _telemetry: Arc<Telemetry>,
    ) -> Result<()> {
        // release_rx and monitors_tx are not used: rdev doesn't hold a compositor grab
        // and doesn't have an OS API to query monitor layout here.
        drop(release_rx);
        drop(monitors_tx);

        let mut modifiers = Modifiers::default();

        listen(move |event| {
            let ev = match event.event_type {
                EventType::MouseMove { x, y } => {
                    Some(InputEvent::MouseMoveAbs { x, y })
                }
                EventType::ButtonPress(btn) => {
                    Some(InputEvent::MouseButton {
                        button: rdev_button_to_proto(btn),
                        pressed: true,
                    })
                }
                EventType::ButtonRelease(btn) => {
                    Some(InputEvent::MouseButton {
                        button: rdev_button_to_proto(btn),
                        pressed: false,
                    })
                }
                EventType::Wheel { delta_x, delta_y } => {
                    Some(InputEvent::Scroll {
                        dx: delta_x as f64,
                        dy: delta_y as f64,
                    })
                }
                EventType::KeyPress(key) => {
                    update_modifiers(&mut modifiers, key, true);
                    rdev_keys::rdev_to_hid(key).map(|hid| InputEvent::Key {
                        keycode: hid,
                        pressed: true,
                        modifiers,
                    })
                }
                EventType::KeyRelease(key) => {
                    let ev = rdev_keys::rdev_to_hid(key).map(|hid| InputEvent::Key {
                        keycode: hid,
                        pressed: false,
                        modifiers,
                    });
                    update_modifiers(&mut modifiers, key, false);
                    ev
                }
            };
            if let Some(input_event) = ev {
                if tx.blocking_send(input_event).is_err() {
                    // Channel closed: router exited, stop silently.
                }
            }
        })
        .map_err(|e| anyhow::anyhow!("rdev listen error: {e:?}"))
    }
}

pub fn backend() -> RdevCapture {
    RdevCapture
}

fn rdev_button_to_proto(button: Button) -> MouseButton {
    match button {
        Button::Left       => MouseButton::Left,
        Button::Right      => MouseButton::Right,
        Button::Middle     => MouseButton::Middle,
        Button::Unknown(4) => MouseButton::Back,
        Button::Unknown(5) => MouseButton::Forward,
        Button::Unknown(n) => MouseButton::Other(n),
    }
}

fn update_modifiers(m: &mut Modifiers, key: Key, pressed: bool) {
    match key {
        Key::ShiftLeft | Key::ShiftRight                 => m.shift = pressed,
        Key::ControlLeft | Key::ControlRight             => m.ctrl  = pressed,
        Key::Alt | Key::AltGr                            => m.alt   = pressed,
        Key::MetaLeft | Key::MetaRight                   => m.meta  = pressed,
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wayflow_proto::MouseButton;

    #[test]
    fn backend_fn_returns_capture() {
        let _b = backend();
    }

    #[test]
    fn rdev_button_all_known() {
        assert!(matches!(rdev_button_to_proto(Button::Left),       MouseButton::Left));
        assert!(matches!(rdev_button_to_proto(Button::Right),      MouseButton::Right));
        assert!(matches!(rdev_button_to_proto(Button::Middle),     MouseButton::Middle));
        assert!(matches!(rdev_button_to_proto(Button::Unknown(4)), MouseButton::Back));
        assert!(matches!(rdev_button_to_proto(Button::Unknown(5)), MouseButton::Forward));
        assert!(matches!(rdev_button_to_proto(Button::Unknown(9)), MouseButton::Other(9)));
    }

    #[test]
    fn update_modifiers_shift() {
        let mut m = Modifiers::default();
        update_modifiers(&mut m, Key::ShiftLeft, true);
        assert!(m.shift && !m.ctrl && !m.alt && !m.meta);
        update_modifiers(&mut m, Key::ShiftLeft, false);
        assert!(!m.shift);
    }

    #[test]
    fn update_modifiers_ctrl() {
        let mut m = Modifiers::default();
        update_modifiers(&mut m, Key::ControlRight, true);
        assert!(m.ctrl);
    }

    #[test]
    fn update_modifiers_alt() {
        let mut m = Modifiers::default();
        update_modifiers(&mut m, Key::Alt, true);
        assert!(m.alt);
        update_modifiers(&mut m, Key::AltGr, true);
        assert!(m.alt);
    }

    #[test]
    fn update_modifiers_meta() {
        let mut m = Modifiers::default();
        update_modifiers(&mut m, Key::MetaLeft, true);
        assert!(m.meta);
    }

    #[test]
    fn update_modifiers_non_modifier_key_no_change() {
        let mut m = Modifiers { shift: true, ..Modifiers::default() };
        update_modifiers(&mut m, Key::KeyA, true);
        assert!(m.shift); // unchanged
    }
}
