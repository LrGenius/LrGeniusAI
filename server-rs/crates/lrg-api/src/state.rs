//! Shared application state, including the db_path bind state machine
//! (port of `services/chroma.ensure_db_path`): binding opens the LanceDB
//! store, auto-runs the one-time Chroma migration when needed, and applies any
//! pending store upgrades (`lrg_store::upgrade`).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex as StdMutex, RwLock};

use tokio::sync::{Mutex, Notify};

use lrg_common::{config, jobs::JobRegistry, logging};
use lrg_ml::bioclip::{BioclipModel, BioclipModelPaths};
use lrg_ml::faces::{FaceModel, FaceModelPaths};
use lrg_ml::siglip::{ModelPaths as SiglipModelPaths, SiglipModel};
use lrg_store::{migrate, upgrade, Store};

use crate::agent_bridge::AgentBridge;
use crate::llm_engine::LlmEngineSlot;
use crate::mlx_engine::MlxEngineSlot;
use crate::routes::clip::ModelDownloadStatus;
use crate::ui_bridge::UiBridge;

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
    /// Global — BioCLIP 2 plus its pruned Tree-of-Life head, same lazy-load
    /// and idle-unload contract as `siglip`/`face`. Its head alone is a few
    /// hundred MB resident, so leaving it out of `run_maintenance` would keep
    /// that memory pinned for the life of the process.
    pub bioclip: Arc<BioclipModel>,
    /// Global — port of `services/jobs.py`, read back through `GET /v1/jobs/{job_id}`.
    pub jobs: Arc<JobRegistry>,
    /// Hands actions from a `/v1/ui/` page in the browser to the plugin task
    /// that is waiting for them, since only the plugin can touch the
    /// Lightroom catalog. Global for the same reason `jobs` is: Lightroom
    /// runs one of these tasks at a time.
    pub ui_bridge: Arc<UiBridge>,
    /// Hands commands from the MCP endpoint (an AI agent) to the plugin and
    /// their answers back. Global, like `ui_bridge`: one Lightroom plugin at a
    /// time drives a backend. See [`crate::agent_bridge`].
    pub agent_bridge: Arc<AgentBridge>,
    /// Text embeddings of the fixed prompts — CLIP-IQA, the organism gate,
    /// the scene probes — read from the database's `text_embeddings` table,
    /// or made once with the text tower and stored there. See
    /// [`crate::prompt_embeddings`].
    ///
    /// Deliberately *not* cleared alongside `siglip.unload()`: the embeddings
    /// stay valid across an unload/reload cycle, and they are what lets a run
    /// do without the text tower in the first place.
    pub prompt_embeddings: crate::prompt_embeddings::PromptCache,
    /// Download progress, keyed by asset group (`"clip"`, `"llm"`,
    /// `"bioclip"`). Keyed
    /// rather than a single slot because a GGUF download and a SigLIP2
    /// download can legitimately be in flight at the same time.
    pub model_download: Arc<StdMutex<HashMap<String, ModelDownloadStatus>>>,
    /// Global — the in-process LLM, lazily loaded and idle-unloaded exactly
    /// like `siglip`/`face`.
    pub llm: Arc<LlmEngineSlot>,
    /// The MLX engine, held separately from `llm` so that switching provider
    /// back and forth does not evict the other backend's resident model.
    pub mlx: Arc<MlxEngineSlot>,
    /// Set by `/update/apply` once the binary swap is done, just before
    /// triggering graceful shutdown. `main.rs` checks this after
    /// `axum::serve`'s graceful shutdown returns (listener fully
    /// dropped) and spawns the new binary at this path — never from
    /// inside the request handler itself, so there's no bind-race
    /// between the exiting old process and the freshly spawned new one.
    pub relaunch_after_shutdown: StdMutex<Option<PathBuf>>,
    /// Taxon name → species-database links, cached on disk. Global rather
    /// than per-catalog: which GBIF page `Panthera leo` lives on does not
    /// depend on whose catalog asked.
    pub species_links: Arc<crate::species_links::LinkResolver>,
}

/// Where the three ONNX model families live on disk.
///
/// Bundled into one value so an `AppState` can be built with the model
/// locations *supplied* rather than resolved from the environment.
///
/// That distinction is what lets a caller guarantee no model is loadable.
/// [`AppState::new`] resolves to the shared `~/.cache/lrgenius/models`
/// directory, so on any machine that has actually downloaded the assets a
/// route that touches a model loads one for real — which is the correct
/// behaviour for the server and the wrong one for a test asserting response
/// shapes. See `tests/contract.rs` for the failure that motivated this.
pub struct ModelAssetPaths {
    pub siglip: SiglipModelPaths,
    pub face: FaceModelPaths,
    pub bioclip: BioclipModelPaths,
}

impl ModelAssetPaths {
    /// The env-var-then-shared-cache resolution the server itself uses.
    pub fn from_env() -> Self {
        Self {
            siglip: lrg_ml::model_paths::resolve(),
            face: lrg_ml::model_paths::resolve_face(),
            bioclip: lrg_ml::model_paths::resolve_bioclip(),
        }
    }
}

impl AppState {
    pub fn new(initial_db_path: Option<PathBuf>, debug: bool) -> Self {
        Self::with_model_paths(initial_db_path, debug, ModelAssetPaths::from_env())
    }

    /// [`AppState::new`] with the ML model locations given explicitly instead
    /// of resolved from the environment.
    pub fn with_model_paths(
        initial_db_path: Option<PathBuf>,
        debug: bool,
        models: ModelAssetPaths,
    ) -> Self {
        Self {
            db_path: RwLock::new(initial_db_path),
            store: RwLock::new(None),
            bind_lock: Mutex::new(()),
            shutdown: Notify::new(),
            debug,
            siglip: Arc::new(SiglipModel::new(models.siglip)),
            face: Arc::new(FaceModel::new(models.face)),
            bioclip: Arc::new(BioclipModel::new(models.bioclip)),
            jobs: Arc::new(JobRegistry::new()),
            ui_bridge: Arc::new(UiBridge::new()),
            agent_bridge: Arc::new(AgentBridge::new()),
            prompt_embeddings: Default::default(),
            model_download: Arc::new(StdMutex::new(HashMap::new())),
            llm: Arc::default(),
            mlx: Arc::default(),
            relaunch_after_shutdown: StdMutex::new(None),
            species_links: Arc::new(crate::species_links::LinkResolver::new()),
        }
    }

    pub fn db_path(&self) -> Option<PathBuf> {
        self.db_path.read().unwrap().clone()
    }

    pub fn store(&self) -> Option<Arc<Store>> {
        self.store.read().unwrap().clone()
    }

    /// Where per-catalog side files (`person_names.json`) live — the
    /// LanceDB root, matching where Python's `person_names.json` sat
    /// alongside `chroma.sqlite3` inside the old Chroma dir.
    /// Background maintenance: idle-unloads ML models and compacts the
    /// LanceDB tables, so a long indexing run doesn't accumulate the
    /// per-photo `merge_insert` write amplification (new fragment +
    /// dataset version per write) indefinitely. Called frequently (every
    /// few seconds) from `lrg-server`'s maintenance loop, never per-request
    /// — cheap when idle, since the compaction itself only actually runs
    /// once `Store::pending_write_ops` crosses `COMPACT_WRITE_THRESHOLD`.
    /// A fixed wall-clock interval alone isn't enough: at "rocket" indexing
    /// speed a catalog can mint thousands of fragments/versions well
    /// before a 10-minute tick, which is what let RSS climb unbounded to
    /// an OOM kill around ~3600 photos.
    pub async fn run_maintenance(&self) {
        self.siglip.unload_if_idle();
        self.face.unload_if_idle();
        self.bioclip.unload_if_idle();
        self.llm.unload_if_idle();
        self.mlx.unload_if_idle();
        if let Some(store) = self.store() {
            if store.pending_write_ops() >= lrg_store::COMPACT_WRITE_THRESHOLD {
                match store.optimize_all().await {
                    // Logged at info because it's the one recurring signal
                    // for the memory-growth class of bug this loop exists
                    // to prevent: fragment counts in the single digits and
                    // a steady cache size mean compaction is doing bounded
                    // work, while numbers that scale with catalog size mean
                    // it has gone back to rewriting everything.
                    Ok(summary) => log::info!(
                        "LanceDB maintenance: compacted {} fragment(s) into {}, \
                         pruned {} old version(s), caches at {} MiB",
                        summary.fragments_removed,
                        summary.fragments_added,
                        summary.old_versions_removed,
                        summary.cache_bytes / (1024 * 1024),
                    ),
                    Err(e) => log::warn!("LanceDB maintenance optimize failed: {e}"),
                }
            }
        }
    }

    pub fn lance_root(&self) -> Option<PathBuf> {
        self.db_path
            .read()
            .unwrap()
            .as_ref()
            .map(|p| migrate::lance_root_for_db(p))
    }

    fn is_bound_to(&self, path: &PathBuf) -> bool {
        self.db_path.read().unwrap().as_ref() == Some(path) && self.store.read().unwrap().is_some()
    }

    /// Bind the backend to `db_path`: open the store (running the one-time
    /// Chroma migration when applicable, then any pending store upgrades) and
    /// move the log file next to the catalog. Returns true if a switch/init
    /// happened, false when the path was already active — matching the
    /// Python semantics where "already bound" requires both the path to match
    /// AND the client to be open.
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
        // After the Chroma migration, so rows it just brought over are
        // upgraded too, and before the store is published, so no request
        // writes alongside a step.
        let report = upgrade::ensure_upgraded(
            &lance_root,
            &store,
            &upgrade::UpgradeContext {
                embedding_model: lrg_ml::siglip::MODEL_ID,
            },
        )
        .await;
        for (step, summary) in &report.applied {
            log::info!("Store upgrade {step} completed: {summary}");
        }
        if let Some((step, e)) = &report.failed {
            // Not reported to the user: nothing reads worse for it. The only
            // step so far stamps an identity that, while absent, is already
            // taken as current, and it is retried on the next bind.
            log::warn!("Store upgrade {step} failed, will retry on the next bind: {e}");
        }

        *self.store.write().unwrap() = Some(Arc::new(store));
        *self.db_path.write().unwrap() = Some(new_path);
        Ok(true)
    }
}
