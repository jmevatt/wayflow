// Input capture backend trait + platform dispatch.
//
// The backend captures keyboard/mouse events from the local compositor/OS
// and sends them as InputEvent values over an async channel.
//
// `release_rx` receives a signal from the router when the cursor returns to the
// server side, so the backend can release any compositor-level grab it holds
// (e.g. InputCapture portal on Wayland).

use anyhow::Result;
use tokio::sync::mpsc;
use wayflow_proto::{MouseButton, Modifiers};

#[derive(Debug, Clone, PartialEq)]
pub enum InputEvent {
    MouseMoveAbs { x: f64, y: f64 },
    MouseButton  { button: MouseButton, pressed: bool },
    Scroll       { dx: f64, dy: f64 },
    Key          { keycode: u32, pressed: bool, modifiers: Modifiers },
}

pub trait CaptureBackend: Send + 'static {
    /// Start capturing input on the current thread (blocks until capture ends or errors).
    /// Call from a dedicated `std::thread::spawn`.
    ///
    /// `release_rx` receives `()` each time the router returns cursor focus to the server.
    /// Backends that hold a compositor-level input grab should release it on this signal.
    fn start(self, tx: mpsc::Sender<InputEvent>, release_rx: mpsc::Receiver<()>) -> Result<()>;
}

// Platform dispatch -- each module exposes `pub fn backend() -> impl CaptureBackend`.

#[cfg(target_os = "linux")]
pub mod linux_wayland;

#[cfg(any(target_os = "macos", target_os = "windows"))]
pub mod rdev_backend;

/// Start the platform capture backend on the current thread.
/// Blocks until the backend exits or errors.
/// Call from a dedicated `std::thread::spawn`.
pub fn start_capture(tx: mpsc::Sender<InputEvent>, release_rx: mpsc::Receiver<()>) -> Result<()> {
    #[cfg(target_os = "linux")]
    return linux_wayland::backend().start(tx, release_rx);

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    return rdev_backend::backend().start(tx, release_rx);

    #[allow(unreachable_code)]
    { let _ = (tx, release_rx); Err(anyhow::anyhow!("no capture backend for this platform")) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_event_variants_are_debug_and_clone() {
        let events = [
            InputEvent::MouseMoveAbs { x: 1.0, y: 2.0 },
            InputEvent::MouseButton { button: MouseButton::Left, pressed: true },
            InputEvent::Scroll { dx: 0.5, dy: -1.5 },
            InputEvent::Key { keycode: 65, pressed: false, modifiers: Modifiers::default() },
        ];
        for e in &events {
            let cloned = e.clone();
            assert_eq!(*e, cloned);
            let _ = format!("{e:?}");
        }
    }
}
