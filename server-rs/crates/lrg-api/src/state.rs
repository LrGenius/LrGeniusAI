//! Shared application state, including the db_path bind state machine
//! (port of `services/chroma.ensure_db_path` minus the store itself, which
//! arrives in M2).

use std::path::PathBuf;
use std::sync::{Mutex, RwLock};

use tokio::sync::Notify;

use lrg_common::{config, logging};

pub struct AppState {
    db_path: RwLock<Option<PathBuf>>,
    /// Serializes concurrent bind attempts (the Python `_db_path_lock`).
    bind_lock: Mutex<()>,
    /// Signalled by /shutdown and /restart to trigger graceful shutdown.
    pub shutdown: Notify,
    pub debug: bool,
}

impl AppState {
    pub fn new(initial_db_path: Option<PathBuf>, debug: bool) -> Self {
        Self {
            db_path: RwLock::new(initial_db_path),
            bind_lock: Mutex::new(()),
            shutdown: Notify::new(),
            debug,
        }
    }

    pub fn db_path(&self) -> Option<PathBuf> {
        self.db_path.read().unwrap().clone()
    }

    /// Bind the backend to `db_path`. Returns true if a switch/init
    /// happened, false when the path was already active. Moves the log
    /// file next to the catalog, like `config.update_log_path`.
    ///
    /// M2 will additionally (re)open the vector store here.
    pub fn ensure_db_path(&self, db_path: &str) -> bool {
        if db_path.is_empty() {
            return false;
        }
        let new_path = PathBuf::from(db_path);
        if self.db_path.read().unwrap().as_ref() == Some(&new_path) {
            return false;
        }

        let _guard = self.bind_lock.lock().unwrap();
        // Re-check inside the lock — another request may have just bound it.
        {
            let current = self.db_path.read().unwrap();
            if current.as_ref() == Some(&new_path) {
                return false;
            }
            match current.as_ref() {
                Some(old) => log::info!(
                    "Switching catalog database: {} -> {}",
                    old.display(),
                    new_path.display()
                ),
                None => log::info!("Binding backend to db_path from request: {db_path}"),
            }
        }

        logging::swap_log_file(&config::log_path_for_db(&new_path));
        *self.db_path.write().unwrap() = Some(new_path);
        true
    }
}
