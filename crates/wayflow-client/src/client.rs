// Client connection loop.
//
// Phase 1: TLS connect, handshake, event dispatch to the inject backend, Pong.
// Phase 2 (TODO): real screen dimensions from backend, clipboard read/write.

use anyhow::{bail, Result};
use rustls::pki_types::ServerName;
use std::sync::Arc;
use tokio_rustls::TlsConnector;
use tracing::{debug, info};
use wayflow_core::{config::Config, tls, transport};
use wayflow_proto::{C2S, HelloC2S, S2C, ScreenInfo, PROTOCOL_VERSION};

pub async fn run(config: Config, server_addr: String) -> Result<()> {
    let tls_cfg = tls::client_tls_insecure()?;
    let connector = TlsConnector::from(Arc::new(tls_cfg));

    let tcp = tokio::net::TcpStream::connect(&server_addr).await?;
    info!("connected to {server_addr}");

    // Server cert is self-signed with subject "wayflow"; AcceptAny verifier ignores hostname.
    let server_name = ServerName::try_from("wayflow")
        .map_err(|e| anyhow::anyhow!("invalid server name: {e}"))?
        .to_owned();

    let tls = connector.connect(server_name, tcp).await?;
    let (mut r, mut w) = tokio::io::split(tls);

    // --- Handshake ---
    // Placeholder screen dimensions -- phase 2 queries the inject backend for the real size.
    let my_screen = ScreenInfo {
        name: config.server.name.clone(),
        width: 1920,
        height: 1080,
    };
    transport::send_c2s(&mut w, &C2S::Hello(HelloC2S {
        version: PROTOCOL_VERSION,
        name: config.server.name.clone(),
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

    let mut backend = crate::backend::create()?;

    // --- Event loop ---
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
            Ok(S2C::Hello(_)) => {}
            Err(e) => {
                info!("server disconnected: {e}");
                break;
            }
        }
    }

    Ok(())
}
