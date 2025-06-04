use clap::Parser;
use fileshare_daemon::{config::Settings, service::FileshareDaemon, Result};
use tracing::{error, info};
use tracing_subscriber;

#[derive(Parser)]
#[command(name = "fileshare-daemon")]
#[command(about = "The ultimate network file sharing daemon")]
struct Cli {
    /// Configuration file path
    #[arg(short, long)]
    config: Option<String>,

    /// Run in foreground (don't daemonize)
    #[arg(short, long)]
    foreground: bool,

    /// Verbose logging
    #[arg(short, long)]
    verbose: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Initialize logging with debug level for discovery
    let log_level = if cli.verbose { "debug" } else { "info" };
    tracing_subscriber::fmt()
        .with_env_filter(format!(
            "fileshare_daemon={},fileshare_daemon::network::discovery=debug",
            log_level
        ))
        .init();

    info!("Starting Fileshare Daemon v{}", env!("CARGO_PKG_VERSION"));

    // Load configuration
    let config_path = cli.config.as_deref();
    let settings = Settings::load(config_path)?;

    // Create and start the daemon
    let daemon = FileshareDaemon::new(settings).await?;

    // Handle shutdown gracefully
    let shutdown_signal = setup_shutdown_handler();

    tokio::select! {
        result = daemon.run() => {
            if let Err(e) = result {
                error!("Daemon error: {}", e);
                return Err(e);
            }
        }
        _ = shutdown_signal => {
            info!("Shutdown signal received, stopping daemon...");
        }
    }

    info!("Fileshare Daemon stopped");
    Ok(())
}

async fn setup_shutdown_handler() {
    use tokio::signal;

    #[cfg(unix)]
    {
        let mut sigterm = signal::unix::signal(signal::unix::SignalKind::terminate()).unwrap();
        let mut sigint = signal::unix::signal(signal::unix::SignalKind::interrupt()).unwrap();

        tokio::select! {
            _ = sigterm.recv() => {},
            _ = sigint.recv() => {},
        }
    }

    #[cfg(windows)]
    {
        signal::ctrl_c().await.unwrap();
    }
}
