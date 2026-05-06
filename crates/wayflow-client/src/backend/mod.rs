// Input injection backend trait + platform dispatch.

use anyhow::Result;
use wayflow_proto::{MouseButton, Modifiers};

pub trait InjectBackend: Send + 'static {
    fn move_abs(&mut self, x: u16, y: u16) -> Result<()>;
    fn mouse_button(&mut self, button: MouseButton, pressed: bool) -> Result<()>;
    fn scroll(&mut self, dx: i16, dy: i16) -> Result<()>;
    fn key_event(&mut self, keycode: u32, pressed: bool, modifiers: Modifiers) -> Result<()>;
    /// Return the cached union dimensions of the client's display layout.
    /// Used to report screen size to the server during handshake.
    fn screen_size(&self) -> (u16, u16) {
        (1920, 1080)
    }
    /// Re-query the platform for the current display layout, refresh any
    /// cached state (e.g. global display origin used for coordinate
    /// translation), and return the new dimensions. Called periodically by
    /// the client's event loop to detect monitor connect/disconnect.
    fn refresh_screen_size(&mut self) -> (u16, u16) {
        self.screen_size()
    }
}

#[cfg(target_os = "linux")]
pub mod linux_wayland;

#[cfg(any(target_os = "macos", target_os = "windows"))]
pub mod rdev_backend;

/// Create the platform-appropriate injection backend.
#[cfg(target_os = "linux")]
pub fn create() -> Result<Box<dyn InjectBackend>> {
    Ok(Box::new(linux_wayland::backend()?))
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
pub fn create() -> Result<Box<dyn InjectBackend>> {
    Ok(Box::new(rdev_backend::backend()?))
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
pub fn create() -> Result<Box<dyn InjectBackend>> {
    anyhow::bail!("no inject backend for this platform")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "linux")]
    #[test]
    fn create_returns_ok_on_linux() {
        assert!(create().is_ok());
    }

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    #[test]
    fn create_returns_ok_on_macos_windows() {
        assert!(create().is_ok());
    }
}
