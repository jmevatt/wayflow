// Input capture on Linux/Wayland via the XDG RemoteDesktop portal (ashpd).
//
// Flow:
//   1. Request a RemoteDesktop session via ashpd.
//   2. Call SelectDevices (pointer + keyboard).
//   3. Call Start -- compositor shows a permission prompt.
//   4. Read events from the EIS socket (libei / reis on the server side).
//
// Reference: https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.RemoteDesktop.html

use super::{CaptureBackend, InputEvent};
use anyhow::Result;
use tokio::sync::mpsc;

pub struct LinuxWaylandCapture;

impl CaptureBackend for LinuxWaylandCapture {
    fn start(self, _tx: mpsc::Sender<InputEvent>) -> Result<()> {
        // TODO:
        //   1. ashpd::desktop::remote_desktop::RemoteDesktop::new().await
        //   2. session.select_devices(DeviceType::Pointer | DeviceType::Keyboard).await
        //   3. session.start().await
        //   4. session.connect_to_eis().await -- returns an EIS fd
        //   5. Wrap fd with reis::EiClient, read InputEvent loop
        tracing::warn!("linux_wayland capture backend not yet implemented");
        Ok(())
    }

    fn release_grab(&self) -> Result<()> {
        Ok(())
    }

    fn acquire_grab(&self) -> Result<()> {
        Ok(())
    }
}

pub fn backend() -> LinuxWaylandCapture {
    LinuxWaylandCapture
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
        let b = backend();
        assert!(b.release_grab().is_ok());
    }

    #[test]
    fn acquire_grab_returns_ok() {
        let b = backend();
        assert!(b.acquire_grab().is_ok());
    }

    #[tokio::test]
    async fn start_returns_ok() {
        let b = backend();
        let (tx, _rx) = mpsc::channel::<InputEvent>(1);
        assert!(b.start(tx).is_ok());
    }
}
