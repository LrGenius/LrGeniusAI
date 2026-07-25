//! LrGenius Server (Rust backend) — entrypoint. Port of
//! `geniusai_server.py` startup/shutdown semantics: log banner, OK/PID
//! handshake files next to the catalog, GENIUSAI_HOST/PORT binding, and
//! cleanup of the handshake files on exit.

use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;

use lrg_api::state::AppState;
use lrg_common::{config, lifecycle, logging, version};

#[derive(Parser)]
#[command(name = "geniusai-server", about = "LrGenius Server")]
struct Cli {
    /// Path to the database folder (e.g. <catalog dir>/lrgenius.db)
    #[arg(long = "db-path")]
    db_path: Option<PathBuf>,

    /// Enable debug mode with debug log level
    #[arg(long)]
    debug: bool,
}

fn main() {
    let cli = Cli::parse();

    let log_path = match &cli.db_path {
        Some(p) => config::log_path_for_db(p),
        None => config::default_log_path(),
    };
    logging::init(&log_path, cli.debug);

    let info = version::get_backend_version_info();
    log::info!("{}", "=".repeat(60));
    log::info!(
        "LrGenius Server version {} (build {})",
        info.backend_version,
        info.backend_build
    );
    log::info!("LrGenius Server starting...");
    log::info!(
        "Database Path: {}",
        cli.db_path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "Idle (waiting for plugin initialize)".to_string())
    );
    log::info!("{}", "=".repeat(60));

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("failed to build tokio runtime");

    let exit_db_path = cli.db_path.clone();
    let result = runtime.block_on(run(cli));

    log::info!("Shutting down server...");
    if let Some(db_path) = &exit_db_path {
        lifecycle::remove_pid_file(db_path);
        lifecycle::remove_ok_file(db_path);
    }
    log::info!("Bye.");

    if let Err(e) = result {
        log::error!("Server exited with error: {e}");
        std::process::exit(1);
    }
}

async fn run(cli: Cli) -> Result<(), Box<dyn std::error::Error>> {
    // Mark server as ready for startup scripts.
    if let Some(db_path) = &cli.db_path {
        lifecycle::write_ok_file(db_path)?;
        lifecycle::write_pid_file(db_path)?;
    }

    let state = Arc::new(AppState::new(cli.db_path.clone(), cli.debug));
    let app = lrg_api::build_router(state.clone());

    let host = config::host();
    let port = config::port();
    let listener = tokio::net::TcpListener::bind((host.as_str(), port)).await?;
    log::info!("Starting production server on http://{host}:{port}");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal(state))
        .await?;
    Ok(())
}

/// Resolves on /shutdown, /restart, Ctrl-C/SIGINT, or SIGTERM (launchd).
async fn shutdown_signal(state: Arc<AppState>) {
    let api_shutdown = state.shutdown.notified();

    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut sigterm = signal(SignalKind::terminate()).expect("failed to install SIGTERM");
        tokio::select! {
            _ = api_shutdown => {},
            _ = tokio::signal::ctrl_c() => {},
            _ = sigterm.recv() => {},
        }
    }
    #[cfg(not(unix))]
    {
        tokio::select! {
            _ = api_shutdown => {},
            _ = tokio::signal::ctrl_c() => {},
        }
    }
}
