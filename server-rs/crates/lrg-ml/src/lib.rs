//! ONNX inference: SigLIP2 image/text embedding via `ort`, plus the face
//! pipeline (YuNet detection + FaceNet recognition).

/// Session config key that turns off MLAS's KleidiAI convolution kernels
/// (onnxruntime >= 1.25). Ignored on non-arm64 builds, where these kernels
/// don't exist.
///
/// History: under onnxruntime 1.24 (`ort` 2.0.0-rc.12) these kernels leaked
/// badly — `ArmKleidiAI::MlasConv` allocated a workspace with `operator new`
/// on every convolution and never freed it, which `malloc_history` pinned as
/// 89% of all retained bytes while a 14k-photo catalog grew to a 45 GB
/// footprint on a 24 GB machine. That is why the sessions used to set this
/// key unconditionally.
///
/// **The leak is fixed as of onnxruntime 1.28 (rc.13), so we no longer set
/// it.** The original fix bundled the rc.12 -> rc.13 upgrade together with
/// this key and credited the key with the improvement; re-measuring the two
/// changes separately showed the upgrade did all the work and the key only
/// cost speed. Same-binary A/B on rc.13, arm64 macOS, CPU EP, per image:
///
/// | | KleidiAI on | off | |
/// |---|---|---|---|
/// | SigLIP embed | ~480 ms | ~1950 ms | 4.1x |
/// | Face detect + embed | ~68 ms | ~140 ms | 2.1x |
///
/// and `footprint` over 200 images is flat either way (<= 0.01 MB/image on
/// both sessions), versus the 25-31 MB/photo that rc.12 retained.
/// Embeddings are bit-identical between the two paths (worst-case cosine
/// 1.000000 across a real photo set), so no index is affected.
///
/// [`kleidiai_disabled`] is the escape hatch if a future onnxruntime or some
/// other arm64 host regresses this.
pub const DISABLE_KLEIDIAI: &str = "mlas.disable_kleidiai";

/// Whether to opt out of the KleidiAI kernels, via `LRG_DISABLE_KLEIDIAI=1`.
///
/// Off by default — see [`DISABLE_KLEIDIAI`] for why the kernels are safe to
/// use again. This exists so a leak regression can be diagnosed and worked
/// around in the field without shipping a new binary; it costs 2-4x indexing
/// throughput on arm64, so it is not something to set casually.
pub fn kleidiai_disabled() -> bool {
    std::env::var("LRG_DISABLE_KLEIDIAI").as_deref() == Ok("1")
}

/// The one onnxruntime environment every session in this process is built under.
///
/// `ort` 2.0.0-rc.14 refuses a second `ort::init().build()` while the first
/// environment is alive ("only one environment is allowed per process"), and
/// each tower, the face models and the BioCLIP tower are built independently
/// and lazily, so they all have to share one. `Environment` is a cheap
/// reference-counted handle: each session keeps its clone, and the static
/// keeps the environment alive for the life of the process.
static ORT_ENVIRONMENT: std::sync::Mutex<Option<ort::environment::Environment>> =
    std::sync::Mutex::new(None);

/// The shared environment, created on first use.
pub(crate) fn ort_environment() -> ort::Result<ort::environment::Environment> {
    let mut slot = ORT_ENVIRONMENT.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(env) = slot.as_ref() {
        return Ok(env.clone());
    }
    let env = ort::init().build()?;
    *slot = Some(env.clone());
    Ok(env)
}

/// The graph optimization level every session in this crate is pinned to.
///
/// **Do not remove this pin, and do not raise it to `All`.** `Session::builder(..)`
/// sets no level, which leaves onnxruntime on its own default of
/// `ORT_ENABLE_ALL` — and at that level onnxruntime *segfaults* inside
/// `commit_from_file` on two of this project's five graphs. Not an error, not
/// an `ort::Error`: SIGSEGV in the optimizer, which takes the whole server
/// down with it and cannot be caught, logged, or surfaced to the user.
///
/// Measured by loading each model at each level on x86-64 Linux, resident
/// megabytes after load:
///
/// | model | `ENABLE_LAYOUT` (= `Level3`) | `ENABLE_ALL` |
/// |---|---|---|
/// | `bioclip2_image` (fp16 ViT) | 649 | **SIGSEGV** |
/// | `siglip2_text` (fp16) | 1974 | **SIGSEGV** |
/// | `siglip2_image` (fp16 ViT) | 882 | 2597 |
/// | `facenet_vggface2` (fp32) | 248 | 253 |
/// | `yunet_face_detection` (fp32) | 50 | 50 |
///
/// So the large fp16 transformer graphs either crash or cost 3x the memory,
/// and the small fp32 convolutional ones are indifferent. Reproduced
/// identically on onnxruntime 1.26.0, 1.27.0 and 1.28.0, and every model
/// passes `onnx.checker.check_model(full_check=True)` — this is a bug in
/// whatever `ORT_ENABLE_ALL` runs on top of the layout tier, not a bad export.
///
/// Nothing is given up by pinning here. `Level3` *is* `ORT_ENABLE_LAYOUT`,
/// onnxruntime's top documented tier, including the NCHWc memory-layout
/// transforms; `ORT_ENABLE_ALL` is the open-ended "and whatever else is
/// registered" level above it. The golden tests check the embeddings these
/// sessions produce against their PyTorch/open_clip references, so a level
/// that changed the numbers would fail rather than pass quietly.
pub(crate) const OPTIMIZATION_LEVEL: ort::session::builder::GraphOptimizationLevel =
    ort::session::builder::GraphOptimizationLevel::Level3;

/// The start of every session in this crate: the shared environment,
/// [`OPTIMIZATION_LEVEL`], and onnxruntime's automatic device selection put
/// back on the CPU.
///
/// `ort`'s `SessionBuilder::new` switches automatic execution provider
/// selection on with the `MaxEfficiency` policy (prefer an NPU), and it stays
/// on unless an EP is appended — which the CPU EP never does: registering it
/// only switches the arena off. Under onnxruntime 1.28 (`ort` rc.13) the CPU
/// was all that policy could find on a Mac. 1.31 (`ort` rc.14, backend 3.5.0)
/// offers CoreML as the NPU, so every model was split between the CPU and
/// CoreML without anyone asking for it: "CoreMLExecutionProvider::GetCapability
/// … number of nodes supported by CoreML: 3" of SigLIP2's 1199, 97 partitions
/// of BioCLIP's graph. Measured on an M4 Pro, inference was no faster, each
/// load took 1–2 s longer, and the log filled with CoreML warnings.
///
/// `AutoDevicePolicy::Default` is onnxruntime's "prefer CPU". An EP a builder
/// appends itself, like `LRG_ML_EP=coreml` in [`tower`], still takes precedence.
pub(crate) fn session_builder() -> ort::Result<ort::session::builder::SessionBuilder> {
    let env = ort_environment()?;
    Ok(ort::session::Session::builder(&env)?
        .with_auto_device(ort::session::builder::AutoDevicePolicy::Default)?
        .with_optimization_level(OPTIMIZATION_LEVEL)?)
}

/// Applies the KleidiAI policy above to a session builder.
pub(crate) fn apply_kleidiai_policy(
    builder: ort::session::builder::SessionBuilder,
) -> ort::Result<ort::session::builder::SessionBuilder> {
    if kleidiai_disabled() {
        log::warn!(
            "LRG_DISABLE_KLEIDIAI=1: disabling MLAS KleidiAI kernels, \
             expect 2-4x slower inference on arm64"
        );
        return Ok(builder.with_config_entry(DISABLE_KLEIDIAI, "1")?);
    }
    Ok(builder)
}

pub mod aesthetic;
pub mod auraface;
pub mod bioclip;
pub mod bioclip_pre;
pub mod clip_iqa;
pub mod cv2_resize;
pub mod face_quality;
pub mod facenet;
pub mod faces;
pub mod genre;
pub mod model_paths;
pub mod naflex_pre;
pub mod siglip;
pub mod text_pre;
pub(crate) mod tower;
pub mod umeyama;
pub mod yunet;
