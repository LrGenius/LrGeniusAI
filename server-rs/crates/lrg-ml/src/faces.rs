//! Top-level face pipeline: YuNet detect + FaceNet (or, opted into, AuraFace)
//! embed + quality metrics + base64 thumbnail.
//!
//! Both models replaced InsightFace's `buffalo_l` pack (SCRFD + ArcFace),
//! which could not be redistributed with this project. The pipeline's shape
//! is unchanged — detect, align on five landmarks, embed to 512 dims, score
//! quality — so everything downstream of [`FaceResult`] kept working; what
//! changed is that the identity embeddings live in a different space, which
//! [`MODEL_ID`] exists to make visible to the store.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use ort::session::Session;
use ort::value::Tensor;

use lrg_imaging::cull_config::FaceMetricsConfig;
use lrg_imaging::pil_resample::{resize_plane, Filter};

use crate::{auraface, face_quality, facenet, umeyama, yunet};

const IDLE_UNLOAD_SECONDS: i64 = 30 * 60;

/// Identifies the detector/recognizer pair that produced a stored face row.
///
/// Stamped onto every `FACE_TABLE` row and onto each photo's `faces_checked`
/// marker, mirroring what `species_model` does for BioCLIP. It is what makes
/// a model swap recoverable: FaceNet and the ArcFace embeddings before it are
/// both 512-d unit vectors, so nothing about a row's *shape* reveals which
/// model wrote it, and clustering a mixture of the two produces confident
/// nonsense rather than an error. Rows tagged with anything other than this
/// are ignored by clustering and re-queued for detection.
///
/// Bump this whenever a change would make new embeddings incomparable with
/// stored ones — a different model, different weights, or a change to the
/// alignment framing in [`crate::umeyama::norm_crop_facenet`].
pub const MODEL_ID: &str = "yunet-2023mar/facenet-vggface2";

/// [`MODEL_ID`] for the AuraFace recognizer. Same detector, so the boxes of a
/// re-detected photo line up with its FaceNet rows — which is what lets a
/// switch carry the user's names over (see the carry-over in `lrg-api`).
pub const AURAFACE_MODEL_ID: &str = "yunet-2023mar/auraface-v1";

/// Which network turns an aligned face into its identity embedding.
///
/// FaceNet ships with the default model download. AuraFace is the opt-in
/// alternative: sharper identities, a 250 MB download. The choice is global
/// to the backend and changes [`FaceModel::model_id`], so every stored face
/// from the other recognizer is set aside by clustering and its photo is
/// re-queued for detection until it is re-done.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Recognizer {
    #[default]
    FaceNet,
    AuraFace,
}

impl Recognizer {
    pub const ALL: [Recognizer; 2] = [Recognizer::FaceNet, Recognizer::AuraFace];

    /// The name used in the API and in the saved setting.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Recognizer::FaceNet => "facenet",
            Recognizer::AuraFace => "auraface",
        }
    }

    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "facenet" => Some(Recognizer::FaceNet),
            "auraface" => Some(Recognizer::AuraFace),
            _ => None,
        }
    }

    /// What a stored row from this recognizer is stamped with.
    #[must_use]
    pub fn model_id(self) -> &'static str {
        match self {
            Recognizer::FaceNet => MODEL_ID,
            Recognizer::AuraFace => AURAFACE_MODEL_ID,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum FaceError {
    #[error("ort error: {0}")]
    Ort(#[from] ort::Error),
    #[error("model not loaded")]
    NotLoaded,
    #[error("unexpected model output: {0}")]
    UnexpectedOutputs(String),
}

pub struct FaceModelPaths {
    /// YuNet detector (`face_detection_yunet_2023mar.onnx`).
    pub det_onnx: PathBuf,
    /// FaceNet recognizer (`facenet_vggface2.onnx`).
    pub rec_onnx: PathBuf,
    /// AuraFace recognizer (`auraface_glintr100.onnx`). Optional: only
    /// needed once the user switches to it.
    pub auraface_onnx: PathBuf,
}

impl FaceModelPaths {
    /// The default pair (YuNet + FaceNet) — what "Download AI models" fetches.
    pub fn all_exist(&self) -> bool {
        self.det_onnx.is_file() && self.rec_onnx.is_file()
    }

    /// The recognizer's weights.
    #[must_use]
    pub fn recognizer_onnx(&self, recognizer: Recognizer) -> &Path {
        match recognizer {
            Recognizer::FaceNet => &self.rec_onnx,
            Recognizer::AuraFace => &self.auraface_onnx,
        }
    }

    /// Whether the detector and `recognizer` are both on disk.
    #[must_use]
    pub fn ready_for(&self, recognizer: Recognizer) -> bool {
        self.det_onnx.is_file() && self.recognizer_onnx(recognizer).is_file()
    }
}

struct Loaded {
    det_session: Session,
    rec_session: Session,
    /// Which recognizer `rec_session` is. Checked on every use, so a switch
    /// between two photos reloads instead of embedding with the old network.
    recognizer: Recognizer,
    /// The recognizer's input name: FaceNet's export names it `input`,
    /// InsightFace-family graphs `input.1`, so it is read from the graph.
    rec_input: String,
}

pub struct FaceModel {
    paths: FaceModelPaths,
    recognizer: Mutex<Recognizer>,
    loaded: Mutex<Option<Loaded>>,
    last_used_unix: AtomicI64,
    load_error: Mutex<Option<String>>,
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// How much of the face pipeline to run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FacePass {
    /// Detection, FaceNet identity embedding, and a 112x112 JPEG thumbnail.
    /// Required for person clustering and the people UI.
    Full,
    /// Detection and the quality proxies only.
    ///
    /// Culling reads `sharpness`, `eye_openness`, `occlusion`, `det_score`,
    /// `area_ratio` and `center_proximity` — never the identity embedding or
    /// the thumbnail. Skipping those avoids one FaceNet session run *per
    /// detected face* — plus the second alignment warp it needs, three Lanczos
    /// plane resizes and a JPEG encode — which on a group shot is most of the
    /// per-photo face cost.
    QualityOnly,
}

#[derive(Debug, Clone)]
pub struct FaceResult {
    /// L2-normalized, 512-d. Empty under [`FacePass::QualityOnly`].
    pub embedding: Vec<f32>,
    /// Empty under [`FacePass::QualityOnly`].
    pub thumbnail_base64: String,
    pub bbox: [i64; 4],
    pub area_ratio: f64,
    pub sharpness: f64,
    pub det_score: f64,
    pub center_proximity: f64,
    pub eye_openness: f64,
    pub blink_penalty: f64,
    pub occlusion: f64,
}

fn round4(v: f64) -> f64 {
    (v * 10000.0).round_ties_even() / 10000.0
}

fn crop_rgb(
    pixels: &[u8],
    width: usize,
    x1: usize,
    y1: usize,
    x2: usize,
    y2: usize,
) -> (Vec<u8>, usize, usize) {
    let (cw, ch) = (x2 - x1, y2 - y1);
    let mut out = vec![0u8; cw * ch * 3];
    for y in 0..ch {
        let src = ((y1 + y) * width + x1) * 3;
        let dst = y * cw * 3;
        out[dst..dst + cw * 3].copy_from_slice(&pixels[src..src + cw * 3]);
    }
    (out, cw, ch)
}

fn thumbnail_112_jpeg(crop: &[u8], cw: usize, ch: usize) -> String {
    let mut planes = [Vec::new(), Vec::new(), Vec::new()];
    for (c, plane) in planes.iter_mut().enumerate() {
        let p: Vec<u8> = (0..cw * ch).map(|i| crop[i * 3 + c]).collect();
        *plane = resize_plane(&p, cw, ch, 112, 112, Filter::Lanczos3);
    }
    let mut rgb = vec![0u8; 112 * 112 * 3];
    for i in 0..112 * 112 {
        rgb[i * 3] = planes[0][i];
        rgb[i * 3 + 1] = planes[1][i];
        rgb[i * 3 + 2] = planes[2][i];
    }
    let img = image::RgbImage::from_raw(112, 112, rgb).expect("sized buffer");
    let mut buf = std::io::Cursor::new(Vec::new());
    let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, 85);
    img.write_with_encoder(encoder).expect("jpeg encode");
    base64::engine::general_purpose::STANDARD.encode(buf.into_inner())
}

/// Disables ONNX Runtime's CPU arena allocator. The arena's default
/// extend strategy only ever grows (each `Session::run` can mint a new,
/// larger chunk instead of reusing freed ones) and never returns memory
/// to the OS — a well-documented onnxruntime behavior, confirmed here by
/// per-photo RSS logging showing a ~30-40MB step on every single
/// `detect_faces` call that never comes back down. Falling back to plain
/// malloc/free per allocation is slower per call but keeps steady-state
/// memory bounded, which matters far more for a long indexing run.
///
/// The detector is convolution-heavy, so it used to be the single largest source
/// of memory growth during indexing via onnxruntime's leaking KleidiAI
/// kernels. That leak is gone as of onnxruntime 1.28 and these sessions run
/// with KleidiAI enabled again — see [`crate::DISABLE_KLEIDIAI`] for the
/// measurements and the `LRG_DISABLE_KLEIDIAI` escape hatch.
fn build_session(path: &Path) -> ort::Result<Session> {
    let builder = crate::session_builder()?
        .with_execution_providers([ort::ep::CPU::default().with_arena_allocator(false).build()])?;
    crate::apply_kleidiai_policy(builder)?.commit_from_file(path)
}

impl FaceModel {
    pub fn new(paths: FaceModelPaths) -> Self {
        FaceModel {
            paths,
            recognizer: Mutex::new(Recognizer::default()),
            loaded: Mutex::new(None),
            last_used_unix: AtomicI64::new(0),
            load_error: Mutex::new(None),
        }
    }

    /// Whether the detector and the *active* recognizer are on disk.
    pub fn is_cached(&self) -> bool {
        self.paths.ready_for(self.recognizer())
    }

    /// The model files, for the routes that report or download them.
    #[must_use]
    pub fn paths(&self) -> &FaceModelPaths {
        &self.paths
    }

    /// The recognizer new embeddings are made with.
    #[must_use]
    pub fn recognizer(&self) -> Recognizer {
        *self.recognizer.lock().unwrap()
    }

    /// Switches the recognizer. The sessions of the old one are dropped, and
    /// [`FaceModel::model_id`] changes with it. Refused when the recognizer's
    /// weights are not on disk: a switch that fails on the next photo would
    /// leave every run reporting a missing model.
    pub fn set_recognizer(&self, recognizer: Recognizer) -> Result<(), String> {
        if !self.paths.recognizer_onnx(recognizer).is_file() {
            return Err(format!(
                "the {} model is not downloaded ({} is missing)",
                recognizer.as_str(),
                self.paths.recognizer_onnx(recognizer).display()
            ));
        }
        let mut current = self.recognizer.lock().unwrap();
        if *current != recognizer {
            log::info!(
                "Face recognizer: {} -> {}",
                current.as_str(),
                recognizer.as_str()
            );
            *current = recognizer;
            drop(current);
            self.unload_with_reason("recognizer switched");
        }
        Ok(())
    }

    /// Loads the sessions for the active recognizer into `guard` unless they
    /// are already there. Takes the guard so [`FaceModel::detect_faces_with_model`]
    /// loads and runs under one lock: a switch can then never land between
    /// the two and leave a photo embedded by one network and stamped with the
    /// other's id.
    fn load_into(&self, guard: &mut Option<Loaded>) -> Result<(), FaceError> {
        let recognizer = self.recognizer();
        if guard.as_ref().is_some_and(|l| l.recognizer == recognizer) {
            self.last_used_unix.store(now_unix(), Ordering::Relaxed);
            return Ok(());
        }
        *guard = None;
        match (
            build_session(&self.paths.det_onnx),
            build_session(self.paths.recognizer_onnx(recognizer)),
        ) {
            (Ok(det_session), Ok(rec_session)) => {
                let rec_input = rec_session
                    .inputs()
                    .first()
                    .map(|i| i.name().to_string())
                    .ok_or_else(|| {
                        FaceError::UnexpectedOutputs("recognizer has no input".to_string())
                    })?;
                *guard = Some(Loaded {
                    det_session,
                    rec_session,
                    recognizer,
                    rec_input,
                });
                *self.load_error.lock().unwrap() = None;
                self.last_used_unix.store(now_unix(), Ordering::Relaxed);
                Ok(())
            }
            (Err(e), _) | (_, Err(e)) => {
                *self.load_error.lock().unwrap() = Some(e.to_string());
                Err(e.into())
            }
        }
    }

    /// Drops the loaded session(s). Safe to call when nothing is loaded.
    ///
    /// The reason is a parameter because this is reached two ways — the
    /// `/unload` route and the idle sweep — and the log line used to say
    /// "due to inactivity" for both. A real `/unload` from the plugin then
    /// looked like an idle timeout in the log, which is the kind of small lie
    /// that costs an hour the next time someone reads it.
    fn unload_with_reason(&self, reason: &str) {
        let mut guard = self.loaded.lock().unwrap();
        if guard.take().is_some() {
            log::info!("Unloading face models ({reason})");
        }
    }

    pub fn unload(&self) {
        self.unload_with_reason("requested");
    }

    pub fn unload_if_idle(&self) {
        let last = self.last_used_unix.load(Ordering::Relaxed);
        if last != 0 && now_unix() - last >= IDLE_UNLOAD_SECONDS {
            self.unload_with_reason("idle");
        }
    }

    /// Which detector/recognizer pair this build writes, for stamping onto
    /// stored rows. See [`MODEL_ID`].
    ///
    /// Unlike BioCLIP's `model_id`, this is a compile-time constant rather
    /// than something read off disk: the face models carry no version metadata
    /// of their own, and the paths are fixed filenames, so the only thing that
    /// can change which embeddings this build produces is this build.
    pub fn model_id(&self) -> &'static str {
        self.recognizer().model_id()
    }

    pub fn status(&self) -> (&'static str, Option<String>) {
        if self.loaded.lock().unwrap().is_some() {
            ("loaded", None)
        } else if let Some(err) = self.load_error.lock().unwrap().clone() {
            ("failed", Some(err))
        } else {
            ("not_loaded", None)
        }
    }

    /// Port of `detect_faces`: RGB8 image in, quality-scored face
    /// records out (embedding, thumbnail, bbox, quality proxies).
    pub fn detect_faces(
        &self,
        pixels: &[u8],
        width: usize,
        height: usize,
        cfg: &FaceMetricsConfig,
        pass: FacePass,
    ) -> Result<Vec<FaceResult>, FaceError> {
        self.detect_faces_with_model(pixels, width, height, cfg, pass)
            .map(|(faces, _)| faces)
    }

    /// [`FaceModel::detect_faces`], plus the [`Recognizer::model_id`] the
    /// embeddings were made with — what the caller stamps on stored rows.
    /// Read from the sessions that actually ran rather than from
    /// [`FaceModel::model_id`] afterwards, which a concurrent switch could
    /// already have changed.
    pub fn detect_faces_with_model(
        &self,
        pixels: &[u8],
        width: usize,
        height: usize,
        cfg: &FaceMetricsConfig,
        pass: FacePass,
    ) -> Result<(Vec<FaceResult>, &'static str), FaceError> {
        let mut guard = self.loaded.lock().unwrap();
        self.load_into(&mut guard)?;
        let loaded = guard.as_mut().ok_or(FaceError::NotLoaded)?;
        let model_id = loaded.recognizer.model_id();

        let pre = yunet::preprocess(pixels, width, height);
        let input = Tensor::from_array(([1, 3, 640, 640], pre.blob))?;
        let outputs = loaded.det_session.run(ort::inputs!["input" => input])?;
        // Fetched by name, not by index. The YuNet graph declares its twelve
        // outputs as cls/obj/bbox/kps per stride, and `decode_outputs` needs
        // them in exactly that grouping; positional indexing would depend on
        // the exporter's ordering, and a silently permuted set decodes into
        // plausible boxes in the wrong places rather than failing.
        let mut flat = Vec::with_capacity(yunet::NUM_OUTPUTS);
        for name in yunet::OUTPUT_NAMES {
            let value = outputs.get(name).ok_or_else(|| {
                FaceError::UnexpectedOutputs(format!("detector has no output named `{name}`"))
            })?;
            let (_, data) = value.try_extract_tensor::<f32>()?;
            flat.push(data);
        }
        let refs: [&[f32]; yunet::NUM_OUTPUTS] = std::array::from_fn(|i| flat[i]);
        let detections = yunet::decode_outputs(&refs, pre.det_scale);

        let image_area = (width * height).max(1) as f64;
        let mut results = Vec::with_capacity(detections.len());

        for det in &detections {
            let kps_f64: [[f64; 2]; 5] = det.kps.map(|p| [p[0] as f64, p[1] as f64]);

            let x1 = (det.bbox[0].round() as i64).max(0).min(width as i64);
            let y1 = (det.bbox[1].round() as i64).max(0).min(height as i64);
            let x2 = (det.bbox[2].round() as i64).max(0).min(width as i64);
            let y2 = (det.bbox[3].round() as i64).max(0).min(height as i64);
            // Checked before the recognition pass rather than after it, so a
            // degenerate box no longer costs a FaceNet run it then discards.
            if x2 <= x1 || y2 <= y1 {
                continue;
            }

            // The canonical 112x112 crop, needed on *both* passes: the
            // occlusion measure further down is a mirror-symmetry test, which
            // is only meaningful once the face has been rotated and scaled onto
            // the canonical template. A 112x112 bilinear sample is three orders
            // of magnitude cheaper than the inference `QualityOnly` skips.
            let aligned = umeyama::norm_crop_112(pixels, width, height, &kps_f64);

            let embedding = match pass {
                FacePass::Full => {
                    let (blob, dim, embed_dim) = match loaded.recognizer {
                        // A second warp rather than a resize of `aligned`:
                        // FaceNet wants a wider framing than ArcFace did (see
                        // `norm_crop_facenet`), and context outside the 112
                        // crop cannot be recovered by upscaling it.
                        // `QualityOnly` never pays for this, so the cull pass
                        // costs exactly what it did.
                        Recognizer::FaceNet => {
                            let wide = umeyama::norm_crop_facenet(
                                pixels,
                                width,
                                height,
                                &kps_f64,
                                facenet::INPUT_SIZE,
                            );
                            (
                                facenet::to_blob(&wide),
                                facenet::INPUT_SIZE,
                                facenet::EMBED_DIM,
                            )
                        }
                        // AuraFace is an ArcFace-family network: it takes
                        // exactly the canonical crop made above.
                        Recognizer::AuraFace => (
                            auraface::to_blob(&aligned),
                            auraface::INPUT_SIZE,
                            auraface::EMBED_DIM,
                        ),
                    };
                    let rec_input = Tensor::from_array(([1, 3, dim, dim], blob))?;
                    let rec_out = loaded
                        .rec_session
                        .run(ort::inputs![loaded.rec_input.as_str() => rec_input])?;
                    let (_, raw_emb) = rec_out[0].try_extract_tensor::<f32>()?;
                    if raw_emb.len() != embed_dim {
                        return Err(FaceError::UnexpectedOutputs(format!(
                            "recognizer returned {} dims, expected {embed_dim}",
                            raw_emb.len(),
                        )));
                    }
                    let mut embedding = raw_emb.to_vec();
                    // FaceNet normalizes its own output, so this is a no-op on
                    // it; AuraFace (like every ArcFace export) does not, and
                    // cosine distance downstream assumes unit vectors.
                    crate::siglip::l2_normalize(&mut embedding);
                    embedding
                }
                FacePass::QualityOnly => Vec::new(),
            };
            let (cw, ch) = ((x2 - x1) as usize, (y2 - y1) as usize);
            let (crop, _, _) = crop_rgb(
                pixels,
                width,
                x1 as usize,
                y1 as usize,
                x2 as usize,
                y2 as usize,
            );

            let area_ratio = (((x2 - x1) * (y2 - y1)) as f64 / image_area).clamp(0.0, 1.0);
            let sharpness = face_quality::face_sharpness(&crop, cw, ch, cfg);
            let eye_openness =
                face_quality::eye_openness_proxy(&crop, cw, ch, [x1, y1, x2, y2], &kps_f64, cfg);

            let center_x = (x1 + x2) as f64 / 2.0;
            let center_y = (y1 + y2) as f64 / 2.0;
            let offset_x = ((center_x / (width.max(1) as f64)) - 0.5).abs() * 2.0;
            let offset_y = ((center_y / (height.max(1) as f64)) - 0.5).abs() * 2.0;
            let center_proximity = (1.0 - (offset_x + offset_y) / 2.0).clamp(0.0, 1.0);

            let det_score = det.score as f64;
            let occlusion = face_quality::occlusion_from_crop(&aligned, 112, &kps_f64, cfg);
            let thumbnail_base64 = match pass {
                FacePass::Full => thumbnail_112_jpeg(&crop, cw, ch),
                FacePass::QualityOnly => String::new(),
            };

            results.push(FaceResult {
                embedding,
                thumbnail_base64,
                bbox: [x1, y1, x2, y2],
                area_ratio: round4(area_ratio),
                sharpness: round4(sharpness),
                det_score,
                center_proximity: round4(center_proximity),
                eye_openness: round4(eye_openness),
                blink_penalty: round4((1.0 - eye_openness).clamp(0.0, 1.0)),
                occlusion: round4(occlusion),
            });
        }
        Ok((results, model_id))
    }
}

#[cfg(test)]
mod recognizer_tests {
    use super::*;

    fn paths(dir: &Path) -> FaceModelPaths {
        FaceModelPaths {
            det_onnx: dir.join("det.onnx"),
            rec_onnx: dir.join("facenet.onnx"),
            auraface_onnx: dir.join("auraface.onnx"),
        }
    }

    #[test]
    fn names_round_trip_and_ids_differ() {
        for r in Recognizer::ALL {
            assert_eq!(Recognizer::parse(r.as_str()), Some(r));
        }
        assert_eq!(Recognizer::parse(" AuraFace\n"), Some(Recognizer::AuraFace));
        assert_eq!(Recognizer::parse("arcface"), None);
        assert_ne!(
            Recognizer::FaceNet.model_id(),
            Recognizer::AuraFace.model_id()
        );
    }

    /// The stamp on stored rows follows the switch; that is what keeps the
    /// two embedding spaces from being compared.
    #[test]
    fn switching_changes_the_model_id_and_needs_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let model = FaceModel::new(paths(dir.path()));
        assert_eq!(model.model_id(), MODEL_ID);
        assert!(model.set_recognizer(Recognizer::AuraFace).is_err());
        assert_eq!(model.recognizer(), Recognizer::FaceNet);

        std::fs::write(dir.path().join("auraface.onnx"), b"weights").unwrap();
        model.set_recognizer(Recognizer::AuraFace).unwrap();
        assert_eq!(model.model_id(), AURAFACE_MODEL_ID);
        // Readiness is the active recognizer's: no detector here yet.
        assert!(!model.is_cached());
        std::fs::write(dir.path().join("det.onnx"), b"weights").unwrap();
        assert!(model.is_cached());
    }
}
