//! Eye state and expression per face: MediaPipe Face Mesh V2 (478 landmarks)
//! and Blendshape V2 (52 expression coefficients), both Apache-2.0, exported to
//! ONNX by `scripts/export_face_state.py`.
//!
//! Replaces what the culling ranking used to know about eyes, which was a
//! vertical-gradient proxy in a patch of at most 17×17 px around YuNet's eye
//! point (`face_quality::eye_openness_proxy`). On the first real shoot it read
//! "closed" on 767 of 769 frames, because eyelashes and brows have vertical
//! gradients too and small faces have no eye to look at. The blendshape model
//! answers the question it was trained on — `eyeBlinkLeft` 0.04 on an open eye
//! of the reference portrait, 0.81 with the eyelids moved shut — and also
//! answers the ones the proxy never could: squinting, the smile, where the
//! eyes look, and how far the head is turned.
//!
//! The pipeline mirrors MediaPipe's FaceLandmarker, whose sources fix every
//! constant here: a square region 1.5× the face, rotated so the eyes are level
//! (`DetectionsToRects`/`RectTransformation`), warped to 256×256 in `0..1` with
//! the border replicated (`ImageToTensor`); the mesh's landmarks projected back
//! through the same region; 146 of them, in image pixels, into the blendshape
//! model, which normalizes scale, rotation and translation itself. The region
//! is computed twice: from YuNet's box and eyes, then from the mesh's own
//! landmarks — MediaPipe's tracking mode, which its model card names as the
//! mode the model is accurate in (it tolerates 10% shift and scale and 8° of
//! roll in its input). Checked against Python MediaPipe on the reference
//! portrait: landmarks within ~1% of the interocular distance, blink and smile
//! within 0.01 (`tests/face_state_goldens.rs`).
//!
//! The model cards also say where the models stop working: a face turned more
//! than 80° away, less than half visible, or too small to rescale to the input
//! without loss. Such a face gets [`EyeState::Unknown`] and abstains from
//! every decision that reads the eyes.

use std::path::PathBuf;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use ort::session::Session;
use ort::value::Tensor;

use lrg_imaging::cull_config::FaceMetricsConfig;

/// Stamped on every stored face state (`cull_face_state_model`). Bump when a
/// change would make new readings incomparable with stored ones.
pub const MODEL_ID: &str = "mediapipe-facemesh-v2+blendshape-v2/1";

pub const MESH_INPUT_SIZE: usize = 256;
pub const LANDMARK_COUNT: usize = 478;
pub const BLENDSHAPE_COUNT: usize = 52;

const IDLE_UNLOAD_SECONDS: i64 = 30 * 60;

/// The landmarks the blendshape model reads, in its order. From
/// `face_blendshapes_graph.cc` (`kLandmarksSubsetIdxs`); the golden test checks
/// it against the copy `export_face_state.py` records.
pub const BLENDSHAPE_LANDMARKS: [usize; 146] = [
    0, 1, 4, 5, 6, 7, 8, 10, 13, 14, 17, 21, 33, 37, 39, 40, 46, 52, 53, 54, 55, 58, 61, 63, 65,
    66, 67, 70, 78, 80, 81, 82, 84, 87, 88, 91, 93, 95, 103, 105, 107, 109, 127, 132, 133, 136,
    144, 145, 146, 148, 149, 150, 152, 153, 154, 155, 157, 158, 159, 160, 161, 162, 163, 168, 172,
    173, 176, 178, 181, 185, 191, 195, 197, 234, 246, 249, 251, 263, 267, 269, 270, 276, 282, 283,
    284, 285, 288, 291, 293, 295, 296, 297, 300, 308, 310, 311, 312, 314, 317, 318, 321, 323, 324,
    332, 334, 336, 338, 356, 361, 362, 365, 373, 374, 375, 377, 378, 379, 380, 381, 382, 384, 385,
    386, 387, 388, 389, 390, 397, 398, 400, 402, 405, 409, 415, 454, 466, 468, 469, 470, 471, 472,
    473, 474, 475, 476, 477,
];

/// The blendshape model's outputs, in order (`kBlendshapeNames`).
pub const BLENDSHAPE_NAMES: [&str; BLENDSHAPE_COUNT] = [
    "_neutral",
    "browDownLeft",
    "browDownRight",
    "browInnerUp",
    "browOuterUpLeft",
    "browOuterUpRight",
    "cheekPuff",
    "cheekSquintLeft",
    "cheekSquintRight",
    "eyeBlinkLeft",
    "eyeBlinkRight",
    "eyeLookDownLeft",
    "eyeLookDownRight",
    "eyeLookInLeft",
    "eyeLookInRight",
    "eyeLookOutLeft",
    "eyeLookOutRight",
    "eyeLookUpLeft",
    "eyeLookUpRight",
    "eyeSquintLeft",
    "eyeSquintRight",
    "eyeWideLeft",
    "eyeWideRight",
    "jawForward",
    "jawLeft",
    "jawOpen",
    "jawRight",
    "mouthClose",
    "mouthDimpleLeft",
    "mouthDimpleRight",
    "mouthFrownLeft",
    "mouthFrownRight",
    "mouthFunnel",
    "mouthLeft",
    "mouthLowerDownLeft",
    "mouthLowerDownRight",
    "mouthPressLeft",
    "mouthPressRight",
    "mouthPucker",
    "mouthRight",
    "mouthRollLower",
    "mouthRollUpper",
    "mouthShrugLower",
    "mouthShrugUpper",
    "mouthSmileLeft",
    "mouthSmileRight",
    "mouthStretchLeft",
    "mouthStretchRight",
    "mouthUpperUpLeft",
    "mouthUpperUpRight",
    "noseSneerLeft",
    "noseSneerRight",
];

const EYE_BLINK_LEFT: usize = 9;
const EYE_BLINK_RIGHT: usize = 10;
const EYE_LOOK_FIRST: usize = 11;
const EYE_LOOK_LAST: usize = 18;
const EYE_SQUINT_LEFT: usize = 19;
const EYE_SQUINT_RIGHT: usize = 20;
const MOUTH_SMILE_LEFT: usize = 44;
const MOUTH_SMILE_RIGHT: usize = 45;

// Mesh landmarks. "Right" and "left" are the subject's, as in MediaPipe: the
// right eye is the one on the left of the image of a frontal face.
const RIGHT_EYE_OUTER: usize = 33;
const RIGHT_EYE_INNER: usize = 133;
const LEFT_EYE_INNER: usize = 362;
const LEFT_EYE_OUTER: usize = 263;
const RIGHT_FACE_EDGE: usize = 234;
const LEFT_FACE_EDGE: usize = 454;

#[derive(Debug, thiserror::Error)]
pub enum FaceStateError {
    #[error("ort error: {0}")]
    Ort(#[from] ort::Error),
    #[error("model not loaded")]
    NotLoaded,
    #[error("unexpected model output: {0}")]
    UnexpectedOutputs(String),
}

#[derive(Debug, Clone)]
pub struct FaceStatePaths {
    pub mesh_onnx: PathBuf,
    pub blendshapes_onnx: PathBuf,
}

impl FaceStatePaths {
    pub fn all_exist(&self) -> bool {
        self.mesh_onnx.is_file() && self.blendshapes_onnx.is_file()
    }
}

/// What the eyes of one face show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EyeState {
    Open,
    /// Half closed: squinting into the sun, mid-blink, a sleepy look.
    Half,
    Closed,
    /// The model cannot say: the face is turned too far, barely a face, or
    /// too small. Abstains from every eye decision.
    Unknown,
}

impl EyeState {
    pub fn as_str(self) -> &'static str {
        match self {
            EyeState::Open => "open",
            EyeState::Half => "half",
            EyeState::Closed => "closed",
            EyeState::Unknown => "unknown",
        }
    }

    pub fn parse(s: &str) -> Option<EyeState> {
        match s {
            "open" => Some(EyeState::Open),
            "half" => Some(EyeState::Half),
            "closed" => Some(EyeState::Closed),
            "unknown" => Some(EyeState::Unknown),
            _ => None,
        }
    }
}

/// One face's state, from the mesh and the blendshapes.
#[derive(Debug, Clone)]
pub struct FaceState {
    /// Probability that the region holds a face at all (the mesh's own
    /// presence output, through a sigmoid as MediaPipe applies it).
    pub presence: f64,
    /// How closed the eyes are, `0..1`: the mean of `eyeBlinkLeft/Right`, or
    /// only the near eye's when the head is turned so far that the far one is
    /// hidden ([`FaceMetricsConfig::face_state_near_eye_yaw_degrees`]).
    pub eye_closed: f64,
    pub eye_squint: f64,
    /// Mean of `mouthSmileLeft/Right`.
    pub smile: f64,
    /// The strongest of the eight `eyeLook*` coefficients: 0 is looking into
    /// the lens.
    pub gaze_away: f64,
    /// Head turn in degrees from frontal, from the depth of the two face
    /// edges; positive turns the subject's left cheek away.
    pub yaw_degrees: f64,
    pub eye_state: EyeState,
    pub blendshapes: [f32; BLENDSHAPE_COUNT],
    /// The 478 landmarks in pixels of the image analysed (`z` scaled like
    /// `x`).
    pub landmarks: Vec<[f64; 3]>,
}

/// The square, rotated region the mesh model sees, in image pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Roi {
    pub cx: f64,
    pub cy: f64,
    pub size: f64,
    /// Radians, clockwise in image coordinates (y down), as MediaPipe's rect
    /// rotation: the angle that makes the eye line level.
    pub rotation: f64,
}

fn normalize_radians(a: f64) -> f64 {
    use std::f64::consts::PI;
    a - 2.0 * PI * ((a + PI) / (2.0 * PI)).floor()
}

/// `DetectionsToRectsCalculator::ComputeRotation` with a target angle of 0.
fn rotation_between(start: [f64; 2], end: [f64; 2]) -> f64 {
    normalize_radians(0.0 - (-(end[1] - start[1])).atan2(end[0] - start[0]))
}

impl Roi {
    /// From a detector box and its eye points (YuNet's first two keypoints:
    /// the eye on the left of the image, then the other), squared on the long
    /// side and grown 1.5×.
    pub fn from_detection(bbox: [f64; 4], kps: &[[f64; 2]; 5]) -> Roi {
        let (w, h) = (bbox[2] - bbox[0], bbox[3] - bbox[1]);
        Roi {
            cx: (bbox[0] + bbox[2]) / 2.0,
            cy: (bbox[1] + bbox[3]) / 2.0,
            size: w.max(h) * 1.5,
            rotation: rotation_between(kps[0], kps[1]),
        }
    }

    /// From the mesh's own landmarks, as MediaPipe's tracking mode does: their
    /// bounding box squared and grown 1.5×, rotated by the outer eye corners.
    pub fn from_landmarks(landmarks: &[[f64; 3]]) -> Roi {
        let (mut x0, mut y0) = (f64::INFINITY, f64::INFINITY);
        let (mut x1, mut y1) = (f64::NEG_INFINITY, f64::NEG_INFINITY);
        for p in landmarks {
            x0 = x0.min(p[0]);
            y0 = y0.min(p[1]);
            x1 = x1.max(p[0]);
            y1 = y1.max(p[1]);
        }
        let s = landmarks[RIGHT_EYE_OUTER];
        let e = landmarks[LEFT_EYE_OUTER];
        Roi {
            cx: (x0 + x1) / 2.0,
            cy: (y0 + y1) / 2.0,
            size: (x1 - x0).max(y1 - y0) * 1.5,
            rotation: rotation_between([s[0], s[1]], [e[0], e[1]]),
        }
    }

    /// The image point at `(u, v)` in the region, both `0..1` across it.
    pub fn to_image(&self, u: f64, v: f64) -> [f64; 2] {
        let (lx, ly) = ((u - 0.5) * self.size, (v - 0.5) * self.size);
        let (c, s) = (self.rotation.cos(), self.rotation.sin());
        [self.cx + lx * c - ly * s, self.cy + lx * s + ly * c]
    }
}

/// The region warped to `n`×`n` RGB in `0..1`, NHWC, bilinear, border
/// replicated (`ImageToTensorCalculator`'s default).
pub fn warp_roi(rgb: &[u8], width: usize, height: usize, roi: &Roi, n: usize) -> Vec<f32> {
    let mut out = vec![0f32; n * n * 3];
    if width == 0 || height == 0 || rgb.len() < width * height * 3 {
        return out;
    }
    let (maxx, maxy) = ((width - 1) as f64, (height - 1) as f64);
    for j in 0..n {
        let v = (j as f64 + 0.5) / n as f64;
        for i in 0..n {
            let u = (i as f64 + 0.5) / n as f64;
            let p = roi.to_image(u, v);
            // Pixel centres sit at +0.5.
            let x = (p[0] - 0.5).clamp(0.0, maxx);
            let y = (p[1] - 0.5).clamp(0.0, maxy);
            let (x0, y0) = (x.floor() as usize, y.floor() as usize);
            let (x1, y1) = ((x0 + 1).min(width - 1), (y0 + 1).min(height - 1));
            let (fx, fy) = (x - x0 as f64, y - y0 as f64);
            for c in 0..3 {
                let at = |xx: usize, yy: usize| rgb[(yy * width + xx) * 3 + c] as f64;
                let value = at(x0, y0) * (1.0 - fx) * (1.0 - fy)
                    + at(x1, y0) * fx * (1.0 - fy)
                    + at(x0, y1) * (1.0 - fx) * fy
                    + at(x1, y1) * fx * fy;
                out[(j * n + i) * 3 + c] = (value / 255.0) as f32;
            }
        }
    }
    out
}

/// Mesh output (pixels of the `n`×`n` input) back to image pixels.
fn project(raw: &[f32], roi: &Roi, n: usize) -> Vec<[f64; 3]> {
    raw.as_chunks::<3>()
        .0
        .iter()
        .map(|p| {
            let (u, v) = (p[0] as f64 / n as f64, p[1] as f64 / n as f64);
            let xy = roi.to_image(u, v);
            [xy[0], xy[1], p[2] as f64 / n as f64 * roi.size]
        })
        .collect()
}

fn sigmoid(x: f64) -> f64 {
    1.0 / (1.0 + (-x).exp())
}

fn dist2(a: [f64; 3], b: [f64; 3]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

/// Head turn from frontal, in degrees, from the depth difference of the two
/// face edges against their horizontal distance.
pub fn yaw_degrees(landmarks: &[[f64; 3]]) -> f64 {
    let r = landmarks[RIGHT_FACE_EDGE];
    let l = landmarks[LEFT_FACE_EDGE];
    (l[2] - r[2])
        .atan2((l[0] - r[0]).abs().max(1e-6))
        .to_degrees()
}

/// Turns the raw model outputs into [`FaceState`]. `eye_px` is the
/// interocular distance in pixels of the image analysed, for the size limit.
pub fn derive_state(
    presence: f64,
    blendshapes: [f32; BLENDSHAPE_COUNT],
    landmarks: Vec<[f64; 3]>,
    eye_px: f64,
    cfg: &FaceMetricsConfig,
) -> FaceState {
    let b = |i: usize| blendshapes[i] as f64;
    let yaw = yaw_degrees(&landmarks);
    // Turned far enough that one eye is hidden: only the near one says
    // anything. The near eye is the wider one in the image.
    let eye_closed = if yaw.abs() >= cfg.face_state_near_eye_yaw_degrees {
        let right_w = dist2(landmarks[RIGHT_EYE_OUTER], landmarks[RIGHT_EYE_INNER]);
        let left_w = dist2(landmarks[LEFT_EYE_OUTER], landmarks[LEFT_EYE_INNER]);
        // MediaPipe's "left" blendshapes are the subject's left eye.
        if left_w >= right_w {
            b(EYE_BLINK_LEFT)
        } else {
            b(EYE_BLINK_RIGHT)
        }
    } else {
        (b(EYE_BLINK_LEFT) + b(EYE_BLINK_RIGHT)) / 2.0
    };
    let eye_squint = (b(EYE_SQUINT_LEFT) + b(EYE_SQUINT_RIGHT)) / 2.0;
    let smile = (b(MOUTH_SMILE_LEFT) + b(MOUTH_SMILE_RIGHT)) / 2.0;
    let gaze_away = (EYE_LOOK_FIRST..=EYE_LOOK_LAST)
        .map(b)
        .fold(0.0f64, f64::max);
    let eye_state = if presence < cfg.face_state_min_presence
        || yaw.abs() > cfg.face_state_max_yaw_degrees
        || eye_px < cfg.face_state_min_eye_px
    {
        EyeState::Unknown
    } else if eye_closed >= cfg.face_state_closed_threshold {
        EyeState::Closed
    } else if eye_closed >= cfg.face_state_half_threshold {
        EyeState::Half
    } else {
        EyeState::Open
    };
    FaceState {
        presence,
        eye_closed: eye_closed.clamp(0.0, 1.0),
        eye_squint: eye_squint.clamp(0.0, 1.0),
        smile: smile.clamp(0.0, 1.0),
        gaze_away: gaze_away.clamp(0.0, 1.0),
        yaw_degrees: yaw,
        eye_state,
        blendshapes,
        landmarks,
    }
}

/// A face to analyse: the detector's box and five points, in pixels of the
/// image handed to [`FaceStateModel::analyze`].
#[derive(Debug, Clone, Copy)]
pub struct FaceGeometry {
    pub bbox: [f64; 4],
    pub kps: [[f64; 2]; 5],
}

struct Loaded {
    mesh: Session,
    blendshapes: Session,
}

pub struct FaceStateModel {
    paths: FaceStatePaths,
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

fn build_session(path: &std::path::Path) -> ort::Result<Session> {
    let builder = crate::session_builder()?
        .with_execution_providers([ort::ep::CPU::default().with_arena_allocator(false).build()])?;
    crate::apply_kleidiai_policy(builder)?.commit_from_file(path)
}

impl FaceStateModel {
    pub fn new(paths: FaceStatePaths) -> Self {
        FaceStateModel {
            paths,
            loaded: Mutex::new(None),
            last_used_unix: AtomicI64::new(0),
            load_error: Mutex::new(None),
        }
    }

    pub fn paths(&self) -> &FaceStatePaths {
        &self.paths
    }

    pub fn is_cached(&self) -> bool {
        self.paths.all_exist()
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

    fn load_into(&self, guard: &mut Option<Loaded>) -> Result<(), FaceStateError> {
        if guard.is_some() {
            self.last_used_unix.store(now_unix(), Ordering::Relaxed);
            return Ok(());
        }
        match (
            build_session(&self.paths.mesh_onnx),
            build_session(&self.paths.blendshapes_onnx),
        ) {
            (Ok(mesh), Ok(blendshapes)) => {
                *guard = Some(Loaded { mesh, blendshapes });
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
        if self.loaded.lock().unwrap().take().is_some() {
            log::info!("Unloading face state models (requested)");
        }
    }

    pub fn unload_if_idle(&self) {
        let last = self.last_used_unix.load(Ordering::Relaxed);
        if last != 0 && now_unix() - last >= IDLE_UNLOAD_SECONDS {
            let mut guard = self.loaded.lock().unwrap();
            if guard.take().is_some() {
                log::info!("Unloading face state models (idle)");
            }
        }
    }

    fn run_mesh(
        loaded: &mut Loaded,
        rgb: &[u8],
        width: usize,
        height: usize,
        roi: &Roi,
    ) -> Result<(Vec<[f64; 3]>, f64), FaceStateError> {
        let n = MESH_INPUT_SIZE;
        let input = Tensor::from_array(([1usize, n, n, 3], warp_roi(rgb, width, height, roi, n)))?;
        let outputs = loaded.mesh.run(ort::inputs!["image" => input])?;
        let get = |name: &str| {
            outputs.get(name).ok_or_else(|| {
                FaceStateError::UnexpectedOutputs(format!("face mesh has no output `{name}`"))
            })
        };
        let (_, raw) = get("landmarks")?.try_extract_tensor::<f32>()?;
        if raw.len() != LANDMARK_COUNT * 3 {
            return Err(FaceStateError::UnexpectedOutputs(format!(
                "face mesh returned {} values, expected {}",
                raw.len(),
                LANDMARK_COUNT * 3
            )));
        }
        let landmarks = project(raw, roi, n);
        let (_, presence) = get("presence")?.try_extract_tensor::<f32>()?;
        let presence = sigmoid(presence.first().copied().unwrap_or(f32::MIN) as f64);
        Ok((landmarks, presence))
    }

    fn run_blendshapes(
        loaded: &mut Loaded,
        landmarks: &[[f64; 3]],
    ) -> Result<[f32; BLENDSHAPE_COUNT], FaceStateError> {
        let points: Vec<f32> = BLENDSHAPE_LANDMARKS
            .iter()
            .flat_map(|&i| [landmarks[i][0] as f32, landmarks[i][1] as f32])
            .collect();
        let input = Tensor::from_array(([1usize, BLENDSHAPE_LANDMARKS.len(), 2], points))?;
        let outputs = loaded.blendshapes.run(ort::inputs!["points" => input])?;
        let value = outputs.get("blendshapes").ok_or_else(|| {
            FaceStateError::UnexpectedOutputs("blendshape model has no output `blendshapes`".into())
        })?;
        let (_, raw) = value.try_extract_tensor::<f32>()?;
        if raw.len() != BLENDSHAPE_COUNT {
            return Err(FaceStateError::UnexpectedOutputs(format!(
                "blendshape model returned {} values, expected {BLENDSHAPE_COUNT}",
                raw.len()
            )));
        }
        let mut out = [0f32; BLENDSHAPE_COUNT];
        out.copy_from_slice(raw);
        Ok(out)
    }

    /// The 52 blendshapes for landmarks already in hand (image pixels). The
    /// golden test uses this to pin the closed-eye path, which no photograph
    /// in the repository shows.
    pub fn blendshapes_for(
        &self,
        landmarks: &[[f64; 3]],
    ) -> Result<[f32; BLENDSHAPE_COUNT], FaceStateError> {
        if landmarks.len() != LANDMARK_COUNT {
            return Err(FaceStateError::UnexpectedOutputs(format!(
                "{} landmarks given, the blendshape model reads {LANDMARK_COUNT}",
                landmarks.len()
            )));
        }
        let mut guard = self.loaded.lock().unwrap();
        self.load_into(&mut guard)?;
        let loaded = guard.as_mut().ok_or(FaceStateError::NotLoaded)?;
        Self::run_blendshapes(loaded, landmarks)
    }

    /// Analyses each face of `rgb` (`width`×`height`, RGB8). `None` for a face
    /// too small to analyse at all (its box below
    /// [`FaceMetricsConfig::face_state_min_face_px`]): the region would be
    /// upsampled from a handful of pixels, which the model card warns against.
    pub fn analyze(
        &self,
        rgb: &[u8],
        width: usize,
        height: usize,
        faces: &[FaceGeometry],
        cfg: &FaceMetricsConfig,
    ) -> Result<Vec<Option<FaceState>>, FaceStateError> {
        let mut guard = self.loaded.lock().unwrap();
        self.load_into(&mut guard)?;
        let loaded = guard.as_mut().ok_or(FaceStateError::NotLoaded)?;
        let mut out = Vec::with_capacity(faces.len());
        for face in faces {
            let side = (face.bbox[2] - face.bbox[0]).min(face.bbox[3] - face.bbox[1]);
            if side < cfg.face_state_min_face_px {
                out.push(None);
                continue;
            }
            // Detector region first, then MediaPipe's tracking region from the
            // landmarks it produced.
            let first = Roi::from_detection(face.bbox, &face.kps);
            let (landmarks, _) = Self::run_mesh(loaded, rgb, width, height, &first)?;
            let second = Roi::from_landmarks(&landmarks);
            let (landmarks, presence) = Self::run_mesh(loaded, rgb, width, height, &second)?;
            let blendshapes = Self::run_blendshapes(loaded, &landmarks)?;
            let eye_px = dist2(landmarks[RIGHT_EYE_OUTER], landmarks[LEFT_EYE_OUTER]);
            out.push(Some(derive_state(
                presence,
                blendshapes,
                landmarks,
                eye_px,
                cfg,
            )));
        }
        self.last_used_unix.store(now_unix(), Ordering::Relaxed);
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> FaceMetricsConfig {
        FaceMetricsConfig::defaults()
    }

    #[test]
    fn level_eyes_give_no_rotation_and_a_tilt_gives_its_angle() {
        let kps = [[100.0, 100.0], [140.0, 100.0], [0.0; 2], [0.0; 2], [0.0; 2]];
        let roi = Roi::from_detection([80.0, 70.0, 160.0, 170.0], &kps);
        assert!(roi.rotation.abs() < 1e-12);
        assert_eq!((roi.cx, roi.cy), (120.0, 120.0));
        // Long side 100, grown 1.5x.
        assert!((roi.size - 150.0).abs() < 1e-12);

        // Eye line going down to the right by 30 degrees: a clockwise tilt in
        // image coordinates, which the region rotates with.
        let a = 30f64.to_radians();
        let kps = [
            [100.0, 100.0],
            [100.0 + 40.0 * a.cos(), 100.0 + 40.0 * a.sin()],
            [0.0; 2],
            [0.0; 2],
            [0.0; 2],
        ];
        let roi = Roi::from_detection([80.0, 70.0, 160.0, 170.0], &kps);
        assert!((roi.rotation - a).abs() < 1e-9, "{}", roi.rotation);
        // The region's x axis then points along the eye line.
        let p0 = roi.to_image(0.5, 0.5);
        let p1 = roi.to_image(1.0, 0.5);
        let dir = (p1[1] - p0[1]).atan2(p1[0] - p0[0]);
        assert!((dir - a).abs() < 1e-9);
    }

    #[test]
    fn the_warp_samples_the_region_and_replicates_the_border() {
        // 8x8 image: left half 0, right half 255.
        let (w, h) = (8usize, 8usize);
        let mut rgb = vec![0u8; w * h * 3];
        for y in 0..h {
            for x in 4..w {
                let i = (y * w + x) * 3;
                rgb[i..i + 3].fill(255);
            }
        }
        let roi = Roi {
            cx: 4.0,
            cy: 4.0,
            size: 8.0,
            rotation: 0.0,
        };
        let out = warp_roi(&rgb, w, h, &roi, 8);
        assert!(out[0] < 0.01 && out[(8 - 1) * 3] > 0.99);
        // Turned half a circle, left and right swap.
        let flipped = Roi {
            rotation: std::f64::consts::PI,
            ..roi
        };
        let out = warp_roi(&rgb, w, h, &flipped, 8);
        assert!(out[0] > 0.99 && out[(8 - 1) * 3] < 0.01);
        // Far outside the image: the nearest edge pixel, not black.
        let outside = Roi {
            cx: 100.0,
            cy: 4.0,
            size: 8.0,
            rotation: 0.0,
        };
        assert!(warp_roi(&rgb, w, h, &outside, 4).iter().all(|v| *v > 0.99));
    }

    #[test]
    fn projection_inverts_the_warp_geometry() {
        let roi = Roi {
            cx: 300.0,
            cy: 200.0,
            size: 160.0,
            rotation: 0.4,
        };
        // The centre of the model input is the centre of the region.
        let p = project(&[128.0, 128.0, 0.0], &roi, 256);
        assert!((p[0][0] - 300.0).abs() < 1e-9 && (p[0][1] - 200.0).abs() < 1e-9);
        // A point on the input's right edge lands half a region away along
        // the rotated x axis.
        let p = project(&[256.0, 128.0, 25.6], &roi, 256);
        assert!((p[0][0] - (300.0 + 80.0 * 0.4f64.cos())).abs() < 1e-9);
        assert!((p[0][1] - (200.0 + 80.0 * 0.4f64.sin())).abs() < 1e-9);
        // Depth scales with the region like x does.
        assert!((p[0][2] - 16.0).abs() < 1e-5, "{}", p[0][2]);
    }

    fn frontal_landmarks() -> Vec<[f64; 3]> {
        let mut lm = vec![[0.0f64; 3]; LANDMARK_COUNT];
        lm[RIGHT_EYE_OUTER] = [100.0, 100.0, 0.0];
        lm[RIGHT_EYE_INNER] = [130.0, 100.0, 0.0];
        lm[LEFT_EYE_INNER] = [170.0, 100.0, 0.0];
        lm[LEFT_EYE_OUTER] = [200.0, 100.0, 0.0];
        lm[RIGHT_FACE_EDGE] = [70.0, 130.0, 0.0];
        lm[LEFT_FACE_EDGE] = [230.0, 130.0, 0.0];
        lm
    }

    fn shapes(blink: (f32, f32), smile: f32) -> [f32; BLENDSHAPE_COUNT] {
        let mut b = [0f32; BLENDSHAPE_COUNT];
        b[EYE_BLINK_LEFT] = blink.0;
        b[EYE_BLINK_RIGHT] = blink.1;
        b[MOUTH_SMILE_LEFT] = smile;
        b[MOUTH_SMILE_RIGHT] = smile;
        b
    }

    #[test]
    fn blink_scores_map_to_eye_states() {
        let c = cfg();
        let state = |blink| derive_state(0.99, shapes(blink, 0.0), frontal_landmarks(), 100.0, &c);
        assert_eq!(state((0.04, 0.03)).eye_state, EyeState::Open);
        assert_eq!(state((0.45, 0.40)).eye_state, EyeState::Half);
        assert_eq!(state((0.81, 0.76)).eye_state, EyeState::Closed);
        assert!((state((0.81, 0.76)).eye_closed - 0.785).abs() < 1e-6);
    }

    #[test]
    fn faces_the_model_cannot_judge_are_unknown() {
        let c = cfg();
        // Barely a face.
        let s = derive_state(0.2, shapes((0.9, 0.9), 0.0), frontal_landmarks(), 100.0, &c);
        assert_eq!(s.eye_state, EyeState::Unknown);
        // Eyes too small in the image.
        let s = derive_state(0.99, shapes((0.9, 0.9), 0.0), frontal_landmarks(), 8.0, &c);
        assert_eq!(s.eye_state, EyeState::Unknown);
        // Turned almost to profile: the left edge far deeper than the right.
        let mut lm = frontal_landmarks();
        lm[LEFT_FACE_EDGE][2] = 1000.0;
        let s = derive_state(0.99, shapes((0.9, 0.9), 0.0), lm, 100.0, &c);
        assert!(s.yaw_degrees > 80.0, "{}", s.yaw_degrees);
        assert_eq!(s.eye_state, EyeState::Unknown);
    }

    #[test]
    fn a_turned_head_is_judged_on_the_near_eye() {
        let c = cfg();
        // About 40 degrees turned, the subject's left eye (on the right of
        // the image) seen wide, the right one foreshortened and reading closed.
        let mut lm = frontal_landmarks();
        lm[LEFT_FACE_EDGE][2] = 134.0;
        lm[RIGHT_EYE_INNER] = [112.0, 100.0, 0.0];
        let s = derive_state(0.99, shapes((0.05, 0.9), 0.0), lm, 100.0, &c);
        assert!(
            s.yaw_degrees > 35.0 && s.yaw_degrees < 60.0,
            "{}",
            s.yaw_degrees
        );
        assert_eq!(s.eye_state, EyeState::Open);
        assert!((s.eye_closed - 0.05).abs() < 1e-6);
    }

    #[test]
    fn smile_and_gaze_come_from_their_blendshapes() {
        let c = cfg();
        let mut b = shapes((0.0, 0.0), 0.9);
        b[EYE_LOOK_FIRST + 3] = 0.7;
        let s = derive_state(0.99, b, frontal_landmarks(), 100.0, &c);
        assert!((s.smile - 0.9).abs() < 1e-6);
        assert!((s.gaze_away - 0.7).abs() < 1e-6);
    }

    #[test]
    fn eye_state_names_round_trip() {
        for s in [
            EyeState::Open,
            EyeState::Half,
            EyeState::Closed,
            EyeState::Unknown,
        ] {
            assert_eq!(EyeState::parse(s.as_str()), Some(s));
        }
        assert_eq!(EyeState::parse("blinking"), None);
    }
}
