use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;
use wayflow_core::config::Config;

#[derive(Parser)]
#[command(name = "wayflow", about = "Wayland-native KVM-over-network")]
struct Cli {
    /// Config file path (default: platform config dir / wayflow / config.toml)
    #[arg(long)]
    config: Option<std::path::PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run as the server (shares this machine's keyboard + mouse)
    Server,
    /// Run as a client (receives keyboard + mouse from a server)
    Client {
        /// Server host or IP address
        #[arg(long, short)]
        server: String,
        /// Server port
        #[arg(long, short, default_value_t = 24800)]
        port: u16,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    // Must be called before any rustls usage.
    wayflow_core::tls::install_default_crypto_provider();

    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_env("WAYFLOW_LOG"))
        .init();

    let cli = Cli::parse();
    let config_path = cli
        .config
        .unwrap_or_else(Config::default_path);

    let config = if config_path.exists() {
        Config::load(&config_path)?
    } else {
        tracing::warn!("no config at {}; using defaults", config_path.display());
        Config {
            server: wayflow_core::config::ServerConfig {
                name: hostname(),
                port: 24800,
            },
            clients: vec![],
        }
    };

    match cli.command {
        Command::Server => wayflow_server::server::run(config).await,
        Command::Client { server, port } => {
            wayflow_client::client::run(config, format!("{server}:{port}")).await
        }
    }
}

fn hostname() -> String {
    std::env::var("HOSTNAME")
        .or_else(|_| {
            std::fs::read_to_string("/etc/hostname").map(|s| s.trim().to_string())
        })
        .unwrap_or_else(|_| "wayflow".to_string())
}
