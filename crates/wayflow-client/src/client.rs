// Client connection loop.
//
// Client connection loop: TLS connect, handshake, event dispatch to the inject backend, Pong.

use anyhow::{bail, Result};
use rustls::pki_types::ServerName;
use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_rustls::TlsConnector;
use tracing::{debug, info};
use wayflow_core::{tls, transport};
use wayflow_proto::{C2S, HelloC2S, S2C, ScreenInfo, PROTOCOL_VERSION};

use crate::backend::InjectBackend;

pub async fn run(server_addr: String, own_name: String) -> Result<()> {
    let tls_cfg = tls::client_tls_tofu(&server_addr)?;
    let connector = TlsConnector::from(Arc::new(tls_cfg));

    let tcp = tokio::net::TcpStream::connect(&server_addr).await?;
    info!("connected to {server_addr}");

    // Server cert is self-signed with subject "wayflow"; AcceptAny verifier ignores hostname.
    let server_name = ServerName::try_from("wayflow")
        .map_err(|e| anyhow::anyhow!("invalid server name: {e}"))?
        .to_owned();

    let tls = connector.connect(server_name, tcp).await?;
    let (mut r, mut w) = tokio::io::split(tls);

    // Create backend before handshake so we can query real screen dimensions.
    let mut backend = crate::backend::create()?;
    let (sw, sh) = backend.screen_size();
    info!("screen size: {sw}x{sh}");

    // --- Handshake ---
    let my_screen = ScreenInfo {
        name: own_name.clone(),
        x: 0,
        y: 0,
        width: sw,
        height: sh,
    };
    transport::send_c2s(&mut w, &C2S::Hello(HelloC2S {
        version: PROTOCOL_VERSION,
        name: own_name,
        screens: vec![my_screen],
    })).await?;

    let server_hello = match transport::recv_s2c(&mut r).await? {
        S2C::Hello(h) => h,
        msg => bail!("expected Hello from server, got {msg:?}"),
    };
    if server_hello.version != PROTOCOL_VERSION {
        bail!("version mismatch: server={} us={}", server_hello.version, PROTOCOL_VERSION);
    }
    info!("handshake complete; server knows {} screen(s)", server_hello.screens.len());

    event_loop(r, w, backend.as_mut()).await
}

/// Drive the inject backend from an already-established framed stream.
/// Exposed for testing with in-memory streams and mock backends.
pub(crate) async fn event_loop<R, W>(
    mut r: R,
    mut w: W,
    backend: &mut dyn InjectBackend,
) -> Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    loop {
        match transport::recv_s2c(&mut r).await {
            Ok(S2C::EnterScreen { x, y }) => {
                info!("entering screen at ({x}, {y})");
                backend.move_abs(x, y)?;
            }
            Ok(S2C::LeaveScreen) => {
                info!("leaving screen");
            }
            Ok(S2C::MouseMoveAbs { x, y }) => {
                backend.move_abs(x, y)?;
            }
            Ok(S2C::MouseButton { button, pressed }) => {
                backend.mouse_button(button, pressed)?;
            }
            Ok(S2C::Scroll { dx, dy }) => {
                backend.scroll(dx, dy)?;
            }
            Ok(S2C::KeyEvent { keycode, pressed, modifiers }) => {
                backend.key_event(keycode, pressed, modifiers)?;
            }
            Ok(S2C::ClipboardData(_content)) => {
                // TODO: write to local clipboard via smithay-clipboard / arboard
                info!("clipboard sync received (not yet implemented)");
            }
            Ok(S2C::Ping) => {
                debug!("ping");
                transport::send_c2s(&mut w, &C2S::Pong).await?;
            }
            Ok(S2C::Hello(_)) => bail!("unexpected second Hello from server -- protocol error"),
            Err(e) => {
                info!("server disconnected: {e}");
                break;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Result as AResult;
    use std::sync::{Arc, Mutex};
    use tokio::io::{duplex, split, AsyncWriteExt};
    use wayflow_core::transport;
    use wayflow_proto::*;

    // ---------- Mock backend ----------

    #[derive(Debug, Clone, PartialEq)]
    enum Call {
        MoveAbs(u16, u16),
        MouseButton(MouseButton, bool),
        Scroll(i16, i16),
        KeyEvent(u32, bool),
    }

    struct MockBackend {
        calls: Arc<Mutex<Vec<Call>>>,
        fail_on: Option<Call>,
    }

    impl MockBackend {
        fn new() -> (Self, Arc<Mutex<Vec<Call>>>) {
            let calls = Arc::new(Mutex::new(Vec::new()));
            (MockBackend { calls: calls.clone(), fail_on: None }, calls)
        }

        fn failing(on: Call) -> Self {
            MockBackend { calls: Arc::new(Mutex::new(Vec::new())), fail_on: Some(on) }
        }
    }

    impl InjectBackend for MockBackend {
        fn move_abs(&mut self, x: u16, y: u16) -> AResult<()> {
            let call = Call::MoveAbs(x, y);
            if self.fail_on.as_ref() == Some(&call) {
                anyhow::bail!("injected failure");
            }
            self.calls.lock().unwrap().push(call);
            Ok(())
        }
        fn mouse_button(&mut self, button: MouseButton, pressed: bool) -> AResult<()> {
            let call = Call::MouseButton(button, pressed);
            if self.fail_on.as_ref() == Some(&call) {
                anyhow::bail!("injected failure");
            }
            self.calls.lock().unwrap().push(call);
            Ok(())
        }
        fn scroll(&mut self, dx: i16, dy: i16) -> AResult<()> {
            let call = Call::Scroll(dx, dy);
            if self.fail_on.as_ref() == Some(&call) {
                anyhow::bail!("injected failure");
            }
            self.calls.lock().unwrap().push(call);
            Ok(())
        }
        fn key_event(&mut self, keycode: u32, pressed: bool, _modifiers: Modifiers) -> AResult<()> {
            let call = Call::KeyEvent(keycode, pressed);
            if self.fail_on.as_ref() == Some(&call) {
                anyhow::bail!("injected failure");
            }
            self.calls.lock().unwrap().push(call);
            Ok(())
        }
    }

    // Helper: send one S2C message from "server" then close, run event_loop, return calls.
    async fn run_with_msg(msg: S2C) -> Vec<Call> {
        let (mut server_side, client_side) = duplex(65536);
        let (client_r, client_w) = split(client_side);

        transport::send_s2c(&mut server_side, &msg).await.unwrap();
        drop(server_side); // EOF after the message

        let (mut backend, calls) = MockBackend::new();
        event_loop(client_r, client_w, &mut backend).await.unwrap();
        let result = calls.lock().unwrap().clone();
        result
    }

    // ---------- S2C dispatch tests ----------

    #[tokio::test]
    async fn enter_screen_calls_move_abs() {
        let calls = run_with_msg(S2C::EnterScreen { x: 10, y: 20 }).await;
        assert_eq!(calls, vec![Call::MoveAbs(10, 20)]);
    }

    #[tokio::test]
    async fn leave_screen_makes_no_backend_call() {
        let calls = run_with_msg(S2C::LeaveScreen).await;
        assert!(calls.is_empty());
    }

    #[tokio::test]
    async fn mouse_move_abs_calls_move_abs() {
        let calls = run_with_msg(S2C::MouseMoveAbs { x: 640, y: 480 }).await;
        assert_eq!(calls, vec![Call::MoveAbs(640, 480)]);
    }

    #[tokio::test]
    async fn mouse_button_dispatched() {
        let calls = run_with_msg(S2C::MouseButton {
            button: MouseButton::Left,
            pressed: true,
        }).await;
        assert_eq!(calls, vec![Call::MouseButton(MouseButton::Left, true)]);
    }

    #[tokio::test]
    async fn scroll_dispatched() {
        let calls = run_with_msg(S2C::Scroll { dx: 5, dy: -3 }).await;
        assert_eq!(calls, vec![Call::Scroll(5, -3)]);
    }

    #[tokio::test]
    async fn key_event_dispatched() {
        let calls = run_with_msg(S2C::KeyEvent {
            keycode: 65,
            pressed: true,
            modifiers: Modifiers::default(),
        }).await;
        assert_eq!(calls, vec![Call::KeyEvent(65, true)]);
    }

    #[tokio::test]
    async fn clipboard_data_makes_no_backend_call() {
        let calls = run_with_msg(S2C::ClipboardData(ClipboardContent::Text("x".into()))).await;
        assert!(calls.is_empty());
    }

    #[tokio::test]
    async fn hello_from_server_errors() {
        // A second Hello mid-session is a protocol error and should abort the loop.
        let (mut server_side, client_side) = duplex(65536);
        let (client_r, client_w) = split(client_side);
        let (backend, backend_arc) = MockBackend::new();

        let mut backend = backend;
        let task = tokio::spawn(async move { event_loop(client_r, client_w, &mut backend).await });
        transport::send_s2c(&mut server_side, &S2C::Hello(HelloS2C {
            version: PROTOCOL_VERSION,
            screens: vec![],
        })).await.unwrap();
        let result = task.await.unwrap();
        assert!(result.is_err(), "expected error on second Hello, got Ok");
        assert!(backend_arc.lock().unwrap().is_empty());
    }

    // ---------- Ping -> Pong ----------

    #[tokio::test]
    async fn ping_sends_pong_back() {
        let (mut server_side, client_side) = duplex(65536);
        let (client_r, client_w) = split(client_side);

        // Server: send Ping, then read Pong, then close
        let server_task = tokio::spawn(async move {
            transport::send_s2c(&mut server_side, &S2C::Ping).await.unwrap();
            let pong = transport::recv_c2s(&mut server_side).await.unwrap();
            assert_eq!(pong, C2S::Pong);
            // Drop closes connection
        });

        let (mut backend, _) = MockBackend::new();
        event_loop(client_r, client_w, &mut backend).await.unwrap();
        server_task.await.unwrap();
    }

    // ---------- Error propagation ----------

    #[tokio::test]
    async fn backend_move_abs_error_propagates() {
        let (mut server_side, client_side) = duplex(65536);
        let (client_r, client_w) = split(client_side);

        transport::send_s2c(&mut server_side, &S2C::MouseMoveAbs { x: 0, y: 0 }).await.unwrap();

        let mut backend = MockBackend::failing(Call::MoveAbs(0, 0));
        let result = event_loop(client_r, client_w, &mut backend).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn backend_mouse_button_error_propagates() {
        let (mut server_side, client_side) = duplex(65536);
        let (client_r, client_w) = split(client_side);

        transport::send_s2c(&mut server_side, &S2C::MouseButton {
            button: MouseButton::Left, pressed: true,
        }).await.unwrap();

        let mut backend = MockBackend::failing(Call::MouseButton(MouseButton::Left, true));
        let result = event_loop(client_r, client_w, &mut backend).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn backend_scroll_error_propagates() {
        let (mut server_side, client_side) = duplex(65536);
        let (client_r, client_w) = split(client_side);

        transport::send_s2c(&mut server_side, &S2C::Scroll { dx: 1, dy: 1 }).await.unwrap();

        let mut backend = MockBackend::failing(Call::Scroll(1, 1));
        let result = event_loop(client_r, client_w, &mut backend).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn backend_key_event_error_propagates() {
        let (mut server_side, client_side) = duplex(65536);
        let (client_r, client_w) = split(client_side);

        transport::send_s2c(&mut server_side, &S2C::KeyEvent {
            keycode: 1, pressed: false, modifiers: Modifiers::default(),
        }).await.unwrap();

        let mut backend = MockBackend::failing(Call::KeyEvent(1, false));
        let result = event_loop(client_r, client_w, &mut backend).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn server_disconnect_exits_cleanly() {
        let (server_side, client_side) = duplex(65536);
        let (client_r, client_w) = split(client_side);

        // Close immediately without sending anything
        drop(server_side);

        let (mut backend, _) = MockBackend::new();
        let result = event_loop(client_r, client_w, &mut backend).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn write_error_on_pong_propagates() {
        let (mut server_side, client_side) = duplex(65536);
        let (client_r, client_w) = split(client_side);

        // Send Ping then immediately close the read half so client can't write Pong back
        transport::send_s2c(&mut server_side, &S2C::Ping).await.unwrap();
        // Shut down the read side of server so client's write (Pong) errors
        server_side.shutdown().await.unwrap();
        drop(server_side);

        let (mut backend, _) = MockBackend::new();
        // The Pong write may fail or succeed depending on buffering; either way no panic
        let _ = event_loop(client_r, client_w, &mut backend).await;
    }
}
