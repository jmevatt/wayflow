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
        .with_env_filter(
            EnvFilter::try_from_env("WAYFLOW_LOG")
                .unwrap_or_else(|_| EnvFilter::new("info")),
        )
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
    // HOSTNAME env var (set by most Linux shells)
    if let Ok(h) = std::env::var("HOSTNAME") {
        if !h.is_empty() { return h; }
    }
    // gethostname(2) -- works on Linux and macOS
    let mut buf = [0u8; 256];
    let ok = unsafe {
        libc::gethostname(buf.as_mut_ptr() as *mut libc::c_char, buf.len())
    };
    if ok == 0 {
        let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
        if let Ok(s) = std::str::from_utf8(&buf[..end]) {
            let s = s.trim().to_string();
            if !s.is_empty() { return s; }
        }
    }
    "wayflow".to_string()
}
