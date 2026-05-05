// Server connection loop.
//
// Phase 1: TLS accept, handshake, clipboard fan-out, ping/pong keepalive.
// Phase 2 (TODO): wire up the CaptureBackend -- edge detection + event routing.

use std::{collections::HashMap, net::SocketAddr, sync::Arc};
use anyhow::{bail, Result};
use tokio::{
    net::TcpListener,
    sync::{mpsc, RwLock},
};
use tokio_rustls::TlsAcceptor;
use tracing::{debug, info, warn};
use wayflow_core::{config::Config, tls, transport};
use wayflow_proto::{C2S, HelloS2C, S2C, ScreenInfo, PROTOCOL_VERSION};

// Map of client name -> channel to deliver server-to-client messages.
type Clients = Arc<RwLock<HashMap<String, mpsc::Sender<S2C>>>>;

pub async fn run(config: Config) -> Result<()> {
    let (cert_path, key_path) = tls::default_cert_paths();
    let tls_cfg = tls::server_tls(&cert_path, &key_path)?;
    let rustls_cfg = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(tls_cfg.cert, tls_cfg.key)?;
    let acceptor = TlsAcceptor::from(Arc::new(rustls_cfg));

    let clients: Clients = Arc::new(RwLock::new(HashMap::new()));
    let addr = format!("0.0.0.0:{}", config.server.port);
    let listener = TcpListener::bind(&addr).await?;
    info!("listening on {addr}");

    let config = Arc::new(config);
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
    let (mut r, mut w) = tokio::io::split(tls);

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

    // Placeholder server screen -- replaced in phase 2 by the capture backend
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

    // Writer task: drains the mpsc queue and sends frames to the TLS stream.
    let write_task = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if transport::send_s2c(&mut w, &msg).await.is_err() {
                break;
            }
        }
    });

    // Ping task: sends a Ping every 10s; stops when the sender is closed.
    let ping_tx = tx.clone();
    let ping_task = tokio::spawn(async move {
        let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(10));
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
