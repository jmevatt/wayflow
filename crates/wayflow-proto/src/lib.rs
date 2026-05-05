// Wire types for the Wayflow protocol.
//
// All messages are postcard-serialized and framed with a 4-byte LE length prefix.
// This crate has zero I/O and zero platform dependencies -- keep it that way.

#![no_std]
extern crate alloc;

use alloc::{string::String, vec::Vec};
use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u16 = 1;

// ---------------------------------------------------------------------------
// Shared types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScreenInfo {
    pub name: String,
    pub width: u16,
    pub height: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ClipboardContent {
    Text(String),
    // TODO: image/bitmap support
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

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct Modifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub meta: bool,
}

// ---------------------------------------------------------------------------
// Server -> Client
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HelloS2C {
    pub version: u16,
    /// Screens the server knows about (its own + all registered clients).
    pub screens: Vec<ScreenInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum S2C {
    Hello(HelloS2C),
    /// Cursor is entering this client's screen at (x, y) in screen-local coords.
    EnterScreen { x: u16, y: u16 },
    /// Cursor has left this client's screen; client should stop injecting.
    LeaveScreen,
    /// Absolute cursor position within the client screen.
    MouseMoveAbs { x: u16, y: u16 },
    MouseButton { button: MouseButton, pressed: bool },
    /// Scroll delta in logical pixels.
    Scroll { dx: i16, dy: i16 },
    /// Raw keycode (evdev on Linux; platform-native elsewhere) + state.
    KeyEvent { keycode: u32, pressed: bool, modifiers: Modifiers },
    ClipboardData(ClipboardContent),
    Ping,
}

// ---------------------------------------------------------------------------
// Client -> Server
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HelloC2S {
    pub version: u16,
    pub name: String,
    pub screens: Vec<ScreenInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum C2S {
    Hello(HelloC2S),
    /// Client's clipboard changed; server should sync to other screens.
    ClipboardData(ClipboardContent),
    Pong,
}
