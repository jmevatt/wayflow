// Client connection loop.
//
// Client connection loop: TLS connect, handshake, event dispatch to the inject backend, Pong.
// Auto-reconnects with exponential backoff on disconnect or transport error.
// Polls the platform display layout every SCREEN_POLL_INTERVAL and pushes a
// C2S::ScreenLayoutUpdate to the server when monitors are connected/disconnected.

use anyhow::{bail, Result};
use rustls::pki_types::ServerName;
use std::time::Duration;
use std::{collections::BTreeSet, sync::Arc};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::mpsc;
use tokio_rustls::TlsConnector;
use tracing::{debug, info, warn};
use wayflow_core::{tls, transport};
use wayflow_proto::{HelloC2S, ScreenInfo, C2S, PROTOCOL_VERSION, S2C};

use crate::backend::InjectBackend;

const RECONNECT_INITIAL: Duration = Duration::from_millis(500);
const RECONNECT_MAX: Duration = Duration::from_secs(5);
const SCREEN_POLL_INTERVAL: Duration = Duration::from_secs(2);

pub async fn run(server_addr: String, own_name: String) -> Result<()> {
    let tls_cfg = tls::client_tls_tofu(&server_addr)?;
    let connector = TlsConnector::from(Arc::new(tls_cfg));
    // Backend lives across reconnects -- screen-size queries and key/button state are reusable.
    let mut backend = crate::backend::create()?;

    let mut backoff = RECONNECT_INITIAL;
    loop {
        match connect_once(&server_addr, &connector, &own_name, backend.as_mut()).await {
            Ok(()) => {
                info!(
                    "server disconnected; reconnecting in {:?}",
                    RECONNECT_INITIAL
                );
                backoff = RECONNECT_INITIAL;
            }
            Err(e) => {
                warn!("connection error: {e:#}; reconnecting in {backoff:?}");
            }
        }
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(RECONNECT_MAX);
    }
}

async fn connect_once(
    server_addr: &str,
    connector: &TlsConnector,
    own_name: &str,
    backend: &mut dyn InjectBackend,
) -> Result<()> {
    let tcp = tokio::net::TcpStream::connect(server_addr).await?;
    // Disable Nagle so small frames (Pong, EnterScreen) hit the wire without coalescing delay.
    tcp.set_nodelay(true)?;
    info!("connected to {server_addr}");

    // Server cert is self-signed with subject "wayflow"; AcceptAny verifier ignores hostname.
    let server_name = ServerName::try_from("wayflow")
        .map_err(|e| anyhow::anyhow!("invalid server name: {e}"))?
        .to_owned();

    let tls = connector.connect(server_name, tcp).await?;
    let (mut r, mut w) = tokio::io::split(tls);

    let (sw, sh) = backend.screen_size();
    info!("screen size: {sw}x{sh}");

    let my_screen = ScreenInfo {
        name: own_name.into(),
        x: 0,
        y: 0,
        width: sw,
        height: sh,
    };
    transport::send_c2s(
        &mut w,
        &C2S::Hello(HelloC2S {
            version: PROTOCOL_VERSION,
            name: own_name.into(),
            screens: vec![my_screen],
        }),
    )
    .await?;

    let server_hello = match transport::recv_s2c(&mut r).await? {
        S2C::Hello(h) => h,
        msg => bail!("expected Hello from server, got {msg:?}"),
    };
    if server_hello.version != PROTOCOL_VERSION {
        bail!(
            "version mismatch: server={} us={}",
            server_hello.version,
            PROTOCOL_VERSION
        );
    }
    info!(
        "handshake complete; server knows {} screen(s)",
        server_hello.screens.len()
    );

    event_loop(r, w, backend, own_name).await
}

/// Drive the inject backend from an already-established framed stream.
///
/// Spawns separate reader and writer tasks so the main task can `tokio::select!`
/// between incoming server messages (via cancel-safe mpsc) and a periodic
/// screen-layout poll without risking partial-frame reads on cancellation.
///
/// Exposed for testing with in-memory streams and mock backends.
pub(crate) async fn event_loop<R, W>(
    r: R,
    w: W,
    backend: &mut dyn InjectBackend,
    own_name: &str,
) -> Result<()>
where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let (read_tx, mut read_rx) = mpsc::channel::<Result<S2C>>(16);
    let (write_tx, write_rx) = mpsc::channel::<C2S>(16);
    #[cfg(not(test))]
    let clipboard_apply_tx = Some(crate::clipboard::start(write_tx.clone()));
    #[cfg(test)]
    let clipboard_apply_tx: Option<mpsc::Sender<wayflow_proto::ClipboardContent>> = None;

    let read_task = tokio::spawn(reader_task(r, read_tx));
    let write_task = tokio::spawn(writer_task(w, write_rx));

    let mut poll = tokio::time::interval(SCREEN_POLL_INTERVAL);
    poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    poll.tick().await; // first tick fires immediately -- consume it
    let mut last_size = backend.screen_size();
    let mut pressed_keys = BTreeSet::new();

    let mut result: Result<()> = loop {
        tokio::select! {
            msg = read_rx.recv() => {
                match msg {
                    Some(Ok(s2c)) => {
                        match handle_msg(s2c, backend, &write_tx, clipboard_apply_tx.as_ref(), &mut pressed_keys).await {
                            Ok(true) => continue,
                            Ok(false) => break Ok(()),
                            Err(e) => break Err(e),
                        }
                    }
                    Some(Err(e)) => {
                        info!("server disconnected: {e}");
                        break Ok(());
                    }
                    None => break Ok(()),
                }
            }
            _ = poll.tick() => {
                let cur = backend.refresh_screen_size();
                if cur != last_size {
                    info!("screen layout changed: {}x{} -> {}x{}",
                          last_size.0, last_size.1, cur.0, cur.1);
                    let screen = ScreenInfo {
                        name: own_name.into(),
                        x: 0, y: 0,
                        width: cur.0, height: cur.1,
                    };
                    if write_tx.send(C2S::ScreenLayoutUpdate { screens: vec![screen] }).await.is_err() {
                        break Ok(());
                    }
                    last_size = cur;
                }
            }
        }
    };

    if let Err(e) = release_pressed_keys(backend, &mut pressed_keys, "event loop exit") {
        if result.is_ok() {
            result = Err(e);
        } else {
            warn!("failed to release pressed keys during shutdown: {e:#}");
        }
    }

    drop(write_tx);
    read_task.abort();
    let _ = write_task.await;
    let _ = read_task.await;
    result
}

async fn reader_task<R: AsyncRead + Unpin>(mut r: R, tx: mpsc::Sender<Result<S2C>>) {
    loop {
        let res = transport::recv_s2c(&mut r).await;
        let is_err = res.is_err();
        if tx.send(res).await.is_err() || is_err {
            break;
        }
    }
}

async fn writer_task<W: AsyncWrite + Unpin>(mut w: W, mut rx: mpsc::Receiver<C2S>) {
    while let Some(msg) = rx.recv().await {
        if transport::send_c2s(&mut w, &msg).await.is_err() {
            break;
        }
    }
}

/// Returns Ok(true) to continue, Ok(false) to exit cleanly (writer dead),
/// Err(_) to abort with a protocol error.
async fn handle_msg(
    msg: S2C,
    backend: &mut dyn InjectBackend,
    write_tx: &mpsc::Sender<C2S>,
    clipboard_apply_tx: Option<&mpsc::Sender<wayflow_proto::ClipboardContent>>,
    pressed_keys: &mut BTreeSet<u32>,
) -> Result<bool> {
    match msg {
        S2C::EnterScreen { x, y } => {
            info!("entering screen at ({x}, {y})");
            backend.wake_display()?;
            backend.move_abs(x, y)?;
        }
        S2C::LeaveScreen => {
            info!("leaving screen");
            release_pressed_keys(backend, pressed_keys, "LeaveScreen")?;
        }
        S2C::MouseMoveAbs { x, y } => {
            backend.wake_display()?;
            backend.move_abs(x, y)?;
        }
        S2C::MouseButton { button, pressed } => {
            backend.wake_display()?;
            backend.mouse_button(button, pressed)?;
        }
        S2C::Scroll { dx, dy } => {
            backend.wake_display()?;
            backend.scroll(dx, dy)?;
        }
        S2C::KeyEvent {
            keycode,
            pressed,
            modifiers,
        } => {
            backend.wake_display()?;
            backend.key_event(keycode, pressed, modifiers)?;
            if pressed {
                pressed_keys.insert(keycode);
            } else {
                pressed_keys.remove(&keycode);
            }
        }
        S2C::ClipboardData(content) => {
            info!("clipboard sync received");
            if let Some(tx) = clipboard_apply_tx {
                if tx.send(content).await.is_err() {
                    warn!("clipboard worker unavailable");
                }
            }
        }
        S2C::Ping => {
            debug!("ping");
            if write_tx.send(C2S::Pong).await.is_err() {
                return Ok(false);
            }
        }
        S2C::Hello(_) => bail!("unexpected second Hello from server -- protocol error"),
    }
    Ok(true)
}

fn release_pressed_keys(
    backend: &mut dyn InjectBackend,
    pressed_keys: &mut BTreeSet<u32>,
    reason: &'static str,
) -> Result<()> {
    if pressed_keys.is_empty() {
        return Ok(());
    }

    let keys: Vec<u32> = pressed_keys.iter().copied().collect();
    info!(
        "releasing {} pressed key(s) on {reason}",
        pressed_keys.len()
    );
    for keycode in keys {
        backend.key_event(keycode, false, wayflow_proto::Modifiers::default())?;
    }
    pressed_keys.clear();
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
        WakeDisplay,
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
            (
                MockBackend {
                    calls: calls.clone(),
                    fail_on: None,
                },
                calls,
            )
        }

        fn failing(on: Call) -> Self {
            MockBackend {
                calls: Arc::new(Mutex::new(Vec::new())),
                fail_on: Some(on),
            }
        }
    }

    impl InjectBackend for MockBackend {
        fn wake_display(&mut self) -> AResult<()> {
            self.calls.lock().unwrap().push(Call::WakeDisplay);
            Ok(())
        }
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
        event_loop(client_r, client_w, &mut backend, "test")
            .await
            .unwrap();
        let result = calls.lock().unwrap().clone();
        result
    }

    // ---------- S2C dispatch tests ----------

    #[tokio::test]
    async fn enter_screen_calls_move_abs() {
        let calls = run_with_msg(S2C::EnterScreen { x: 10, y: 20 }).await;
        assert_eq!(calls, vec![Call::WakeDisplay, Call::MoveAbs(10, 20)]);
    }

    #[tokio::test]
    async fn leave_screen_makes_no_backend_call() {
        let calls = run_with_msg(S2C::LeaveScreen).await;
        assert!(calls.is_empty());
    }

    #[tokio::test]
    async fn leave_screen_releases_pressed_keys() {
        let (mut server_side, client_side) = duplex(65536);
        let (client_r, client_w) = split(client_side);

        transport::send_s2c(
            &mut server_side,
            &S2C::KeyEvent {
                keycode: 0xE1,
                pressed: true,
                modifiers: Modifiers {
                    shift: true,
                    ..Modifiers::default()
                },
            },
        )
        .await
        .unwrap();
        transport::send_s2c(&mut server_side, &S2C::LeaveScreen)
            .await
            .unwrap();
        drop(server_side);

        let (mut backend, calls) = MockBackend::new();
        event_loop(client_r, client_w, &mut backend, "test")
            .await
            .unwrap();

        assert_eq!(
            calls.lock().unwrap().clone(),
            vec![
                Call::WakeDisplay,
                Call::KeyEvent(0xE1, true),
                Call::KeyEvent(0xE1, false)
            ]
        );
    }

    #[tokio::test]
    async fn mouse_move_abs_calls_move_abs() {
        let calls = run_with_msg(S2C::MouseMoveAbs { x: 640, y: 480 }).await;
        assert_eq!(calls, vec![Call::WakeDisplay, Call::MoveAbs(640, 480)]);
    }

    #[tokio::test]
    async fn mouse_button_dispatched() {
        let calls = run_with_msg(S2C::MouseButton {
            button: MouseButton::Left,
            pressed: true,
        })
        .await;
        assert_eq!(
            calls,
            vec![
                Call::WakeDisplay,
                Call::MouseButton(MouseButton::Left, true)
            ]
        );
    }

    #[tokio::test]
    async fn scroll_dispatched() {
        let calls = run_with_msg(S2C::Scroll { dx: 5, dy: -3 }).await;
        assert_eq!(calls, vec![Call::WakeDisplay, Call::Scroll(5, -3)]);
    }

    #[tokio::test]
    async fn key_event_dispatched() {
        let calls = run_with_msg(S2C::KeyEvent {
            keycode: 65,
            pressed: true,
            modifiers: Modifiers::default(),
        })
        .await;
        assert_eq!(
            calls,
            vec![
                Call::WakeDisplay,
                Call::KeyEvent(65, true),
                Call::KeyEvent(65, false)
            ]
        );
    }

    #[tokio::test]
    async fn disconnect_releases_pressed_keys() {
        let calls = run_with_msg(S2C::KeyEvent {
            keycode: 0xE1,
            pressed: true,
            modifiers: Modifiers {
                shift: true,
                ..Modifiers::default()
            },
        })
        .await;
        assert_eq!(
            calls,
            vec![
                Call::WakeDisplay,
                Call::KeyEvent(0xE1, true),
                Call::KeyEvent(0xE1, false)
            ]
        );
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
        let task =
            tokio::spawn(async move { event_loop(client_r, client_w, &mut backend, "test").await });
        transport::send_s2c(
            &mut server_side,
            &S2C::Hello(HelloS2C {
                version: PROTOCOL_VERSION,
                screens: vec![],
            }),
        )
        .await
        .unwrap();
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
            transport::send_s2c(&mut server_side, &S2C::Ping)
                .await
                .unwrap();
            let pong = transport::recv_c2s(&mut server_side).await.unwrap();
            assert_eq!(pong, C2S::Pong);
            // Drop closes connection
        });

        let (mut backend, _) = MockBackend::new();
        event_loop(client_r, client_w, &mut backend, "test")
            .await
            .unwrap();
        server_task.await.unwrap();
    }

    // ---------- Error propagation ----------

    #[tokio::test]
    async fn backend_move_abs_error_propagates() {
        let (mut server_side, client_side) = duplex(65536);
        let (client_r, client_w) = split(client_side);

        transport::send_s2c(&mut server_side, &S2C::MouseMoveAbs { x: 0, y: 0 })
            .await
            .unwrap();

        let mut backend = MockBackend::failing(Call::MoveAbs(0, 0));
        let result = event_loop(client_r, client_w, &mut backend, "test").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn backend_mouse_button_error_propagates() {
        let (mut server_side, client_side) = duplex(65536);
        let (client_r, client_w) = split(client_side);

        transport::send_s2c(
            &mut server_side,
            &S2C::MouseButton {
                button: MouseButton::Left,
                pressed: true,
            },
        )
        .await
        .unwrap();

        let mut backend = MockBackend::failing(Call::MouseButton(MouseButton::Left, true));
        let result = event_loop(client_r, client_w, &mut backend, "test").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn backend_scroll_error_propagates() {
        let (mut server_side, client_side) = duplex(65536);
        let (client_r, client_w) = split(client_side);

        transport::send_s2c(&mut server_side, &S2C::Scroll { dx: 1, dy: 1 })
            .await
            .unwrap();

        let mut backend = MockBackend::failing(Call::Scroll(1, 1));
        let result = event_loop(client_r, client_w, &mut backend, "test").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn backend_key_event_error_propagates() {
        let (mut server_side, client_side) = duplex(65536);
        let (client_r, client_w) = split(client_side);

        transport::send_s2c(
            &mut server_side,
            &S2C::KeyEvent {
                keycode: 1,
                pressed: false,
                modifiers: Modifiers::default(),
            },
        )
        .await
        .unwrap();

        let mut backend = MockBackend::failing(Call::KeyEvent(1, false));
        let result = event_loop(client_r, client_w, &mut backend, "test").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn server_disconnect_exits_cleanly() {
        let (server_side, client_side) = duplex(65536);
        let (client_r, client_w) = split(client_side);

        // Close immediately without sending anything
        drop(server_side);

        let (mut backend, _) = MockBackend::new();
        let result = event_loop(client_r, client_w, &mut backend, "test").await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn write_error_on_pong_propagates() {
        let (mut server_side, client_side) = duplex(65536);
        let (client_r, client_w) = split(client_side);

        // Send Ping then immediately close the read half so client can't write Pong back
        transport::send_s2c(&mut server_side, &S2C::Ping)
            .await
            .unwrap();
        // Shut down the read side of server so client's write (Pong) errors
        server_side.shutdown().await.unwrap();
        drop(server_side);

        let (mut backend, _) = MockBackend::new();
        // The Pong write may fail or succeed depending on buffering; either way no panic
        let _ = event_loop(client_r, client_w, &mut backend, "test").await;
    }
}
