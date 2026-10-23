//! The lazily-loaded, idle-unloadable ort session holder every ONNX family in
//! this crate sits on, and the one place a session is built.
//!
//! Device selection mirrors `config.TORCH_DEVICE`: CoreML on macOS (opt-in, see
//! [`build_session`]), CPU elsewhere (Windows is deliberately CPU-only to avoid
//! VRAM contention with local CUDA LLMs).

use std::path::Path;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use ort::session::Session;

/// How long a tower may sit unused before the idle sweep drops it.
pub(crate) const IDLE_UNLOAD_SECONDS: i64 = 30 * 60;

/// What [`Tower::with`] reports when, after a successful build, there is still
/// nothing loaded: a caller maps it onto its own error type.
#[derive(Debug)]
pub(crate) struct NotLoaded;

/// One lazily-loaded, independently idle-unloaded tower.
///
/// The image and text towers each hold one of these. They used to share a
/// single `Option<Loaded>` that owned both sessions behind one mutex and one
/// idle clock, so anything that touched either tower built both: an
/// embeddings-only run paid for the text tower it never calls, and a semantic
/// search paid for the image tower. Nothing about either tower's cost changes
/// here — only which of them a call makes resident.
pub(crate) struct Tower<T> {
    /// Which model family this tower belongs to ("SigLIP2", "aesthetic"), for
    /// the log lines only.
    family: &'static str,
    loaded: Mutex<Option<T>>,
    /// This tower's own clock. A shared one meant image work refreshing the
    /// text tower's idle timer, so a tower nothing ever called could not be
    /// dropped while the other one was busy.
    last_used_unix: AtomicI64,
    load_error: Mutex<Option<String>>,
}

impl<T> Tower<T> {
    pub(crate) fn new(family: &'static str) -> Self {
        Tower {
            family,
            loaded: Mutex::new(None),
            last_used_unix: AtomicI64::new(0),
            load_error: Mutex::new(None),
        }
    }

    /// Runs `f` against this tower's loaded value, building it first when it is
    /// not there yet.
    ///
    /// `name` reaches the failure log and is only for the human reading it: a
    /// load that fails on a background path has to say which tower it was.
    /// `build` fails with an ort error, `f` with the caller's own error type
    /// `E`, so (for SigLIP2) the tokenizer's error reaches the caller unchanged.
    pub(crate) fn with<R, E: From<ort::Error> + From<NotLoaded>>(
        &self,
        name: &str,
        build: impl FnOnce() -> ort::Result<T>,
        f: impl FnOnce(&mut T) -> Result<R, E>,
    ) -> Result<R, E> {
        let mut guard = self.loaded.lock().unwrap();
        if guard.is_none() {
            match build() {
                Ok(value) => {
                    *guard = Some(value);
                    *self.load_error.lock().unwrap() = None;
                }
                Err(e) => {
                    log::warn!("Could not load the {} {name} tower: {e}", self.family);
                    *self.load_error.lock().unwrap() = Some(e.to_string());
                    return Err(e.into());
                }
            }
        }
        self.last_used_unix.store(now_unix(), Ordering::Relaxed);
        let loaded = guard.as_mut().ok_or(NotLoaded)?;
        f(loaded)
    }

    /// Drops this tower's session(s). Safe to call when nothing is loaded.
    ///
    /// The reason is a parameter because this is reached two ways — the
    /// `/unload` route and the idle sweep — and the log line used to say
    /// "due to inactivity" for both. A real `/unload` from the plugin then
    /// looked like an idle timeout in the log, which is the kind of small lie
    /// that costs an hour the next time someone reads it.
    pub(crate) fn unload_with_reason(&self, name: &str, reason: &str) {
        let mut guard = self.loaded.lock().unwrap();
        if guard.take().is_some() {
            log::info!("Unloading {} {name} tower ({reason})", self.family);
        }
    }

    pub(crate) fn unload_if_idle(&self, name: &str) {
        let last = self.last_used_unix.load(Ordering::Relaxed);
        if last != 0 && now_unix() - last >= IDLE_UNLOAD_SECONDS {
            self.unload_with_reason(name, "idle");
        }
    }

    pub(crate) fn status(&self) -> (&'static str, Option<String>) {
        if self.loaded.lock().unwrap().is_some() {
            ("loaded", None)
        } else if let Some(err) = self.load_error.lock().unwrap().clone() {
            ("failed", Some(err))
        } else {
            ("not_loaded", None)
        }
    }
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// EP selection is env-gated rather than automatic-on-macOS for now:
/// spike S2 found CoreML's MLProgram format needs a *static*-batch export
/// to hit its good numbers (123ms/img vs MPS's 141ms) and to avoid
/// crashing — the dynamic-batch graphs used here/in CI don't qualify yet.
/// M9's model export pipeline will ship per-purpose static exports and
/// this can default to CoreML on macOS at that point.
///
/// Optimization level pinned per [`crate::OPTIMIZATION_LEVEL`] — onnxruntime
/// segfaults loading `siglip2_text.onnx` at its own `ORT_ENABLE_ALL` default,
/// and triples the image tower's resident memory.
pub(crate) fn build_session(path: &Path) -> ort::Result<Session> {
    let mut builder = Session::builder()?.with_optimization_level(crate::OPTIMIZATION_LEVEL)?;
    #[cfg(target_os = "macos")]
    if std::env::var("LRG_ML_EP").as_deref() == Ok("coreml") {
        use ort::ep::coreml::{ComputeUnits, ModelFormat};
        use ort::ep::CoreML;
        builder = builder.with_execution_providers([CoreML::default()
            .with_model_format(ModelFormat::MLProgram)
            .with_compute_units(ComputeUnits::All)
            .with_model_cache_dir(std::env::temp_dir().join("lrg-coreml-cache").display())
            .build()])?;
    }
    // The CPU EP's default arena only ever grows (never returns memory to
    // the OS) — confirmed via per-photo RSS logging showing this same
    // pattern on lrg-ml::faces's sessions. CoreML falls back to CPU for
    // unsupported ops, so this matters on macOS too, not just the
    // CPU-only platforms. See faces::build_session for the full writeup.
    builder = builder
        .with_execution_providers([ort::ep::CPU::default().with_arena_allocator(false).build()])?;
    // KleidiAI is left enabled: it is worth ~4x on this tower and no longer
    // leaks on onnxruntime 1.28. See `crate::DISABLE_KLEIDIAI`.
    crate::apply_kleidiai_policy(builder)?.commit_from_file(path)
}
