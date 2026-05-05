// Input injection backend trait + platform dispatch.

use anyhow::Result;
use wayflow_proto::{MouseButton, Modifiers};

pub trait InjectBackend: Send + 'static {
    fn move_abs(&mut self, x: u16, y: u16) -> Result<()>;
    fn mouse_button(&mut self, button: MouseButton, pressed: bool) -> Result<()>;
    fn scroll(&mut self, dx: i16, dy: i16) -> Result<()>;
    fn key_event(&mut self, keycode: u32, pressed: bool, modifiers: Modifiers) -> Result<()>;
}

#[cfg(target_os = "linux")]
pub mod linux_wayland;

#[cfg(any(target_os = "macos", target_os = "windows"))]
pub mod rdev_backend;
