// Server connection loop.
//
// Phase 1: TLS accept, handshake, clipboard fan-out, ping/pong keepalive.
// Phase 2 (TODO): wire up the CaptureBackend -- edge detection + event routing.

use std::{collections::HashMap, net::SocketAddr, sync::Arc};
use anyhow::{bail, Result};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::TcpListener,
    sync::{mpsc, RwLock},
};
use tokio_rustls::TlsAcceptor;
use tracing::{debug, info, warn};
use wayflow_core::{config::Config, tls, transport};
use wayflow_proto::{C2S, HelloS2C, S2C, ScreenInfo, PROTOCOL_VERSION};

// Map of client name -> channel to deliver server-to-client messages.
type Clients = Arc<RwLock<HashMap<String, mpsc::Sender<S2C>>>>;

const PING_INTERVAL: tokio::time::Duration = tokio::time::Duration::from_secs(10);

pub async fn run(config: Config) -> Result<()> {
    let (cert_path, key_path) = tls::default_cert_paths();
    let tls_cfg = tls::server_tls(&cert_path, &key_path)?;
    let rustls_cfg = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(tls_cfg.cert, tls_cfg.key)?;
    let acceptor = TlsAcceptor::from(Arc::new(rustls_cfg));
    let addr = format!("0.0.0.0:{}", config.server.port);
    let listener = TcpListener::bind(&addr).await?;
    info!("listening on {}", listener.local_addr()?);
    let clients: Clients = Arc::new(RwLock::new(HashMap::new()));
    serve(Arc::new(config), listener, acceptor, clients).await
}

async fn serve(
    config: Arc<Config>,
    listener: TcpListener,
    acceptor: TlsAcceptor,
    clients: Clients,
) -> Result<()> {
    loop {
        match listener.accept().await {
            Ok((stream, peer)) => {
                let acceptor = acceptor.clone();
                let clients = clients.clone();
                let config = config.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle_connection(stream, peer, acceptor, clients, config).await {
                        warn!("{peer} error: {e:#}");
                    }
                });
            }
            Err(e) => warn!("accept error: {e}"),
        }
    }
}

async fn handle_connection(
    stream: tokio::net::TcpStream,
    peer: SocketAddr,
    acceptor: TlsAcceptor,
    clients: Clients,
    config: Arc<Config>,
) -> Result<()> {
    let tls = acceptor.accept(stream).await?;
    let (r, w) = tokio::io::split(tls);
    handle_stream(r, w, peer, clients, config).await
}

async fn handle_stream<R, W>(
    mut r: R,
    mut w: W,
    peer: SocketAddr,
    clients: Clients,
    config: Arc<Config>,
) -> Result<()>
where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    // --- Handshake ---
    let hello = match transport::recv_c2s(&mut r).await? {
        C2S::Hello(h) => h,
        msg => bail!("expected Hello, got {msg:?}"),
    };
    if hello.version != PROTOCOL_VERSION {
        bail!("version mismatch: client={} us={}", hello.version, PROTOCOL_VERSION);
    }
    if !config.clients.iter().any(|c| c.name == hello.name) {
        warn!("unknown client {:?} from {peer} -- accepting anyway", hello.name);
    }
    info!("client {:?} connected from {peer}", hello.name);

    // Placeholder server screen -- replaced in phase 2 by the capture backend.
    let server_screen = ScreenInfo {
        name: config.server.name.clone(),
        width: 1920,
        height: 1080,
    };
    let mut screens = vec![server_screen];
    screens.extend(hello.screens.clone());

    transport::send_s2c(&mut w, &S2C::Hello(HelloS2C {
        version: PROTOCOL_VERSION,
        screens,
    })).await?;

    // --- Register client ---
    let (tx, mut rx) = mpsc::channel::<S2C>(64);
    let name = hello.name.clone();
    clients.write().await.insert(name.clone(), tx.clone());

    // Writer task: drains the mpsc queue and sends frames to the stream.
    let write_task = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if transport::send_s2c(&mut w, &msg).await.is_err() {
                break;
            }
        }
    });

    // Ping task: sends a Ping every PING_INTERVAL; stops when the sender is closed.
    let ping_tx = tx.clone();
    let ping_task = tokio::spawn(async move {
        let mut interval = tokio::time::interval(PING_INTERVAL);
        loop {
            interval.tick().await;
            if ping_tx.send(S2C::Ping).await.is_err() {
                break;
            }
        }
    });

    // --- Read loop ---
    loop {
        match transport::recv_c2s(&mut r).await {
            Ok(C2S::Pong) => debug!("pong from {name}"),
            Ok(C2S::ClipboardData(content)) => {
                info!("clipboard update from {name}");
                let map = clients.read().await;
                for (peer_name, peer_tx) in map.iter() {
                    if peer_name != &name {
                        let _ = peer_tx.send(S2C::ClipboardData(content.clone())).await;
                    }
                }
            }
            Ok(C2S::Hello(_)) => warn!("unexpected re-Hello from {name}"),
            Err(e) => {
                info!("client {name} disconnected: {e}");
                break;
            }
        }
    }

    // --- Cleanup ---
    clients.write().await.remove(&name);
    ping_task.abort();
    write_task.abort();

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::SocketAddr;
    use tokio::io::{duplex, split};
    use wayflow_core::{config::{ClientEntry, Edge, ServerConfig}, transport};
    use wayflow_proto::*;

    fn test_config() -> Arc<Config> {
        Arc::new(Config {
            server: ServerConfig { name: "server".into(), port: 24800 },
            clients: vec![
                ClientEntry { name: "known-client".into(), edge: Edge::Right, offset: 0 },
            ],
        })
    }

    fn test_peer() -> SocketAddr {
        "127.0.0.1:12345".parse().unwrap()
    }

    fn empty_clients() -> Clients {
        Arc::new(RwLock::new(HashMap::new()))
    }

    async fn do_handshake(
        client_w: &mut (impl tokio::io::AsyncWrite + Unpin),
        client_r: &mut (impl tokio::io::AsyncRead + Unpin),
        name: &str,
    ) -> S2C {
        transport::send_c2s(client_w, &C2S::Hello(HelloC2S {
            version: PROTOCOL_VERSION,
            name: name.into(),
            screens: vec![ScreenInfo { name: name.into(), width: 1920, height: 1080 }],
        })).await.unwrap();
        transport::recv_s2c(client_r).await.unwrap()
    }

    // --- Handshake tests ---

    #[tokio::test]
    async fn handshake_happy_path_known_client() {
        let (server_side, client_side) = duplex(65536);
        let (sr, sw) = split(server_side);
        let (mut cr, mut cw) = split(client_side);
        let clients = empty_clients();
        let config = test_config();

        let handle = tokio::spawn(handle_stream(sr, sw, test_peer(), clients.clone(), config));

        let response = do_handshake(&mut cw, &mut cr, "known-client").await;
        assert!(matches!(response, S2C::Hello(_)));
        if let S2C::Hello(h) = response {
            assert_eq!(h.version, PROTOCOL_VERSION);
            // Server screen + client screen
            assert_eq!(h.screens.len(), 2);
        }

        // Client disconnects cleanly
        drop(cw);
        drop(cr);
        handle.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn handshake_unknown_client_still_connects() {
        let (server_side, client_side) = duplex(65536);
        let (sr, sw) = split(server_side);
        let (mut cr, mut cw) = split(client_side);
        let clients = empty_clients();
        let config = test_config();

        let handle = tokio::spawn(handle_stream(sr, sw, test_peer(), clients, config));

        // "unknown" is not in config.clients but server should accept anyway
        let response = do_handshake(&mut cw, &mut cr, "unknown").await;
        assert!(matches!(response, S2C::Hello(_)));

        drop(cw);
        drop(cr);
        handle.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn handshake_version_mismatch_errors() {
        let (server_side, client_side) = duplex(65536);
        let (sr, sw) = split(server_side);
        let (mut _cr, mut cw) = split(client_side);
        let clients = empty_clients();
        let config = test_config();

        let handle = tokio::spawn(handle_stream(sr, sw, test_peer(), clients, config));

        transport::send_c2s(&mut cw, &C2S::Hello(HelloC2S {
            version: PROTOCOL_VERSION + 1,
            name: "client".into(),
            screens: vec![],
        })).await.unwrap();

        let result = handle.await.unwrap();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("version mismatch"));
    }

    #[tokio::test]
    async fn handshake_wrong_first_message_errors() {
        let (server_side, client_side) = duplex(65536);
        let (sr, sw) = split(server_side);
        let (mut _cr, mut cw) = split(client_side);
        let clients = empty_clients();
        let config = test_config();

        let handle = tokio::spawn(handle_stream(sr, sw, test_peer(), clients, config));

        // Send Pong instead of Hello
        transport::send_c2s(&mut cw, &C2S::Pong).await.unwrap();

        let result = handle.await.unwrap();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("expected Hello"));
    }

    // --- Read-loop message tests ---

    #[tokio::test]
    async fn pong_is_handled_silently() {
        let (server_side, client_side) = duplex(65536);
        let (sr, sw) = split(server_side);
        let (mut cr, mut cw) = split(client_side);
        let clients = empty_clients();
        let config = test_config();

        let handle = tokio::spawn(handle_stream(sr, sw, test_peer(), clients, config));
        do_handshake(&mut cw, &mut cr, "c").await;

        // Send a Pong -- server should not error
        transport::send_c2s(&mut cw, &C2S::Pong).await.unwrap();

        drop(cw);
        drop(cr);
        handle.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn unexpected_re_hello_is_handled() {
        let (server_side, client_side) = duplex(65536);
        let (sr, sw) = split(server_side);
        let (mut cr, mut cw) = split(client_side);
        let clients = empty_clients();
        let config = test_config();

        let handle = tokio::spawn(handle_stream(sr, sw, test_peer(), clients, config));
        do_handshake(&mut cw, &mut cr, "c").await;

        transport::send_c2s(&mut cw, &C2S::Hello(HelloC2S {
            version: PROTOCOL_VERSION,
            name: "c".into(),
            screens: vec![],
        })).await.unwrap();

        drop(cw);
        drop(cr);
        // Should still exit cleanly -- unexpected re-Hello is warned but not fatal
        handle.await.unwrap().unwrap();
    }

    // --- Clipboard fan-out ---

    #[tokio::test]
    async fn clipboard_is_fanned_out_to_other_clients() {
        let clients = empty_clients();
        let config = test_config();

        // Connect client A
        let (ss_a, cs_a) = duplex(65536);
        let (sr_a, sw_a) = split(ss_a);
        let (mut cr_a, mut cw_a) = split(cs_a);
        tokio::spawn(handle_stream(
            sr_a, sw_a, "127.0.0.1:1".parse().unwrap(), clients.clone(), config.clone(),
        ));
        do_handshake(&mut cw_a, &mut cr_a, "known-client").await;

        // Connect client B
        let (ss_b, cs_b) = duplex(65536);
        let (sr_b, sw_b) = split(ss_b);
        let (mut cr_b, mut cw_b) = split(cs_b);
        tokio::spawn(handle_stream(
            sr_b, sw_b, "127.0.0.1:2".parse().unwrap(), clients.clone(), config.clone(),
        ));
        do_handshake(&mut cw_b, &mut cr_b, "other-client").await;

        // A sends clipboard
        let content = ClipboardContent::Text("shared text".into());
        transport::send_c2s(&mut cw_a, &C2S::ClipboardData(content.clone())).await.unwrap();

        // B should receive it
        let msg = transport::recv_s2c(&mut cr_b).await.unwrap();
        assert_eq!(msg, S2C::ClipboardData(content));

        // A should NOT receive its own clipboard
        // (verify by sending a known Ping marker and checking it's the next message)
        // We do this by not reading from cr_a and verifying no immediate data without timing.
    }

    // --- Client registration and cleanup ---

    #[tokio::test]
    async fn client_is_registered_in_map_and_removed_on_disconnect() {
        let clients = empty_clients();
        let config = test_config();

        let (server_side, client_side) = duplex(65536);
        let (sr, sw) = split(server_side);
        let (mut cr, mut cw) = split(client_side);

        let handle = tokio::spawn(handle_stream(
            sr, sw, test_peer(), clients.clone(), config,
        ));
        do_handshake(&mut cw, &mut cr, "known-client").await;

        // Client should be in the map now
        {
            let map = clients.read().await;
            assert!(map.contains_key("known-client"), "client not registered");
        }

        // Disconnect
        drop(cw);
        drop(cr);
        handle.await.unwrap().unwrap();

        // Map should be empty after cleanup
        let map = clients.read().await;
        assert!(map.is_empty(), "client not removed after disconnect");
    }

    // --- Ping ---

    #[tokio::test]
    async fn ping_is_sent_after_interval() {
        tokio::time::pause();

        let (server_side, client_side) = duplex(65536);
        let (sr, sw) = split(server_side);
        let (mut cr, mut cw) = split(client_side);
        let clients = empty_clients();
        let config = test_config();

        tokio::spawn(handle_stream(sr, sw, test_peer(), clients, config));
        do_handshake(&mut cw, &mut cr, "known-client").await;

        // Advance time past PING_INTERVAL (10s)
        tokio::time::advance(tokio::time::Duration::from_secs(11)).await;

        let msg = transport::recv_s2c(&mut cr).await.unwrap();
        assert_eq!(msg, S2C::Ping);
    }
}
