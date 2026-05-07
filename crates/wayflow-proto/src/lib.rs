// Wire types for the Wayflow protocol.
//
// All messages are postcard-serialized and framed with a 4-byte LE length prefix.
// This crate has zero I/O and zero platform dependencies -- keep it that way.

#![no_std]
extern crate alloc;

use alloc::{string::String, vec::Vec};
use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u16 = 4;

// ---------------------------------------------------------------------------
// Shared types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreenInfo {
    pub name: String,
    /// Position of this monitor's top-left corner in the virtual desktop.
    pub x: i32,
    pub y: i32,
    pub width: u16,
    pub height: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClipboardContent {
    Text(String),
    Image(ClipboardImage),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClipboardImage {
    pub width: u32,
    pub height: u32,
    /// RGBA8 pixels, row-major, 4 bytes per pixel.
    pub rgba: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    Back,
    Forward,
    Other(u8),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Modifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub meta: bool,
}

// ---------------------------------------------------------------------------
// Server -> Client
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelloS2C {
    pub version: u16,
    /// Screens the server knows about (its own + all registered clients).
    pub screens: Vec<ScreenInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum S2C {
    Hello(HelloS2C),
    /// Cursor is entering this client's screen at (x, y) in screen-local coords.
    EnterScreen {
        x: u16,
        y: u16,
    },
    /// Cursor has left this client's screen; client should stop injecting.
    LeaveScreen,
    /// Absolute cursor position within the client screen.
    MouseMoveAbs {
        x: u16,
        y: u16,
    },
    MouseButton {
        button: MouseButton,
        pressed: bool,
    },
    /// Scroll delta in logical pixels.
    Scroll {
        dx: i16,
        dy: i16,
    },
    /// Raw keycode (evdev on Linux; platform-native elsewhere) + state.
    KeyEvent {
        keycode: u32,
        pressed: bool,
        modifiers: Modifiers,
    },
    ClipboardData(ClipboardContent),
    Ping,
}

// ---------------------------------------------------------------------------
// Client -> Server
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelloC2S {
    pub version: u16,
    pub name: String,
    pub screens: Vec<ScreenInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum C2S {
    Hello(HelloC2S),
    /// Client's clipboard changed; server should sync to other screens.
    ClipboardData(ClipboardContent),
    Pong,
    /// Client's display layout changed (monitor connect/disconnect, arrangement
    /// rearranged). Server should update its stored screen dims for clamp logic.
    ScreenLayoutUpdate {
        screens: Vec<ScreenInfo>,
    },
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use postcard::{from_bytes, to_allocvec};

    fn rt<T: serde::Serialize + serde::de::DeserializeOwned + PartialEq + core::fmt::Debug>(
        val: &T,
    ) {
        let bytes = to_allocvec(val).expect("serialize");
        let decoded: T = from_bytes(&bytes).expect("deserialize");
        assert_eq!(*val, decoded);
    }

    #[test]
    fn protocol_version_is_nonzero() {
        const _: () = assert!(PROTOCOL_VERSION > 0);
    }

    #[test]
    fn screen_info() {
        rt(&ScreenInfo {
            name: "monitor-1".into(),
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        });
        rt(&ScreenInfo {
            name: "".into(),
            x: 0,
            y: 0,
            width: 0,
            height: 0,
        });
    }

    #[test]
    fn screen_info_with_position() {
        rt(&ScreenInfo {
            name: "DP-2".into(),
            x: 2560,
            y: 0,
            width: 1920,
            height: 1080,
        });
        rt(&ScreenInfo {
            name: "below".into(),
            x: 0,
            y: 1440,
            width: 2560,
            height: 1440,
        });
    }

    #[test]
    fn screen_info_negative_position() {
        // Monitor to the left or above the origin is valid.
        rt(&ScreenInfo {
            name: "left".into(),
            x: -1920,
            y: 0,
            width: 1920,
            height: 1080,
        });
        rt(&ScreenInfo {
            name: "above".into(),
            x: 0,
            y: -1440,
            width: 2560,
            height: 1440,
        });
    }

    #[test]
    fn multi_monitor_hello() {
        // Dual monitor server: DP-1 at origin, DP-2 to the right.
        rt(&HelloS2C {
            version: PROTOCOL_VERSION,
            screens: alloc::vec![
                ScreenInfo {
                    name: "DP-1".into(),
                    x: 0,
                    y: 0,
                    width: 2560,
                    height: 1440
                },
                ScreenInfo {
                    name: "DP-2".into(),
                    x: 2560,
                    y: 0,
                    width: 1920,
                    height: 1080
                },
                ScreenInfo {
                    name: "mac".into(),
                    x: 0,
                    y: 0,
                    width: 2560,
                    height: 1600
                },
            ],
        });
    }

    #[test]
    fn modifiers_default() {
        rt(&Modifiers::default());
    }

    #[test]
    fn modifiers_all_true() {
        rt(&Modifiers {
            shift: true,
            ctrl: true,
            alt: true,
            meta: true,
        });
    }

    #[test]
    fn mouse_button_all_variants() {
        rt(&MouseButton::Left);
        rt(&MouseButton::Right);
        rt(&MouseButton::Middle);
        rt(&MouseButton::Back);
        rt(&MouseButton::Forward);
        rt(&MouseButton::Other(42));
        rt(&MouseButton::Other(0));
        rt(&MouseButton::Other(255));
    }

    #[test]
    fn clipboard_content_text() {
        rt(&ClipboardContent::Text("hello wayflow".into()));
        rt(&ClipboardContent::Text("".into()));
    }

    #[test]
    fn clipboard_content_image() {
        rt(&ClipboardContent::Image(ClipboardImage {
            width: 2,
            height: 1,
            rgba: alloc::vec![255, 0, 0, 255, 0, 0, 255, 255],
        }));
        rt(&ClipboardContent::Image(ClipboardImage {
            width: 0,
            height: 0,
            rgba: alloc::vec![],
        }));
    }

    #[test]
    fn hello_s2c() {
        rt(&HelloS2C {
            version: PROTOCOL_VERSION,
            screens: alloc::vec![
                ScreenInfo {
                    name: "server".into(),
                    x: 0,
                    y: 0,
                    width: 2560,
                    height: 1440
                },
                ScreenInfo {
                    name: "client".into(),
                    x: 0,
                    y: 0,
                    width: 1920,
                    height: 1080
                },
            ],
        });
        rt(&HelloS2C {
            version: 0,
            screens: alloc::vec![],
        });
    }

    #[test]
    fn hello_c2s() {
        rt(&HelloC2S {
            version: PROTOCOL_VERSION,
            name: "helicon".into(),
            screens: alloc::vec![ScreenInfo {
                name: "helicon".into(),
                x: 0,
                y: 0,
                width: 3840,
                height: 2160
            }],
        });
        rt(&HelloC2S {
            version: 0,
            name: "".into(),
            screens: alloc::vec![],
        });
    }

    // S2C variants
    #[test]
    fn s2c_hello() {
        rt(&S2C::Hello(HelloS2C {
            version: 1,
            screens: alloc::vec![],
        }));
    }

    #[test]
    fn s2c_enter_screen() {
        rt(&S2C::EnterScreen { x: 0, y: 0 });
        rt(&S2C::EnterScreen { x: 1920, y: 1080 });
        rt(&S2C::EnterScreen {
            x: u16::MAX,
            y: u16::MAX,
        });
    }

    #[test]
    fn s2c_leave_screen() {
        rt(&S2C::LeaveScreen);
    }

    #[test]
    fn s2c_mouse_move_abs() {
        rt(&S2C::MouseMoveAbs { x: 100, y: 200 });
        rt(&S2C::MouseMoveAbs { x: 0, y: 0 });
        rt(&S2C::MouseMoveAbs {
            x: u16::MAX,
            y: u16::MAX,
        });
    }

    #[test]
    fn s2c_mouse_button() {
        rt(&S2C::MouseButton {
            button: MouseButton::Left,
            pressed: true,
        });
        rt(&S2C::MouseButton {
            button: MouseButton::Right,
            pressed: false,
        });
        rt(&S2C::MouseButton {
            button: MouseButton::Other(7),
            pressed: true,
        });
    }

    #[test]
    fn s2c_scroll() {
        rt(&S2C::Scroll { dx: 0, dy: 0 });
        rt(&S2C::Scroll { dx: 120, dy: -120 });
        rt(&S2C::Scroll {
            dx: i16::MIN,
            dy: i16::MAX,
        });
    }

    #[test]
    fn s2c_key_event() {
        rt(&S2C::KeyEvent {
            keycode: 0,
            pressed: false,
            modifiers: Modifiers::default(),
        });
        rt(&S2C::KeyEvent {
            keycode: 65, // 'A'
            pressed: true,
            modifiers: Modifiers {
                shift: true,
                ..Modifiers::default()
            },
        });
        rt(&S2C::KeyEvent {
            keycode: u32::MAX,
            pressed: true,
            modifiers: Modifiers {
                shift: true,
                ctrl: true,
                alt: true,
                meta: true,
            },
        });
    }

    #[test]
    fn s2c_clipboard_data() {
        rt(&S2C::ClipboardData(ClipboardContent::Text(
            "paste me".into(),
        )));
        rt(&S2C::ClipboardData(ClipboardContent::Image(
            ClipboardImage {
                width: 1,
                height: 1,
                rgba: alloc::vec![1, 2, 3, 4],
            },
        )));
    }

    #[test]
    fn s2c_ping() {
        rt(&S2C::Ping);
    }

    // C2S variants
    #[test]
    fn c2s_hello() {
        rt(&C2S::Hello(HelloC2S {
            version: PROTOCOL_VERSION,
            name: "client-name".into(),
            screens: alloc::vec![],
        }));
    }

    #[test]
    fn c2s_clipboard_data() {
        rt(&C2S::ClipboardData(ClipboardContent::Text(
            "from client".into(),
        )));
        rt(&C2S::ClipboardData(ClipboardContent::Image(
            ClipboardImage {
                width: 1,
                height: 1,
                rgba: alloc::vec![4, 3, 2, 1],
            },
        )));
    }

    #[test]
    fn c2s_pong() {
        rt(&C2S::Pong);
    }

    #[test]
    fn c2s_screen_layout_update() {
        rt(&C2S::ScreenLayoutUpdate {
            screens: alloc::vec![ScreenInfo {
                name: "mac".into(),
                x: 0,
                y: 0,
                width: 4880,
                height: 1440
            },],
        });
        rt(&C2S::ScreenLayoutUpdate {
            screens: alloc::vec![ScreenInfo {
                name: "mac".into(),
                x: 0,
                y: 0,
                width: 1440,
                height: 900
            },],
        });
    }
}
