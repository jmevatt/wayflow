// Input capture backend trait + platform dispatch.
//
// The backend captures keyboard/mouse events from the local compositor/OS
// and sends them as InputEvent values over an async channel.

use anyhow::Result;
use tokio::sync::mpsc;
use wayflow_proto::{MouseButton, Modifiers};

#[derive(Debug, Clone)]
pub enum InputEvent {
    MouseMoveAbs { x: f64, y: f64 },
    MouseButton  { button: MouseButton, pressed: bool },
    Scroll       { dx: f64, dy: f64 },
    Key          { keycode: u32, pressed: bool, modifiers: Modifiers },
}

pub trait CaptureBackend: Send + 'static {
    fn start(self, tx: mpsc::Sender<InputEvent>) -> Result<()>;
    fn release_grab(&self) -> Result<()>;
    fn acquire_grab(&self) -> Result<()>;
}

// Platform dispatch -- each module exposes `pub fn backend() -> impl CaptureBackend`.

#[cfg(target_os = "linux")]
pub mod linux_wayland;

#[cfg(any(target_os = "macos", target_os = "windows"))]
pub mod rdev_backend;
