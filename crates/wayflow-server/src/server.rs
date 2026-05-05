// Server connection loop + event routing.
//
// Two responsibilities:
//   1. handle_stream: TLS accept, handshake, clipboard fan-out, ping/pong.
//   2. route_events:  translate InputEvents from the capture backend into
//      S2C messages for the focused client, using edge detection.

use std::{collections::HashMap, net::SocketAddr, path::PathBuf, sync::Arc, time::SystemTime};
use anyhow::{bail, Result};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::TcpListener,
    sync::{mpsc, watch, RwLock},
};
use tokio_rustls::TlsAcceptor;
use tracing::{debug, info, warn};
use wayflow_core::{
    config::Config,
    layout::{ServerLayout, map_to_client},
    tls,
    transport,
};
use wayflow_proto::{C2S, HelloS2C, S2C, ScreenInfo, PROTOCOL_VERSION};

use crate::backend::InputEvent;

// Map of client name -> (primary screen, channel to deliver S2C messages).
type Clients = Arc<RwLock<HashMap<String, (ScreenInfo, mpsc::Sender<S2C>)>>>;

const PING_INTERVAL: tokio::time::Duration = tokio::time::Duration::from_secs(10);

pub async fn run(config: Config, config_path: PathBuf) -> Result<()> {
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

    // Placeholder server monitors -- phase 2 queries the capture backend for real layout.
    let server_monitors = vec![ScreenInfo {
        name: config.server.name.clone(),
        x: 0, y: 0, width: 1920, height: 1080,
    }];

    let (event_tx, event_rx) = mpsc::channel::<InputEvent>(256);
    // release_tx is signalled by route_events when cursor focus returns to the server.
    // The capture backend listens on release_rx and releases its compositor-level grab.
    let (release_tx, release_rx) = mpsc::channel::<()>(4);

    let (config_tx, config_rx) = watch::channel(Arc::new(config.clone()));
    tokio::spawn(watch_config(config_path, config_tx));

    let routing_clients = clients.clone();
    tokio::spawn(async move {
        if let Err(e) = route_events(event_rx, routing_clients, config_rx, server_monitors, release_tx).await {
            warn!("route_events exited: {e:#}");
        }
    });

    let event_tx_capture = event_tx.clone();
    std::thread::spawn(move || {
        if let Err(e) = crate::backend::start_capture(event_tx_capture, release_rx) {
            warn!("capture backend error: {e:#}");
        }
    });

    serve(Arc::new(config), listener, acceptor, clients).await
}

async fn route_events(
    mut rx: mpsc::Receiver<InputEvent>,
    clients: Clients,
    config_rx: watch::Receiver<Arc<Config>>,
    server_monitors: Vec<ScreenInfo>,
    release_tx: mpsc::Sender<()>,
) -> Result<()> {
    let layout = ServerLayout::new(server_monitors);
    let mut active_client: Option<String> = None;
    let mut server_cursor = (0i32, 0i32);
    let mut client_cursor = (0i32, 0i32);

    while let Some(event) = rx.recv().await {
        match event {
            InputEvent::MouseMoveAbs { x, y } => {
                let sx = x as i32;
                let sy = y as i32;

                if active_client.is_none() {
                    if let Some(edge) = layout.crossed_edge(sx, sy) {
                        // Clone the entry before any awaits so the watch borrow is dropped.
                        let entry = config_rx.borrow().clients.iter().find(|c| c.edge == edge).cloned();
                        match entry {
                            Some(entry) => {
                                let map = clients.read().await;
                                if let Some((screen, tx)) = map.get(&entry.name) {
                                    let (cx, cy) = map_to_client(sx, sy, screen, edge, entry.offset);
                                    let _ = tx.send(S2C::EnterScreen { x: cx, y: cy }).await;
                                    active_client = Some(entry.name.clone());
                                    client_cursor = (cx as i32, cy as i32);
                                    debug!("cursor -> {:?} at ({cx}, {cy})", entry.name);
                                } else {
                                    // Client configured but not connected -- release immediately.
                                    let _ = release_tx.try_send(());
                                }
                            }
                            None => {
                                // No client configured for this edge -- release immediately.
                                let _ = release_tx.try_send(());
                            }
                        }
                    }
                    server_cursor = (sx, sy);
                } else {
                    let name = active_client.as_ref().unwrap().clone();
                    let dx = sx - server_cursor.0;
                    let dy = sy - server_cursor.1;
                    server_cursor = (sx, sy);

                    let map = clients.read().await;
                    if let Some((screen, tx)) = map.get(&name) {
                        let new_cx = (client_cursor.0 + dx).clamp(0, screen.width as i32 - 1);
                        let new_cy = (client_cursor.1 + dy).clamp(0, screen.height as i32 - 1);
                        client_cursor = (new_cx, new_cy);

                        let at_edge = new_cx == 0
                            || new_cx == screen.width as i32 - 1
                            || new_cy == 0
                            || new_cy == screen.height as i32 - 1;

                        if at_edge {
                            let _ = tx.send(S2C::LeaveScreen).await;
                            drop(map);
                            active_client = None;
                            let _ = release_tx.try_send(());
                            debug!("cursor returned to server");
                        } else {
                            let _ = tx.send(S2C::MouseMoveAbs {
                                x: new_cx as u16,
                                y: new_cy as u16,
                            }).await;
                        }
                    } else {
                        active_client = None;
                    }
                }
            }

            InputEvent::MouseButton { button, pressed } => {
                if let Some(ref name) = active_client {
                    let map = clients.read().await;
                    if let Some((_, tx)) = map.get(name) {
                        let _ = tx.send(S2C::MouseButton { button, pressed }).await;
                    }
                }
            }

            InputEvent::Scroll { dx, dy } => {
                if let Some(ref name) = active_client {
                    let map = clients.read().await;
                    if let Some((_, tx)) = map.get(name) {
                        let _ = tx.send(S2C::Scroll { dx: dx as i16, dy: dy as i16 }).await;
                    }
                }
            }

            InputEvent::Key { keycode, pressed, modifiers } => {
                if let Some(ref name) = active_client {
                    let map = clients.read().await;
                    if let Some((_, tx)) = map.get(name) {
                        let _ = tx.send(S2C::KeyEvent { keycode, pressed, modifiers }).await;
                    }
                }
            }
        }
    }
    Ok(())
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

    let client_screen = hello.screens
        .first()
        .cloned()
        .unwrap_or_else(|| ScreenInfo {
            name: hello.name.clone(),
            x: 0, y: 0, width: 1920, height: 1080,
        });

    let server_screen = ScreenInfo {
        name: config.server.name.clone(),
        x: 0, y: 0, width: 1920, height: 1080,
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
    clients.write().await.insert(name.clone(), (client_screen, tx.clone()));

    let write_task = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if transport::send_s2c(&mut w, &msg).await.is_err() {
                break;
            }
        }
    });

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
                for (peer_name, (_, peer_tx)) in map.iter() {
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

async fn watch_config(path: PathBuf, tx: watch::Sender<Arc<Config>>) {
    let mut last_modified: Option<SystemTime> = std::fs::metadata(&path)
        .ok()
        .and_then(|m| m.modified().ok());

    loop {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        let Ok(meta) = std::fs::metadata(&path) else { continue };
        let Ok(modified) = meta.modified() else { continue };
        if last_modified.map(|lm| lm != modified).unwrap_or(true) {
            last_modified = Some(modified);
            match wayflow_core::config::Config::load(&path) {
                Ok(cfg) => {
                    info!("config reloaded: {} client(s) configured", cfg.clients.len());
                    let _ = tx.send(Arc::new(cfg));
                }
                Err(e) => warn!("config reload failed (keeping current): {e:#}"),
            }
        }
    }
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
            screens: vec![ScreenInfo { name: name.into(), x: 0, y: 0, width: 1920, height: 1080 }],
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
            assert_eq!(h.screens.len(), 2);
        }

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
        handle.await.unwrap().unwrap();
    }

    // --- Clipboard fan-out ---

    #[tokio::test]
    async fn clipboard_is_fanned_out_to_other_clients() {
        let clients = empty_clients();
        let config = test_config();

        let (ss_a, cs_a) = duplex(65536);
        let (sr_a, sw_a) = split(ss_a);
        let (mut cr_a, mut cw_a) = split(cs_a);
        tokio::spawn(handle_stream(
            sr_a, sw_a, "127.0.0.1:1".parse().unwrap(), clients.clone(), config.clone(),
        ));
        do_handshake(&mut cw_a, &mut cr_a, "known-client").await;

        let (ss_b, cs_b) = duplex(65536);
        let (sr_b, sw_b) = split(ss_b);
        let (mut cr_b, mut cw_b) = split(cs_b);
        tokio::spawn(handle_stream(
            sr_b, sw_b, "127.0.0.1:2".parse().unwrap(), clients.clone(), config.clone(),
        ));
        do_handshake(&mut cw_b, &mut cr_b, "other-client").await;

        let content = ClipboardContent::Text("shared text".into());
        transport::send_c2s(&mut cw_a, &C2S::ClipboardData(content.clone())).await.unwrap();

        let msg = transport::recv_s2c(&mut cr_b).await.unwrap();
        assert_eq!(msg, S2C::ClipboardData(content));
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

        {
            let map = clients.read().await;
            assert!(map.contains_key("known-client"), "client not registered");
        }

        drop(cw);
        drop(cr);
        handle.await.unwrap().unwrap();

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

        tokio::time::advance(tokio::time::Duration::from_secs(11)).await;

        let msg = transport::recv_s2c(&mut cr).await.unwrap();
        assert_eq!(msg, S2C::Ping);
    }

    // --- route_events ---

    fn routing_config() -> watch::Receiver<Arc<Config>> {
        watch::channel(Arc::new(Config {
            server: ServerConfig { name: "server".into(), port: 24800 },
            clients: vec![
                ClientEntry { name: "mac".into(), edge: Edge::Right, offset: 0 },
            ],
        })).1
    }

    fn server_monitors() -> Vec<ScreenInfo> {
        vec![ScreenInfo { name: "server".into(), x: 0, y: 0, width: 2560, height: 1440 }]
    }

    fn connected_clients() -> (Clients, mpsc::Receiver<S2C>) {
        let (tx, rx) = mpsc::channel(64);
        let screen = ScreenInfo { name: "mac".into(), x: 0, y: 0, width: 1920, height: 1080 };
        let mut map = HashMap::new();
        map.insert("mac".into(), (screen, tx));
        (Arc::new(RwLock::new(map)), rx)
    }

    fn drain(rx: &mut mpsc::Receiver<S2C>) -> Vec<S2C> {
        let mut out = Vec::new();
        while let Ok(m) = rx.try_recv() {
            out.push(m);
        }
        out
    }

    // Return a release_tx whose receiver is discarded (for tests that don't need release signals).
    fn dummy_release() -> mpsc::Sender<()> {
        let (tx, _rx) = mpsc::channel(4);
        tx
    }

    #[tokio::test]
    async fn edge_crossing_sends_enter_screen() {
        let (clients, mut client_rx) = connected_clients();
        let (tx, rx) = mpsc::channel(16);
        let task = tokio::spawn(route_events(rx, clients, routing_config(), server_monitors(), dummy_release()));

        // Right edge of 2560x1440 server at y=720
        tx.send(InputEvent::MouseMoveAbs { x: 2559.0, y: 720.0 }).await.unwrap();
        drop(tx);
        task.await.unwrap().unwrap();

        let msgs = drain(&mut client_rx);
        assert_eq!(msgs, vec![S2C::EnterScreen { x: 0, y: 720 }]);
    }

    #[tokio::test]
    async fn no_enter_when_cursor_not_at_edge() {
        let (clients, mut client_rx) = connected_clients();
        let (tx, rx) = mpsc::channel(16);
        let task = tokio::spawn(route_events(rx, clients, routing_config(), server_monitors(), dummy_release()));

        tx.send(InputEvent::MouseMoveAbs { x: 1000.0, y: 720.0 }).await.unwrap();
        drop(tx);
        task.await.unwrap().unwrap();

        assert!(drain(&mut client_rx).is_empty());
    }

    #[tokio::test]
    async fn mouse_move_forwarded_while_on_client() {
        let (clients, mut client_rx) = connected_clients();
        let (tx, rx) = mpsc::channel(16);
        let task = tokio::spawn(route_events(rx, clients, routing_config(), server_monitors(), dummy_release()));

        // Enter client: cursor at right edge (2559, 720) -> EnterScreen { x: 0, y: 720 }
        tx.send(InputEvent::MouseMoveAbs { x: 2559.0, y: 720.0 }).await.unwrap();
        // Move delta (+5, +10): server (2564, 730), client (0+5, 720+10) = (5, 730)
        tx.send(InputEvent::MouseMoveAbs { x: 2564.0, y: 730.0 }).await.unwrap();
        drop(tx);
        task.await.unwrap().unwrap();

        let msgs = drain(&mut client_rx);
        assert_eq!(msgs[0], S2C::EnterScreen { x: 0, y: 720 });
        assert_eq!(msgs[1], S2C::MouseMoveAbs { x: 5, y: 730 });
    }

    #[tokio::test]
    async fn leave_screen_when_cursor_hits_client_left_edge() {
        let (clients, mut client_rx) = connected_clients();
        let (tx, rx) = mpsc::channel(16);
        let task = tokio::spawn(route_events(rx, clients, routing_config(), server_monitors(), dummy_release()));

        // Enter at right edge -> client cursor at (0, 720)
        tx.send(InputEvent::MouseMoveAbs { x: 2559.0, y: 720.0 }).await.unwrap();
        // Delta (-10, 0) -> client (0-10, 720) = (-10, clamped to 0) -> at left edge -> LeaveScreen
        tx.send(InputEvent::MouseMoveAbs { x: 2549.0, y: 720.0 }).await.unwrap();
        drop(tx);
        task.await.unwrap().unwrap();

        let msgs = drain(&mut client_rx);
        assert_eq!(msgs[0], S2C::EnterScreen { x: 0, y: 720 });
        assert_eq!(msgs[1], S2C::LeaveScreen);
    }

    #[tokio::test]
    async fn key_forwarded_to_active_client() {
        let (clients, mut client_rx) = connected_clients();
        let (tx, rx) = mpsc::channel(16);
        let task = tokio::spawn(route_events(rx, clients, routing_config(), server_monitors(), dummy_release()));

        tx.send(InputEvent::MouseMoveAbs { x: 2559.0, y: 720.0 }).await.unwrap();
        tx.send(InputEvent::Key {
            keycode: 65,
            pressed: true,
            modifiers: Modifiers { shift: true, ..Modifiers::default() },
        }).await.unwrap();
        drop(tx);
        task.await.unwrap().unwrap();

        let msgs = drain(&mut client_rx);
        assert_eq!(msgs[0], S2C::EnterScreen { x: 0, y: 720 });
        assert_eq!(msgs[1], S2C::KeyEvent {
            keycode: 65,
            pressed: true,
            modifiers: Modifiers { shift: true, ..Modifiers::default() },
        });
    }

    #[tokio::test]
    async fn button_forwarded_to_active_client() {
        let (clients, mut client_rx) = connected_clients();
        let (tx, rx) = mpsc::channel(16);
        let task = tokio::spawn(route_events(rx, clients, routing_config(), server_monitors(), dummy_release()));

        tx.send(InputEvent::MouseMoveAbs { x: 2559.0, y: 720.0 }).await.unwrap();
        tx.send(InputEvent::MouseButton { button: MouseButton::Left, pressed: true }).await.unwrap();
        drop(tx);
        task.await.unwrap().unwrap();

        let msgs = drain(&mut client_rx);
        assert_eq!(msgs[1], S2C::MouseButton { button: MouseButton::Left, pressed: true });
    }

    #[tokio::test]
    async fn scroll_forwarded_to_active_client() {
        let (clients, mut client_rx) = connected_clients();
        let (tx, rx) = mpsc::channel(16);
        let task = tokio::spawn(route_events(rx, clients, routing_config(), server_monitors(), dummy_release()));

        tx.send(InputEvent::MouseMoveAbs { x: 2559.0, y: 720.0 }).await.unwrap();
        tx.send(InputEvent::Scroll { dx: 3.0, dy: -2.0 }).await.unwrap();
        drop(tx);
        task.await.unwrap().unwrap();

        let msgs = drain(&mut client_rx);
        assert_eq!(msgs[1], S2C::Scroll { dx: 3, dy: -2 });
    }

    #[tokio::test]
    async fn events_not_forwarded_without_active_client() {
        let (clients, mut client_rx) = connected_clients();
        let (tx, rx) = mpsc::channel(16);
        let task = tokio::spawn(route_events(rx, clients, routing_config(), server_monitors(), dummy_release()));

        tx.send(InputEvent::Key { keycode: 65, pressed: true, modifiers: Modifiers::default() }).await.unwrap();
        tx.send(InputEvent::MouseButton { button: MouseButton::Right, pressed: false }).await.unwrap();
        tx.send(InputEvent::Scroll { dx: 1.0, dy: 1.0 }).await.unwrap();
        drop(tx);
        task.await.unwrap().unwrap();

        assert!(drain(&mut client_rx).is_empty());
    }

    #[tokio::test]
    async fn missing_client_in_config_no_enter() {
        // Config has no client for the Right edge -> cursor at edge but no EnterScreen
        let (clients, mut client_rx) = connected_clients();
        let config = watch::channel(Arc::new(Config {
            server: ServerConfig { name: "server".into(), port: 24800 },
            clients: vec![ClientEntry { name: "mac".into(), edge: Edge::Left, offset: 0 }],
        })).1;
        let (tx, rx) = mpsc::channel(16);
        let task = tokio::spawn(route_events(rx, clients, config, server_monitors(), dummy_release()));

        tx.send(InputEvent::MouseMoveAbs { x: 2559.0, y: 720.0 }).await.unwrap();
        drop(tx);
        task.await.unwrap().unwrap();

        assert!(drain(&mut client_rx).is_empty());
    }

    #[tokio::test]
    async fn disconnected_client_releases_focus() {
        // Client is removed from map mid-session
        let (clients, _client_rx) = connected_clients();
        let (tx, rx) = mpsc::channel(16);
        let task = tokio::spawn(route_events(rx, clients.clone(), routing_config(), server_monitors(), dummy_release()));

        // Enter the client
        tx.send(InputEvent::MouseMoveAbs { x: 2559.0, y: 720.0 }).await.unwrap();
        // Remove client from map to simulate disconnect
        clients.write().await.remove("mac");
        // Send another event -- routing should silently drop focus
        tx.send(InputEvent::MouseMoveAbs { x: 2560.0, y: 720.0 }).await.unwrap();
        drop(tx);
        task.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn route_events_exits_when_channel_closed() {
        let clients = empty_clients();
        let config = routing_config();
        let (tx, rx) = mpsc::channel::<InputEvent>(16);
        let task = tokio::spawn(route_events(rx, clients, config, server_monitors(), dummy_release()));

        drop(tx);
        task.await.unwrap().unwrap();
    }

    // --- release signal on leave ---

    #[tokio::test]
    async fn release_tx_fires_when_cursor_leaves_client() {
        let (clients, _client_rx) = connected_clients();
        let (event_tx, event_rx) = mpsc::channel(16);
        let (release_tx, mut release_rx) = mpsc::channel(4);
        let task = tokio::spawn(route_events(event_rx, clients, routing_config(), server_monitors(), release_tx));

        // Enter client
        event_tx.send(InputEvent::MouseMoveAbs { x: 2559.0, y: 720.0 }).await.unwrap();
        // Leave (delta pushes client cursor to left edge)
        event_tx.send(InputEvent::MouseMoveAbs { x: 2549.0, y: 720.0 }).await.unwrap();
        drop(event_tx);
        task.await.unwrap().unwrap();

        assert!(release_rx.try_recv().is_ok(), "release_tx should have fired on LeaveScreen");
    }

    #[tokio::test]
    async fn release_tx_fires_when_no_client_at_edge() {
        // Config has client at Left, cursor hits Right -> no client -> release fires
        let (clients, _client_rx) = connected_clients();
        let config = watch::channel(Arc::new(Config {
            server: ServerConfig { name: "server".into(), port: 24800 },
            clients: vec![ClientEntry { name: "mac".into(), edge: Edge::Left, offset: 0 }],
        })).1;
        let (event_tx, event_rx) = mpsc::channel(16);
        let (release_tx, mut release_rx) = mpsc::channel(4);
        let task = tokio::spawn(route_events(event_rx, clients, config, server_monitors(), release_tx));

        event_tx.send(InputEvent::MouseMoveAbs { x: 2559.0, y: 720.0 }).await.unwrap();
        drop(event_tx);
        task.await.unwrap().unwrap();

        assert!(release_rx.try_recv().is_ok(), "release_tx should fire when no client at edge");
    }
}
