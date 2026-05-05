// Server connection loop.
//
// Listens for incoming client connections, performs the handshake,
// then fans out input events captured by the active backend.

use anyhow::Result;
use tokio::net::TcpListener;
use tracing::{info, warn};
use wayflow_core::config::Config;

pub async fn run(config: Config) -> Result<()> {
    let addr = format!("0.0.0.0:{}", config.server.port);
    let listener = TcpListener::bind(&addr).await?;
    info!("listening on {addr}");

    loop {
        match listener.accept().await {
            Ok((stream, peer)) => {
                info!("connection from {peer}");
                // TODO: TLS upgrade, handshake, event fan-out
                drop(stream);
            }
            Err(e) => warn!("accept error: {e}"),
        }
    }
}
