// Client connection loop.
//
// Connects to the server, performs the handshake, then drives the active
// injection backend with events received from the server.

use anyhow::Result;
use tracing::info;
use wayflow_core::config::Config;

pub async fn run(config: Config, server_addr: String) -> Result<()> {
    info!("connecting to {server_addr}");
    // TODO: TLS connect, HelloC2S handshake, event injection loop
    Ok(())
}
