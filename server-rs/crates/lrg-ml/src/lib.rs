//! ONNX inference: SigLIP2 image/text embedding via `ort`. Face pipeline
//! (SCRFD + ArcFace) lands in M5.

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
/// | SCRFD + ArcFace | ~68 ms | ~140 ms | 2.1x |
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

pub mod arcface;
pub mod clip_iqa;
pub mod cv2_resize;
pub mod face_quality;
pub mod faces;
pub mod image_pre;
pub mod model_paths;
pub mod scrfd;
pub mod siglip;
pub mod text_pre;
pub mod umeyama;
