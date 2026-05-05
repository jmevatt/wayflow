// Input injection on Linux/Wayland via libei (EIS protocol) using the reis crate.
//
// Flow:
//   1. Connect to the compositor's EIS socket.
//      On most compositors this is exposed via the XDG RemoteDesktop portal;
//      the server-side portal session provides a file descriptor for this.
//   2. Create a reis::EiClient from the fd.
//   3. Send Seat / Interface negotiation (pointer + keyboard).
//   4. Inject events using reis::PointerInterface and reis::KeyboardInterface.
//
// For Wayland clipboard injection:
//   Use smithay-clipboard to set clipboard content on the target compositor.

use super::InjectBackend;
use anyhow::Result;
use wayflow_proto::{MouseButton, Modifiers};

pub struct LinuxWaylandInject {
    // TODO: reis::EiClient, pointer/keyboard interfaces
}

impl LinuxWaylandInject {
    pub fn new() -> Result<Self> {
        // TODO: connect to EIS socket via portal or direct fd
        tracing::warn!("linux_wayland inject backend not yet implemented");
        Ok(Self {})
    }
}

impl InjectBackend for LinuxWaylandInject {
    fn move_abs(&mut self, _x: u16, _y: u16) -> Result<()> {
        // TODO: reis pointer absolute motion
        Ok(())
    }

    fn mouse_button(&mut self, _button: MouseButton, _pressed: bool) -> Result<()> {
        // TODO: reis pointer button
        Ok(())
    }

    fn scroll(&mut self, _dx: i16, _dy: i16) -> Result<()> {
        // TODO: reis scroll
        Ok(())
    }

    fn key_event(&mut self, _keycode: u32, _pressed: bool, _modifiers: Modifiers) -> Result<()> {
        // TODO: reis keyboard key
        Ok(())
    }
}

pub fn backend() -> Result<LinuxWaylandInject> {
    LinuxWaylandInject::new()
}
