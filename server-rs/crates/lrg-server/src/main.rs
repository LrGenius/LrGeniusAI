//! LrGenius Server (Rust backend) — entrypoint. Port of
//! `geniusai_server.py` startup/shutdown semantics: log banner, OK/PID
//! handshake files next to the catalog, GENIUSAI_HOST/PORT binding, and
//! cleanup of the handshake files on exit.
//!
//! This stays a normal console-subsystem binary on Windows (unlike the
//! Python era's separate windowless `pythonw.exe`) so the `migrate` CLI
//! subcommand always has working stdout/stderr. The installer and
//! self-updater instead launch the always-running service via a tiny
//! `.vbs` wrapper (`installers/windows/run_hidden.vbs`) that starts it
//! with a hidden window — the standard, well-tested way to background a
//! console-subsystem exe on Windows without touching low-level console
//! attach/redirect APIs that can't be verified without a Windows machine.

use std::path::PathBuf;
use std::sync::Arc;

use clap::{Parser, Subcommand};

use lrg_api::state::AppState;
use lrg_common::{config, lifecycle, logging, version};

#[derive(Parser)]
#[command(name = "geniusai-server", about = "LrGenius Server")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    /// Path to the database folder (e.g. <catalog dir>/lrgenius.db)
    #[arg(long = "db-path", global = true)]
    db_path: Option<PathBuf>,

    /// Enable debug mode with debug log level
    #[arg(long, global = true)]
    debug: bool,
}

#[derive(Subcommand)]
enum Command {
    /// One-time migration of a ChromaDB dir into the LanceDB store.
    Migrate {
        /// Read + report the chroma dir without writing anything.
        #[arg(long)]
        verify_only: bool,
    },
}

fn main() {
    let cli = Cli::parse();

    if let Some(Command::Migrate { verify_only }) = &cli.command {
        let verify_only = *verify_only;
        let db_path = cli
            .db_path
            .clone()
            .expect("--db-path is required for migrate");
        logging::init_with_options(&config::log_path_for_db(&db_path), cli.debug, false);
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("failed to build tokio runtime");
        let result = runtime.block_on(run_migrate(&db_path, verify_only));
        match result {
            Ok(summary) => {
                println!("{}", serde_json::to_string_pretty(&summary).unwrap());
                return;
            }
            Err(e) => {
                eprintln!("migration failed: {e}");
                std::process::exit(1);
            }
        }
    }

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

async fn run_migrate(
    db_path: &std::path::Path,
    verify_only: bool,
) -> Result<serde_json::Value, String> {
    use lrg_store::{migrate, Store};

    if verify_only {
        let db_path = db_path.to_path_buf();
        let collections = tokio::task::spawn_blocking(move || lrg_chroma_reader_counts(&db_path))
            .await
            .map_err(|e| e.to_string())??;
        return Ok(collections);
    }

    let lance_root = migrate::lance_root_for_db(db_path);
    let store = Store::open(&lance_root).await.map_err(|e| e.to_string())?;
    match migrate::ensure_migrated(db_path, &lance_root, &store).await? {
        Some(summary) => Ok(summary),
        None => Ok(serde_json::json!({
            "status": "nothing to do (no chroma dir, or already migrated)",
            "marker": migrate::migration_marker(&lance_root).display().to_string(),
        })),
    }
}

fn lrg_chroma_reader_counts(db_path: &std::path::Path) -> Result<serde_json::Value, String> {
    let collections = lrg_chroma_reader::read_chroma_dir(db_path).map_err(|e| e.to_string())?;
    let mut out = serde_json::Map::new();
    for c in collections {
        let with_vectors = c.records.iter().filter(|r| r.embedding.is_some()).count();
        out.insert(
            c.name.clone(),
            serde_json::json!({
                "records": c.records.len(),
                "with_vectors": with_vectors,
                "dimension": c.dimension,
            }),
        );
    }
    Ok(serde_json::Value::Object(out))
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
