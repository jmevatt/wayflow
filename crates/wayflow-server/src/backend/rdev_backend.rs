// Input capture on macOS and Windows via rdev (CGEventTap / SetWindowsHookEx).
//
// TODO: implement. Rough flow:
//   - macOS:   rdev::listen() with CGEventTap -- requires Accessibility API permission.
//   - Windows: rdev::listen() with SetWindowsHookEx -- requires running on the UI thread.
//
// Both block the calling thread, so start() should spawn a dedicated thread and
// forward events through the mpsc sender.

use super::{CaptureBackend, InputEvent};
use anyhow::Result;
use tokio::sync::mpsc;

pub struct RdevCapture;

impl CaptureBackend for RdevCapture {
    fn start(self, _tx: mpsc::Sender<InputEvent>) -> Result<()> {
        anyhow::bail!("rdev capture backend not yet implemented")
    }

    fn release_grab(&self) -> Result<()> {
        Ok(())
    }

    fn acquire_grab(&self) -> Result<()> {
        Ok(())
    }
}

pub fn backend() -> RdevCapture {
    RdevCapture
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    #[test]
    fn backend_fn_returns_capture() {
        let _b = backend();
    }

    #[test]
    fn release_grab_returns_ok() {
        assert!(backend().release_grab().is_ok());
    }

    #[test]
    fn acquire_grab_returns_ok() {
        assert!(backend().acquire_grab().is_ok());
    }

    #[tokio::test]
    async fn start_returns_err_until_implemented() {
        let b = backend();
        let (tx, _rx) = mpsc::channel::<InputEvent>(1);
        assert!(b.start(tx).is_err());
    }
}
