//! Shared application state, including the db_path bind state machine
//! (port of `services/chroma.ensure_db_path`): binding opens the LanceDB
//! store and auto-runs the one-time Chroma migration when needed.

use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use tokio::sync::{Mutex, Notify};

use lrg_common::{config, logging};
use lrg_ml::faces::FaceModel;
use lrg_ml::siglip::SiglipModel;
use lrg_store::{migrate, Store};

pub struct AppState {
    db_path: RwLock<Option<PathBuf>>,
    store: RwLock<Option<Arc<Store>>>,
    /// Serializes concurrent bind attempts (the Python `_db_path_lock`).
    bind_lock: Mutex<()>,
    /// Signalled by /shutdown and /restart to trigger graceful shutdown.
    pub shutdown: Notify,
    pub debug: bool,
    /// Global (not per-catalog) — lazily loaded on first use, matching
    /// server_lifecycle.py's model lifecycle.
    pub siglip: Arc<SiglipModel>,
    /// Global — lazily loaded, matching `services/face.py`'s `_get_face_app`.
    pub face: Arc<FaceModel>,
}

impl AppState {
    pub fn new(initial_db_path: Option<PathBuf>, debug: bool) -> Self {
        Self {
            db_path: RwLock::new(initial_db_path),
            store: RwLock::new(None),
            bind_lock: Mutex::new(()),
            shutdown: Notify::new(),
            debug,
            siglip: Arc::new(SiglipModel::new(lrg_ml::model_paths::resolve())),
            face: Arc::new(FaceModel::new(lrg_ml::model_paths::resolve_face())),
        }
    }

    pub fn db_path(&self) -> Option<PathBuf> {
        self.db_path.read().unwrap().clone()
    }

    pub fn store(&self) -> Option<Arc<Store>> {
        self.store.read().unwrap().clone()
    }

    fn is_bound_to(&self, path: &PathBuf) -> bool {
        self.db_path.read().unwrap().as_ref() == Some(path) && self.store.read().unwrap().is_some()
    }

    /// Bind the backend to `db_path`: open the store (running the one-time
    /// Chroma migration when applicable) and move the log file next to the
    /// catalog. Returns true if a switch/init happened, false when the path
    /// was already active — matching the Python semantics where "already
    /// bound" requires both the path to match AND the client to be open.
    pub async fn ensure_db_path(&self, db_path: &str) -> Result<bool, String> {
        if db_path.is_empty() {
            return Ok(false);
        }
        let new_path = PathBuf::from(db_path);
        if self.is_bound_to(&new_path) {
            return Ok(false);
        }

        let _guard = self.bind_lock.lock().await;
        // Re-check inside the lock — another request may have just bound it.
        if self.is_bound_to(&new_path) {
            return Ok(false);
        }
        match self.db_path.read().unwrap().as_ref() {
            Some(old) if *old != new_path => log::info!(
                "Switching catalog database: {} -> {}",
                old.display(),
                new_path.display()
            ),
            None => log::info!("Binding backend to db_path from request: {db_path}"),
            _ => {}
        }

        logging::swap_log_file(&config::log_path_for_db(&new_path));

        let lance_root = migrate::lance_root_for_db(&new_path);
        let store = Store::open(&lance_root)
            .await
            .map_err(|e| format!("failed to open store at {}: {e}", lance_root.display()))?;
        if let Some(summary) = migrate::ensure_migrated(&new_path, &lance_root, &store).await? {
            log::info!("Chroma migration completed: {summary}");
        }

        *self.store.write().unwrap() = Some(Arc::new(store));
        *self.db_path.write().unwrap() = Some(new_path);
        Ok(true)
    }
}
