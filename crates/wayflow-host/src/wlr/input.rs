//! Forwarding input while captured.

use wayland_client::protocol::{wl_keyboard, wl_pointer};
use wayland_client::{Connection, Dispatch, QueueHandle, WEnum};
use wayland_protocols::wp::keyboard_shortcuts_inhibit::zv1::client::zwp_keyboard_shortcuts_inhibitor_v1 as ks_inhib;
use wayland_protocols::wp::pointer_constraints::zv1::client::zwp_locked_pointer_v1 as locked_ptr;
use wayland_protocols::wp::relative_pointer::zv1::client::zwp_relative_pointer_v1 as rel_ptr;
use wayflow_proto::Input;

use super::state::{Phase, State};

// `wl_keyboard.key` carries raw evdev codes, so nothing needs converting here. The
// familiar +8 offset belongs at the xkbcommon boundary, which wants keycodes above 8 for
// X11 compatibility, and applying it on the wire shifted every key by eight positions:
// KEY_A arrived as KEY_U.

/// Held alongside Escape to force release. The overlay takes exclusive keyboard focus, so
/// no compositor binding can reach us while captured; without an in-band escape a client
/// that stopped responding would leave the keyboard captured with no way out.
const KEY_ESC: u16 = 1;
const KEY_LEFTCTRL: u16 = 29;
const KEY_RIGHTCTRL: u16 = 97;

impl Dispatch<rel_ptr::ZwpRelativePointerV1, ()> for State {
    fn event(
        state: &mut Self,
        _proxy: &rel_ptr::ZwpRelativePointerV1,
        event: rel_ptr::Event,
        (): &(),
        _conn: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        // `dx_unaccel` rather than `dx`: the accelerated value is shaped for this
        // machine's pointer profile, and applying it to a different machine's screen
        // would make the cursor feel wrong there.
        let rel_ptr::Event::RelativeMotion {
            dx_unaccel,
            dy_unaccel,
            ..
        } = event
        else {
            return;
        };
        if state.phase != Phase::Active {
            return;
        }

        if let Some(active) = &mut state.active {
            active.travel_x += dx_unaccel;
            // Back at the edge we came from: hand control home. The sign depends on which
            // edge was crossed, since travelling "away" is negative on the left and
            // positive on the right.
            let returned = match state.config.edge {
                super::Edge::Left => active.travel_x >= 0.0,
                super::Edge::Right => active.travel_x <= 0.0,
                super::Edge::Top | super::Edge::Bottom => false,
            };
            if returned {
                state.end_capture(qh);
                return;
            }
        }

        state.sink.send_input(Input::Motion {
            dx: dx_unaccel,
            dy: dy_unaccel,
        });
    }
}

impl Dispatch<wl_keyboard::WlKeyboard, ()> for State {
    fn event(
        state: &mut Self,
        _proxy: &wl_keyboard::WlKeyboard,
        event: wl_keyboard::Event,
        (): &(),
        _conn: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        let wl_keyboard::Event::Key {
            key,
            state: key_state,
            ..
        } = event
        else {
            return;
        };
        if state.phase != Phase::Active {
            return;
        }
        let Ok(code) = u16::try_from(key) else {
            return;
        };
        let pressed = matches!(key_state, WEnum::Value(wl_keyboard::KeyState::Pressed));

        if let Some(active) = &mut state.active {
            if pressed {
                if !active.held_keys.contains(&code) {
                    active.held_keys.push(code);
                }
            } else {
                active.held_keys.retain(|&k| k != code);
            }

            let ctrl_held = active
                .held_keys
                .iter()
                .any(|&k| k == KEY_LEFTCTRL || k == KEY_RIGHTCTRL);
            if pressed && code == KEY_ESC && ctrl_held {
                eprintln!("wayflow-host: escape chord pressed, releasing");
                state.end_capture(qh);
                return;
            }
        }

        state.sink.send_input(Input::Key { code, pressed });
    }
}

impl State {
    /// Pointer events that only matter while captured.
    pub(crate) fn forward_pointer(&mut self, event: &wl_pointer::Event) {
        if self.phase != Phase::Active {
            return;
        }
        match *event {
            wl_pointer::Event::Button {
                button,
                state: btn_state,
                ..
            } => {
                let Ok(code) = u16::try_from(button) else {
                    return;
                };
                self.sink.send_input(Input::Button {
                    code,
                    pressed: matches!(
                        btn_state,
                        WEnum::Value(wl_pointer::ButtonState::Pressed)
                    ),
                });
            }
            wl_pointer::Event::Axis { axis, value, .. } => {
                // Wayland measures scroll in surface-local units where positive is down;
                // the wire format uses notches where positive is up, hence the negation
                // and the division by a typical line height.
                let notches = -value / 10.0;
                let (dx, dy) = match axis {
                    WEnum::Value(wl_pointer::Axis::VerticalScroll) => (0.0, notches),
                    WEnum::Value(wl_pointer::Axis::HorizontalScroll) => (notches, 0.0),
                    _ => return,
                };
                self.sink.send_input(Input::Scroll { dx, dy });
            }
            _ => {}
        }
    }
}

impl Dispatch<locked_ptr::ZwpLockedPointerV1, ()> for State {
    fn event(
        _state: &mut Self,
        _proxy: &locked_ptr::ZwpLockedPointerV1,
        _event: locked_ptr::Event,
        (): &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ks_inhib::ZwpKeyboardShortcutsInhibitorV1, ()> for State {
    fn event(
        _state: &mut Self,
        _proxy: &ks_inhib::ZwpKeyboardShortcutsInhibitorV1,
        _event: ks_inhib::Event,
        (): &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

wayland_client::delegate_noop!(State: ignore wayland_protocols::wp::pointer_constraints::zv1::client::zwp_pointer_constraints_v1::ZwpPointerConstraintsV1);
wayland_client::delegate_noop!(State: ignore wayland_protocols::wp::relative_pointer::zv1::client::zwp_relative_pointer_manager_v1::ZwpRelativePointerManagerV1);
wayland_client::delegate_noop!(State: ignore wayland_protocols::wp::keyboard_shortcuts_inhibit::zv1::client::zwp_keyboard_shortcuts_inhibit_manager_v1::ZwpKeyboardShortcutsInhibitManagerV1);
wayland_client::delegate_noop!(State: ignore wayland_protocols_wlr::virtual_pointer::v1::client::zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1);
wayland_client::delegate_noop!(State: ignore wayland_protocols_wlr::virtual_pointer::v1::client::zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1);
