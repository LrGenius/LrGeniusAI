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
    /// MCP over stdio, for AI clients that start local servers as a command
    /// (Claude Desktop). Forwards to the running server's /mcp endpoint; it
    /// does not start a server of its own.
    Mcp {
        /// The endpoint to forward to. Default: this computer's server, on
        /// the port GENIUSAI_PORT names (19819 unless changed).
        #[arg(long)]
        url: Option<String>,
        /// The agent token from the Plug-in Manager. Default: the
        /// LRGENIUS_MCP_TOKEN environment variable, which is how a client
        /// config passes it without putting it on the command line.
        #[arg(long)]
        token: Option<String>,
    },
}

fn main() {
    let cli = Cli::parse();

    // Before any logging is set up: the proxy must not touch the server's
    // log file, and nothing but protocol messages may reach its stdout.
    if let Some(Command::Mcp { url, token }) = &cli.command {
        let url = url
            .clone()
            .unwrap_or_else(|| format!("http://127.0.0.1:{}{}", config::port(), lrg_mcp::MCP_PATH));
        let token = token
            .clone()
            .or_else(|| std::env::var("LRGENIUS_MCP_TOKEN").ok());
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("failed to build tokio runtime");
        if let Err(e) = runtime.block_on(lrg_mcp::stdio::run(url, token)) {
            eprintln!("lrgenius mcp: {e}");
            std::process::exit(1);
        }
        return;
    }

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

    match &result {
        // The listener is fully dropped by now (axum::serve has returned),
        // so spawning the new binary here can never race the old one for
        // the port. This is the last thing the old process does.
        Ok(Some(relaunch_path)) => {
            log::info!("Relaunching updated binary: {}", relaunch_path.display());
            if let Err(e) = spawn_relaunch(relaunch_path) {
                log::error!("Failed to relaunch after update: {e}");
            }
        }
        Ok(None) => {}
        Err(e) => log::error!("Server exited with error: {e}"),
    }
    log::info!("Bye.");

    if result.is_err() {
        std::process::exit(1);
    }
}

/// Spawns the freshly-swapped-in binary, detached, with no arguments —
/// matching how the installer/OS would normally launch it (the plugin
/// re-calls `/v1/db/bind` once the new process is up, same as any other
/// restart). On Windows this goes through `run_hidden.vbs` next to the
/// exe (installer-placed) to avoid a console flash; falls back to a
/// direct spawn if that wrapper isn't present (dev/manual runs).
fn spawn_relaunch(exe_path: &std::path::Path) -> std::io::Result<()> {
    use std::process::Stdio;

    #[cfg(windows)]
    {
        let vbs_path = exe_path.with_file_name("run_hidden.vbs");
        if vbs_path.is_file() {
            return std::process::Command::new("wscript.exe")
                .arg(&vbs_path)
                .arg(exe_path)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .map(|_| ());
        }
    }

    std::process::Command::new(exe_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
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

async fn run(cli: Cli) -> Result<Option<PathBuf>, Box<dyn std::error::Error>> {
    // Mark server as ready for startup scripts.
    if let Some(db_path) = &cli.db_path {
        lifecycle::write_ok_file(db_path)?;
        lifecycle::write_pid_file(db_path)?;
    }

    let state = Arc::new(AppState::new(cli.db_path.clone(), cli.debug));
    let api = lrg_api::build_router(state.clone());
    // The MCP endpoint calls the REST routes in-process, so it gets its own
    // handle on them; it is merged in after the REST layers on purpose (no
    // db_path peeking, its own body limit).
    let mcp_shutdown = tokio_util::sync::CancellationToken::new();
    let app = api
        .clone()
        .merge(lrg_mcp::router(state.clone(), api, mcp_shutdown.clone()));

    tokio::spawn(maintenance_loop(state.clone()));

    let host = config::host();
    let port = config::port();
    let listener = tokio::net::TcpListener::bind((host.as_str(), port)).await?;
    log::info!("Starting production server on http://{host}:{port}");

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown({
        let state = state.clone();
        async move {
            shutdown_signal(state).await;
            // An MCP client's open event stream would otherwise keep the
            // graceful shutdown waiting forever.
            mcp_shutdown.cancel();
        }
    })
    .await?;

    let relaunch_path = state.relaunch_after_shutdown.lock().unwrap().take();
    Ok(relaunch_path)
}

/// Runs `AppState::run_maintenance` (model idle-unload + LanceDB
/// compaction) every 30 seconds for the life of the process. The tick
/// itself is cheap (an atomic-counter check plus two idle-timestamp
/// checks) — actual LanceDB compaction only fires once enough write ops
/// have piled up since the last run, so a short interval just bounds how
/// long a fast indexing run can mint fragments/versions before they get
/// compacted, without wasting work while idle. Runs for the whole process
/// lifetime — no shutdown wiring needed, it's just dropped along with the
/// runtime on exit.
async fn maintenance_loop(state: Arc<AppState>) {
    const INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);
    loop {
        tokio::time::sleep(INTERVAL).await;
        state.run_maintenance().await;
    }
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
