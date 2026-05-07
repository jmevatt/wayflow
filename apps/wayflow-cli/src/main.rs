// Hide the Windows console window in release builds so the tray app
// doesn't pop a black cmd window. Debug builds keep the console for stderr.
#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};
use wayflow_core::config::{ClientConfig, Config};

#[derive(Parser)]
#[command(name = "wayflow", about = "Wayland-native KVM-over-network")]
struct Cli {
    /// Config file path (server mode: config.toml; client mode: client.toml)
    #[arg(long)]
    config: Option<std::path::PathBuf>,

    /// Subcommand to run. Defaults to `tray` so double-clicking the bundled
    /// .app on macOS / .exe on Windows opens the tray supervisor without
    /// needing a launcher shim.
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Run as the server (shares this machine's keyboard + mouse)
    Server,
    /// Run as a client (receives keyboard + mouse from a server)
    Client {
        /// Server host or IP address (overrides client config file)
        #[arg(long, short)]
        server: Option<String>,
        /// Server port
        #[arg(long, short, default_value_t = 24800)]
        port: u16,
    },
    /// Run the tray app (menu-bar/system-tray UI; supervises a child wayflow process)
    Tray,
}

fn main() -> Result<()> {
    // Must be called before any rustls usage.
    wayflow_core::tls::install_default_crypto_provider();

    let cli = Cli::parse();
    let command = cli.command.unwrap_or(Command::Tray);

    // Hold the guard for the full lifetime of main -- when it drops the
    // background flusher thread joins and any buffered log lines are written.
    let _log_guard = init_tracing(log_basename_for(&command))?;

    match command {
        // Tray runs the eframe/winit event loop on the main thread; no tokio
        // runtime here -- the supervised child process gets its own.
        Command::Tray => wayflow_tray::run(),

        // Server and client use a tokio runtime. We build it explicitly rather
        // than via #[tokio::main] so the Tray branch above doesn't pay for it.
        Command::Server => {
            let config_path = cli.config.clone().unwrap_or_else(Config::default_path);
            tokio_rt()?.block_on(async move {
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
                wayflow_server::server::run(config, config_path).await
            })
        }

        Command::Client { server, port } => {
            let cli_config = cli.config.clone();
            tokio_rt()?.block_on(async move {
                let addr = if let Some(host) = server {
                    format!("{host}:{port}")
                } else {
                    let path = cli_config.unwrap_or_else(ClientConfig::default_path);
                    ClientConfig::load(&path)
                        .with_context(|| {
                            format!(
                                "no --server given and could not load client config from {}",
                                path.display()
                            )
                        })?
                        .server_addr()
                };
                wayflow_client::client::run(addr, hostname()).await
            })
        }
    }
}

fn tokio_rt() -> Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("build tokio runtime")
}

fn log_basename_for(cmd: &Command) -> &'static str {
    match cmd {
        Command::Server => "server.log",
        Command::Client { .. } => "client.log",
        Command::Tray => "tray.log",
    }
}

/// Set up tracing with two layers: human-readable to stderr (controlled by
/// $WAYFLOW_LOG, defaults to info) and structured JSON to a daily-rotated file
/// in the platform cache dir (always at debug level so we have history when
/// post-morteming).
///
/// Returns a WorkerGuard that MUST be held by main to flush the non-blocking
/// file writer on shutdown.
fn init_tracing(log_basename: &'static str) -> Result<tracing_appender::non_blocking::WorkerGuard> {
    let stderr_filter =
        EnvFilter::try_from_env("WAYFLOW_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    // Separate filter for the file -- always include debug so the rotating
    // log has more detail than what the user normally sees on stderr.
    let file_filter = EnvFilter::new(
        std::env::var("WAYFLOW_LOG_FILE").unwrap_or_else(|_| "wayflow=debug,info".into()),
    );

    let log_dir = log_dir();
    std::fs::create_dir_all(&log_dir)
        .with_context(|| format!("create log dir {}", log_dir.display()))?;

    let appender = tracing_appender::rolling::daily(&log_dir, log_basename);
    let (file_writer, guard) = tracing_appender::non_blocking(appender);

    tracing_subscriber::registry()
        .with(
            fmt::layer()
                .with_writer(std::io::stderr)
                .with_filter(stderr_filter),
        )
        .with(
            fmt::layer()
                .json()
                .with_writer(file_writer)
                .with_filter(file_filter),
        )
        .init();

    tracing::info!("logging to {}/{}.<date>", log_dir.display(), log_basename);
    Ok(guard)
}

fn log_dir() -> std::path::PathBuf {
    // Cache dir on linux (~/.cache/wayflow), Library/Caches on macOS.
    // dirs::cache_dir() returns the platform default; tests rarely need this.
    dirs::cache_dir()
        .map(|p| p.join("wayflow"))
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp/wayflow"))
}

fn hostname() -> String {
    // HOSTNAME env var (set by most Linux shells)
    if let Ok(h) = std::env::var("HOSTNAME") {
        if !h.is_empty() {
            return h;
        }
    }
    // gethostname(2) -- works on Linux and macOS
    let mut buf = [0u8; 256];
    let ok = unsafe { libc::gethostname(buf.as_mut_ptr() as *mut libc::c_char, buf.len()) };
    if ok == 0 {
        let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
        if let Ok(s) = std::str::from_utf8(&buf[..end]) {
            let s = s.trim().to_string();
            if !s.is_empty() {
                return s;
            }
        }
    }
    "wayflow".to_string()
}
