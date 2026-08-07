//! Top-level face pipeline: SCRFD detect + ArcFace embed + quality
//! metrics + base64 thumbnail. Port of `services/face.py::detect_faces`
//! (the model-management half mirrors `_get_face_app`/`unload_face_app`).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use ort::session::Session;
use ort::value::Tensor;

use lrg_imaging::pil_resample::{resize_plane, Filter};

use crate::{arcface, face_quality, scrfd, umeyama};

const IDLE_UNLOAD_SECONDS: i64 = 30 * 60;

#[derive(Debug, thiserror::Error)]
pub enum FaceError {
    #[error("ort error: {0}")]
    Ort(#[from] ort::Error),
    #[error("model not loaded")]
    NotLoaded,
}

pub struct FaceModelPaths {
    pub det_onnx: PathBuf,
    pub rec_onnx: PathBuf,
}

impl FaceModelPaths {
    pub fn all_exist(&self) -> bool {
        self.det_onnx.is_file() && self.rec_onnx.is_file()
    }
}

struct Loaded {
    det_session: Session,
    rec_session: Session,
}

pub struct FaceModel {
    paths: FaceModelPaths,
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

#[derive(Debug, Clone)]
pub struct FaceResult {
    pub embedding: Vec<f32>, // L2-normalized, 512-d
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
/// SCRFD is convolution-heavy, so it used to be the single largest source
/// of memory growth during indexing via onnxruntime's leaking KleidiAI
/// kernels. That leak is gone as of onnxruntime 1.28 and these sessions run
/// with KleidiAI enabled again — see [`crate::DISABLE_KLEIDIAI`] for the
/// measurements and the `LRG_DISABLE_KLEIDIAI` escape hatch.
fn build_session(path: &Path) -> ort::Result<Session> {
    let builder = Session::builder()?
        .with_execution_providers([ort::ep::CPU::default().with_arena_allocator(false).build()])?;
    crate::apply_kleidiai_policy(builder)?.commit_from_file(path)
}

impl FaceModel {
    pub fn new(paths: FaceModelPaths) -> Self {
        FaceModel {
            paths,
            loaded: Mutex::new(None),
            last_used_unix: AtomicI64::new(0),
            load_error: Mutex::new(None),
        }
    }

    pub fn is_cached(&self) -> bool {
        self.paths.all_exist()
    }

    fn ensure_loaded(&self) -> Result<(), FaceError> {
        let mut guard = self.loaded.lock().unwrap();
        if guard.is_some() {
            self.last_used_unix.store(now_unix(), Ordering::Relaxed);
            return Ok(());
        }
        match (
            build_session(&self.paths.det_onnx),
            build_session(&self.paths.rec_onnx),
        ) {
            (Ok(det_session), Ok(rec_session)) => {
                *guard = Some(Loaded {
                    det_session,
                    rec_session,
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

    pub fn unload(&self) {
        let mut guard = self.loaded.lock().unwrap();
        if guard.take().is_some() {
            log::info!("Unloading InsightFace model due to inactivity...");
        }
    }

    pub fn unload_if_idle(&self) {
        let last = self.last_used_unix.load(Ordering::Relaxed);
        if last != 0 && now_unix() - last >= IDLE_UNLOAD_SECONDS {
            self.unload();
        }
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
    ) -> Result<Vec<FaceResult>, FaceError> {
        self.ensure_loaded()?;
        let mut guard = self.loaded.lock().unwrap();
        let loaded = guard.as_mut().ok_or(FaceError::NotLoaded)?;

        let pre = scrfd::preprocess(pixels, width, height);
        let input = Tensor::from_array(([1, 3, 640, 640], pre.blob))?;
        let outputs = loaded.det_session.run(ort::inputs!["input.1" => input])?;
        // Output order matches det_10g.onnx's declared names (verified
        // via onnxruntime metadata): scores@8,16,32; bbox@8,16,32; kps@8,16,32.
        let mut flat = Vec::with_capacity(9);
        for i in 0..9 {
            let (_, data) = outputs[i].try_extract_tensor::<f32>()?;
            flat.push(data);
        }
        let refs: [&[f32]; 9] = std::array::from_fn(|i| flat[i]);
        let detections = scrfd::decode_outputs(&refs, pre.det_scale);

        let image_area = (width * height).max(1) as f64;
        let mut results = Vec::with_capacity(detections.len());

        for det in &detections {
            let kps_f64: [[f64; 2]; 5] = det.kps.map(|p| [p[0] as f64, p[1] as f64]);
            let aligned = umeyama::norm_crop_112(pixels, width, height, &kps_f64);
            let blob = arcface::to_blob(&aligned);
            let rec_input = Tensor::from_array(([1, 3, 112, 112], blob))?;
            let rec_out = loaded
                .rec_session
                .run(ort::inputs!["input.1" => rec_input])?;
            let (_, raw_emb) = rec_out[0].try_extract_tensor::<f32>()?;
            let mut embedding = raw_emb.to_vec();
            crate::siglip::l2_normalize(&mut embedding);

            let x1 = (det.bbox[0].round() as i64).max(0).min(width as i64);
            let y1 = (det.bbox[1].round() as i64).max(0).min(height as i64);
            let x2 = (det.bbox[2].round() as i64).max(0).min(width as i64);
            let y2 = (det.bbox[3].round() as i64).max(0).min(height as i64);
            if x2 <= x1 || y2 <= y1 {
                continue;
            }
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
            let sharpness = face_quality::face_sharpness(&crop, cw, ch);
            let eye_openness =
                face_quality::eye_openness_proxy(&crop, cw, ch, [x1, y1, x2, y2], &kps_f64);

            let center_x = (x1 + x2) as f64 / 2.0;
            let center_y = (y1 + y2) as f64 / 2.0;
            let offset_x = ((center_x / (width.max(1) as f64)) - 0.5).abs() * 2.0;
            let offset_y = ((center_y / (height.max(1) as f64)) - 0.5).abs() * 2.0;
            let center_proximity = (1.0 - (offset_x + offset_y) / 2.0).clamp(0.0, 1.0);

            let det_score = det.score as f64;
            let occlusion =
                face_quality::occlusion_proxy(det_score, center_proximity, eye_openness);
            let thumbnail_base64 = thumbnail_112_jpeg(&crop, cw, ch);

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
        Ok(results)
    }
}
