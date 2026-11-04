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
//!
//! Two readings needed real photographs to get right (`examples/
//! face_state_probe.rs`, about 300 portraits from Wikimedia Commons; numbers in
//! `docs/wiki/Dev-Image-Culling-Implementation-Plan.md`). `eyeBlink` tops out
//! well below 1: eyes shut in a photograph read 0.54–0.78 on the more closed
//! eye, where lids moved shut on the mesh read 0.8. And `eyeLook*` measures the
//! eye inside the head, so it says nothing about the lens until the head's own
//! turn is added: someone turned 30° and looking straight at the camera reads
//! 0.99, someone turned 40° with the eyes following the head 0.14.

use std::path::PathBuf;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use ort::session::Session;
use ort::value::Tensor;

use lrg_imaging::cull_config::FaceMetricsConfig;

/// Stamped on every stored face state (`cull_face_state_model`). Bump when a
/// change would make new readings incomparable with stored ones.
pub const MODEL_ID: &str = "mediapipe-facemesh-v2+blendshape-v2/2";

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
const EYE_LOOK_DOWN_LEFT: usize = 11;
const EYE_LOOK_DOWN_RIGHT: usize = 12;
const EYE_LOOK_IN_LEFT: usize = 13;
const EYE_LOOK_IN_RIGHT: usize = 14;
const EYE_LOOK_OUT_LEFT: usize = 15;
const EYE_LOOK_OUT_RIGHT: usize = 16;
const EYE_LOOK_UP_LEFT: usize = 17;
const EYE_LOOK_UP_RIGHT: usize = 18;
const EYE_SQUINT_LEFT: usize = 19;
const EYE_SQUINT_RIGHT: usize = 20;
const MOUTH_SMILE_LEFT: usize = 44;
const MOUTH_SMILE_RIGHT: usize = 45;
const JAW_OPEN: usize = 25;
const MOUTH_LOWER_DOWN_LEFT: usize = 34;
const MOUTH_LOWER_DOWN_RIGHT: usize = 35;

// Mesh landmarks. "Right" and "left" are the subject's, as in MediaPipe: the
// right eye is the one on the left of the image of a frontal face.
const RIGHT_EYE_OUTER: usize = 33;
const RIGHT_EYE_INNER: usize = 133;
const LEFT_EYE_INNER: usize = 362;
const LEFT_EYE_OUTER: usize = 263;

// What the head pose is measured on: landmark pairs spanning the face across
// (the subject's right to left) and upwards, all on bone rather than on the jaw
// or the lips, so that an expression barely moves them.
const POSE_ACROSS: [(usize, usize); 5] = [
    (RIGHT_EYE_OUTER, LEFT_EYE_OUTER),
    (RIGHT_EYE_INNER, LEFT_EYE_INNER),
    (234, 454),
    (127, 356),
    (70, 300),
];
const POSE_UP: [(usize, usize); 4] = [(195, 10), (197, 151), (195, 9), (6, 10)];
/// How far the [`POSE_UP`] pairs lean back from the face's own vertical on
/// MediaPipe's canonical face model (`canonical_face_model.obj`). With the face
/// symmetric about x, this one angle is all of that model the pose needs.
const POSE_UP_LEAN_DEGREES: f64 = 16.1844;

// Skin to read the light on, clear of the eyes, the brows and a beard: the
// middle of the forehead, and high on each cheek.
const LIGHT_FOREHEAD: usize = 151;
const LIGHT_RIGHT_CHEEK: [usize; 2] = [50, 205];
const LIGHT_LEFT_CHEEK: [usize; 2] = [280, 425];
/// A patch's radius against the distance between the eyes.
const LIGHT_PATCH_RATIO: f64 = 0.1;
/// The most samples a side of a patch gets, so a large face costs what a small
/// one does.
const LIGHT_PATCH_MAX_SAMPLES_ACROSS: f64 = 32.0;
/// About how many pixels of the frame are sampled for its surroundings.
const LIGHT_SURROUND_SAMPLES: f64 = 40_000.0;
/// Below this share of the frame outside the faces there are no surroundings
/// worth comparing with.
const LIGHT_MIN_SURROUND_SHARE: f64 = 0.2;
/// The middle of a pixel's three channels at or past this is blown. Not the top
/// one: warm light clips red on skin that still holds its detail, and reading
/// that as blown called a third of well-exposed faces so.
const LIGHT_BLOWN: u8 = 245;

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
    /// How closed the eyes are, `0..1`: the more closed of `eyeBlinkLeft/Right`,
    /// or only the near eye's when the head is turned so far that the far one
    /// is hidden ([`FaceMetricsConfig::face_state_near_eye_yaw_degrees`]).
    pub eye_closed: f64,
    /// Mean of `eyeSquintLeft/Right`. High on a broad smile too, so it means a
    /// squint only without one.
    pub eye_squint: f64,
    /// Mean of `mouthSmileLeft/Right`.
    pub smile: f64,
    /// How far the mouth is open, `0..1`: the larger of `jawOpen` and the mean
    /// `mouthLowerDown`, which between them catch a dropped jaw and lips parted
    /// on a word. Not `mouthPucker`, which reads up to 0.99 on full lips at rest,
    /// nor `mouthFunnel`, which barely moves on photographs.
    pub mouth_open: f64,
    /// How far from the lens the eyes point, in degrees: the head's turn and
    /// the eyes' own turn inside it ([`gaze_off_lens_degrees`]).
    pub gaze_off_lens: f64,
    pub pose: HeadPose,
    pub eye_state: EyeState,
    /// `None` when no skin patch fell inside the image, and always from
    /// [`derive_state`], which sees no pixels.
    pub light: Option<FaceLight>,
    pub blendshapes: [f32; BLENDSHAPE_COUNT],
    /// The 478 landmarks in pixels of the image analysed (`z` scaled like
    /// `x`).
    pub landmarks: Vec<[f64; 3]>,
}

/// How the light falls on a face, read on its skin.
///
/// Everything here but [`Self::luminance`] compares the face with itself or
/// with its own frame, and that one is only ever compared with the same face in
/// another frame. Skin ranges over several stops between people, so an absolute
/// reading would call dark skin underexposed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FaceLight {
    /// Median linear luminance of the skin, `0..1`.
    pub luminance: f64,
    /// Against the median of the frame outside the faces, in stops: negative
    /// is a face darker than its surroundings. `None` when the faces fill the
    /// frame.
    pub against_surround_stops: Option<f64>,
    /// Between the two cheeks, in stops. `None` when the head is turned so far
    /// that only one is seen.
    pub across_face_stops: Option<f64>,
    /// Share of the skin samples blown out, `0..1`.
    pub blown: f64,
}

fn linear_channel(c: u8) -> f64 {
    let v = c as f64 / 255.0;
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_luminance(rgb: &[u8], i: usize) -> f64 {
    0.2126 * linear_channel(rgb[i])
        + 0.7152 * linear_channel(rgb[i + 1])
        + 0.0722 * linear_channel(rgb[i + 2])
}

fn median(values: &mut [f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mid = values.len() / 2;
    let (_, m, _) = values.select_nth_unstable_by(mid, f64::total_cmp);
    Some(*m)
}

fn stops(a: f64, b: f64) -> f64 {
    (a.max(1e-4) / b.max(1e-4)).log2()
}

/// The median luminance of `rgb` outside `faces`' boxes grown 1.5×, or `None`
/// when too little of the frame is left.
fn surround_luminance(
    rgb: &[u8],
    width: usize,
    height: usize,
    faces: &[FaceGeometry],
) -> Option<f64> {
    let step = (((width * height) as f64 / LIGHT_SURROUND_SAMPLES).sqrt() as usize).max(1);
    let boxes: Vec<[f64; 4]> = faces
        .iter()
        .map(|f| {
            let (cx, cy) = ((f.bbox[0] + f.bbox[2]) / 2.0, (f.bbox[1] + f.bbox[3]) / 2.0);
            let half = (f.bbox[2] - f.bbox[0]).max(f.bbox[3] - f.bbox[1]) * 0.75;
            [cx - half, cy - half, cx + half, cy + half]
        })
        .collect();
    let (mut samples, mut seen) = (Vec::new(), 0usize);
    for y in (0..height).step_by(step) {
        for x in (0..width).step_by(step) {
            seen += 1;
            let (px, py) = (x as f64, y as f64);
            if boxes
                .iter()
                .any(|b| px >= b[0] && px < b[2] && py >= b[1] && py < b[3])
            {
                continue;
            }
            samples.push(linear_luminance(rgb, (y * width + x) * 3));
        }
    }
    if (samples.len() as f64) < seen as f64 * LIGHT_MIN_SURROUND_SHARE {
        return None;
    }
    median(&mut samples)
}

/// Reads [`FaceLight`] off the skin patches of one face. `None` when none of
/// them falls inside the image.
fn face_light(
    rgb: &[u8],
    width: usize,
    height: usize,
    landmarks: &[[f64; 3]],
    pose: HeadPose,
    surround: Option<f64>,
    cfg: &FaceMetricsConfig,
) -> Option<FaceLight> {
    let radius = dist2(landmarks[RIGHT_EYE_OUTER], landmarks[LEFT_EYE_OUTER]) * LIGHT_PATCH_RATIO;
    let step = ((radius * 2.0 / LIGHT_PATCH_MAX_SAMPLES_ACROSS) as usize).max(1);
    let r = radius.ceil() as i64;
    // Each sample's luminance, and whether it is blown.
    let read = |points: &[usize]| {
        let mut samples: Vec<(f64, bool)> = Vec::new();
        for &point in points {
            let (cx, cy) = (landmarks[point][0] as i64, landmarks[point][1] as i64);
            for dy in (-r..=r).step_by(step) {
                for dx in (-r..=r).step_by(step) {
                    let (x, y) = (cx + dx, cy + dy);
                    if dx * dx + dy * dy > r * r
                        || x < 0
                        || y < 0
                        || x >= width as i64
                        || y >= height as i64
                    {
                        continue;
                    }
                    let i = (y as usize * width + x as usize) * 3;
                    let (a, b, c) = (rgb[i], rgb[i + 1], rgb[i + 2]);
                    let middle = a.max(b).min(a.min(b).max(c));
                    samples.push((linear_luminance(rgb, i), middle >= LIGHT_BLOWN));
                }
            }
        }
        samples
    };
    let luminances = |samples: &[(f64, bool)]| samples.iter().map(|s| s.0).collect::<Vec<f64>>();
    let forehead = read(&[LIGHT_FOREHEAD]);
    let right = read(&LIGHT_RIGHT_CHEEK);
    let left = read(&LIGHT_LEFT_CHEEK);
    // Turned far enough and the far cheek is on its way out of sight, so only
    // the near one is skin worth reading. Positive yaw turns the left cheek away.
    let both_cheeks = pose.yaw.abs() < cfg.face_state_near_eye_yaw_degrees;
    let across_face_stops = if both_cheeks {
        match (
            median(&mut luminances(&right)),
            median(&mut luminances(&left)),
        ) {
            (Some(a), Some(b)) => Some(stops(a, b).abs()),
            _ => None,
        }
    } else {
        None
    };
    let mut skin = forehead;
    if both_cheeks || pose.yaw < 0.0 {
        skin.extend(left);
    }
    if both_cheeks || pose.yaw > 0.0 {
        skin.extend(right);
    }
    let luminance = median(&mut luminances(&skin))?;
    Some(FaceLight {
        luminance,
        against_surround_stops: surround.map(|s| stops(luminance, s)),
        across_face_stops,
        blown: skin.iter().filter(|s| s.1).count() as f64 / skin.len() as f64,
    })
}

/// Which way the head points, in degrees from facing the camera.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HeadPose {
    /// Positive turns the nose to the right of the image, and the subject's
    /// left cheek away.
    pub yaw: f64,
    /// Positive lifts the chin.
    pub pitch: f64,
    /// Positive raises the eye on the right of the image.
    pub roll: f64,
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

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn along(a: [f64; 3], scale: f64) -> [f64; 3] {
    [a[0] * scale, a[1] * scale, a[2] * scale]
}

fn plus(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn normalized(a: [f64; 3]) -> [f64; 3] {
    along(a, 1.0 / dot(a, a).sqrt().max(1e-9))
}

/// The head's pose from a frame of its landmarks: an axis across the face, one
/// up it, and the one straight out of it. On about 280 real faces this stays
/// within 1° of a full Procrustes fit of all 468 points to MediaPipe's canonical
/// face model on yaw and roll, and within 2° on pitch, at no linear algebra
/// dependency.
pub fn head_pose(landmarks: &[[f64; 3]]) -> HeadPose {
    // Into the canonical model's axes: x right, y up, z towards the viewer.
    let at = |i: usize| {
        let p = landmarks[i];
        [p[0], -p[1], -p[2]]
    };
    let span = |pairs: &[(usize, usize)]| {
        pairs.iter().fold([0.0; 3], |sum, &(from, to)| {
            plus(sum, plus(at(to), along(at(from), -1.0)))
        })
    };
    let across = normalized(span(&POSE_ACROSS));
    let up = span(&POSE_UP);
    let up = normalized(plus(up, along(across, -dot(across, up))));
    let out = [
        across[1] * up[2] - across[2] * up[1],
        across[2] * up[0] - across[0] * up[2],
        across[0] * up[1] - across[1] * up[0],
    ];
    let (sin, cos) = POSE_UP_LEAN_DEGREES.to_radians().sin_cos();
    let forward = plus(along(up, -sin), along(out, cos));
    HeadPose {
        yaw: forward[0].atan2(forward[2]).to_degrees(),
        pitch: forward[1].atan2(forward[0].hypot(forward[2])).to_degrees(),
        roll: across[1].atan2(across[0]).to_degrees(),
    }
}

/// How far from the lens the eyes point, in degrees: the head's turn plus the
/// eyes' own turn inside it, read off `eyeLook*`. Someone with the head turned
/// who looks back at the camera comes out near 0; the eyes' turn cancels the
/// head's.
///
/// The up-and-down part is the weaker of the two: `eyeLookUp` barely moves when
/// a lowered head looks up at the lens, so such a face reads 15–20° off it.
pub fn gaze_off_lens_degrees(
    pose: HeadPose,
    blendshapes: &[f32; BLENDSHAPE_COUNT],
    cfg: &FaceMetricsConfig,
) -> f64 {
    let b = |i: usize| blendshapes[i] as f64;
    // Towards the subject's left, the right of the image.
    let across = (b(EYE_LOOK_OUT_LEFT) - b(EYE_LOOK_IN_LEFT) + b(EYE_LOOK_IN_RIGHT)
        - b(EYE_LOOK_OUT_RIGHT))
        / 2.0;
    let up = (b(EYE_LOOK_UP_LEFT) + b(EYE_LOOK_UP_RIGHT)
        - b(EYE_LOOK_DOWN_LEFT)
        - b(EYE_LOOK_DOWN_RIGHT))
        / 2.0;
    (pose.yaw + cfg.face_state_gaze_across_degrees * across)
        .hypot(pose.pitch + cfg.face_state_gaze_up_degrees * up)
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
    let pose = head_pose(&landmarks);
    // Turned far enough that one eye is hidden: only the near one says
    // anything. The near eye is the wider one in the image. Otherwise the more
    // closed eye speaks: the mean let one shut eye hide behind an open one, and
    // read 0.56 on eyes shut that the worse eye put at 0.65.
    let eye_closed = if pose.yaw.abs() >= cfg.face_state_near_eye_yaw_degrees {
        let right_w = dist2(landmarks[RIGHT_EYE_OUTER], landmarks[RIGHT_EYE_INNER]);
        let left_w = dist2(landmarks[LEFT_EYE_OUTER], landmarks[LEFT_EYE_INNER]);
        // MediaPipe's "left" blendshapes are the subject's left eye.
        if left_w >= right_w {
            b(EYE_BLINK_LEFT)
        } else {
            b(EYE_BLINK_RIGHT)
        }
    } else {
        b(EYE_BLINK_LEFT).max(b(EYE_BLINK_RIGHT))
    };
    let eye_squint = (b(EYE_SQUINT_LEFT) + b(EYE_SQUINT_RIGHT)) / 2.0;
    let smile = (b(MOUTH_SMILE_LEFT) + b(MOUTH_SMILE_RIGHT)) / 2.0;
    let mouth_open = b(JAW_OPEN).max((b(MOUTH_LOWER_DOWN_LEFT) + b(MOUTH_LOWER_DOWN_RIGHT)) / 2.0);
    let gaze_off_lens = gaze_off_lens_degrees(pose, &blendshapes, cfg);
    let eye_state = if presence < cfg.face_state_min_presence
        || pose.yaw.abs() > cfg.face_state_max_yaw_degrees
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
        mouth_open: mouth_open.clamp(0.0, 1.0),
        gaze_off_lens,
        pose,
        eye_state,
        light: None,
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
        let surround = surround_luminance(rgb, width, height, faces);
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
            let mut state = derive_state(presence, blendshapes, landmarks, eye_px, cfg);
            state.light = face_light(
                rgb,
                width,
                height,
                &state.landmarks,
                state.pose,
                surround,
                cfg,
            );
            out.push(Some(state));
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

    /// The points [`head_pose`] reads, from MediaPipe's canonical face model,
    /// turned by the given pose and placed in the image as the mesh would.
    fn posed_landmarks(yaw: f64, pitch: f64, roll: f64) -> Vec<[f64; 3]> {
        const CANONICAL: [(usize, [f64; 3]); 16] = [
            (6, [0.0, 2.473255, 5.788627]),
            (9, [0.0, 4.885979, 5.385258]),
            (10, [0.0, 8.261778, 4.481535]),
            (33, [-4.445859, 2.663991, 3.173422]),
            (70, [-5.720968, 4.254584, 2.830852]),
            (127, [-7.743095, 2.364999, -2.005167]),
            (133, [-1.856432, 2.585245, 3.757904]),
            (151, [0.0, 6.54539, 5.027311]),
            (195, [0.0, 1.059413, 6.774605]),
            (197, [0.0, 1.728369, 6.31675]),
            (234, [-7.664182, 0.673132, -2.435867]),
            (263, [4.445859, 2.663991, 3.173422]),
            (300, [5.720968, 4.254584, 2.830852]),
            (356, [7.743095, 2.364999, -2.005167]),
            (362, [1.856432, 2.585245, 3.757904]),
            (454, [7.664182, 0.673132, -2.435867]),
        ];
        let (sy, cy) = yaw.to_radians().sin_cos();
        let (sp, cp) = pitch.to_radians().sin_cos();
        let (sr, cr) = roll.to_radians().sin_cos();
        let mut lm = vec![[0.0f64; 3]; LANDMARK_COUNT];
        for (i, [x, y, z]) in CANONICAL {
            // Pitch about x, so +z goes towards +y; then yaw about y, so +z goes
            // towards +x; then roll about the viewing axis. The order
            // [`head_pose`] reads them back in.
            let (y, z) = (y * cp + z * sp, z * cp - y * sp);
            let (x, z) = (x * cy + z * sy, z * cy - x * sy);
            let (x, y) = (x * cr - y * sr, x * sr + y * cr);
            // Back into the image: y down, z away from the viewer.
            lm[i] = [200.0 + 10.0 * x, 200.0 - 10.0 * y, -10.0 * z];
        }
        lm
    }

    fn frontal_landmarks() -> Vec<[f64; 3]> {
        posed_landmarks(0.0, 0.0, 0.0)
    }

    #[test]
    fn the_head_pose_recovers_a_known_pose() {
        for (yaw, pitch, roll) in [
            (0.0, 0.0, 0.0),
            (30.0, 0.0, 0.0),
            (-30.0, 0.0, 0.0),
            (0.0, 20.0, 0.0),
            (0.0, -20.0, 0.0),
            (0.0, 0.0, 15.0),
            (-40.0, -15.0, 0.0),
        ] {
            let got = head_pose(&posed_landmarks(yaw, pitch, roll));
            assert!((got.yaw - yaw).abs() < 0.1, "yaw {yaw}: {got:?}");
            assert!((got.pitch - pitch).abs() < 0.1, "pitch {pitch}: {got:?}");
            assert!((got.roll - roll).abs() < 0.1, "roll {roll}: {got:?}");
        }
    }

    /// Which way each sign points, against the geometry and not the formula: a
    /// nose moved right, a chin lifted, the right-hand eye raised.
    #[test]
    fn the_head_pose_signs_mean_what_they_say() {
        let turned = posed_landmarks(30.0, 0.0, 0.0);
        let (nose, left, right) = (turned[195], turned[454], turned[234]);
        assert!(nose[0] > (left[0] + right[0]) / 2.0, "nose to image right");
        assert!(left[2] > right[2], "the subject's left cheek goes away");

        let lifted = posed_landmarks(0.0, 20.0, 0.0);
        let (level, up) = (frontal_landmarks()[195][1], lifted[195][1]);
        assert!(up < level, "the nose rises in the image: {up} {level}");

        let rolled = posed_landmarks(0.0, 0.0, 15.0);
        assert!(
            rolled[LEFT_EYE_OUTER][1] < rolled[RIGHT_EYE_OUTER][1],
            "the eye on the right of the image is the higher one"
        );
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
        assert!((state((0.81, 0.76)).eye_closed - 0.81).abs() < 1e-6);
    }

    /// One eye shut on a frontal face: the mean would have read this as open.
    #[test]
    fn the_more_closed_eye_speaks_for_a_frontal_face() {
        let c = cfg();
        let s = derive_state(
            0.99,
            shapes((0.62, 0.11), 0.0),
            frontal_landmarks(),
            100.0,
            &c,
        );
        assert!((s.eye_closed - 0.62).abs() < 1e-6);
        assert_eq!(s.eye_state, EyeState::Closed);
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
        // Turned almost to profile.
        let lm = posed_landmarks(70.0, 0.0, 0.0);
        let s = derive_state(0.99, shapes((0.9, 0.9), 0.0), lm, 100.0, &c);
        assert!(s.pose.yaw > 60.0, "{:?}", s.pose);
        assert_eq!(s.eye_state, EyeState::Unknown);
    }

    #[test]
    fn a_turned_head_is_judged_on_the_near_eye() {
        let c = cfg();
        // 40 degrees to the right of the image: the subject's right eye, on
        // the left of the image, is the near one and seen wide; the far one is
        // foreshortened and reads closed.
        let lm = posed_landmarks(40.0, 0.0, 0.0);
        let s = derive_state(0.99, shapes((0.9, 0.05), 0.0), lm, 100.0, &c);
        assert_eq!(s.eye_state, EyeState::Open);
        assert!((s.eye_closed - 0.05).abs() < 1e-6);
    }

    fn eyes_turned(towards_image_right: f32, up: f32) -> [f32; BLENDSHAPE_COUNT] {
        let mut b = [0f32; BLENDSHAPE_COUNT];
        if towards_image_right >= 0.0 {
            b[EYE_LOOK_OUT_LEFT] = towards_image_right;
            b[EYE_LOOK_IN_RIGHT] = towards_image_right;
        } else {
            b[EYE_LOOK_IN_LEFT] = -towards_image_right;
            b[EYE_LOOK_OUT_RIGHT] = -towards_image_right;
        }
        if up >= 0.0 {
            b[EYE_LOOK_UP_LEFT] = up;
            b[EYE_LOOK_UP_RIGHT] = up;
        } else {
            b[EYE_LOOK_DOWN_LEFT] = -up;
            b[EYE_LOOK_DOWN_RIGHT] = -up;
        }
        b
    }

    /// The portrait pose that used to be labelled "looking away": the head
    /// turned, the eyes back at the camera. Real readings from the probe.
    #[test]
    fn eyes_that_come_back_to_the_lens_cancel_the_head_turn() {
        let c = cfg();
        let gaze = |yaw, eyes| {
            derive_state(0.99, eyes, posed_landmarks(yaw, 0.0, 0.0), 100.0, &c).gaze_off_lens
        };
        assert!(gaze(-29.0, eyes_turned(0.99, 0.0)) < 5.0);
        assert!(gaze(25.0, eyes_turned(-0.84, 0.0)) < 5.0);
        // The head turned and the eyes following it: that is looking away,
        // though the eyes themselves are centred.
        assert!(gaze(39.0, eyes_turned(0.0, 0.0)) > 35.0);
        // A frontal head with the eyes off to the side is looking away too.
        assert!(gaze(0.0, eyes_turned(0.9, 0.0)) > 20.0);
        // The same turn the other way round does not cancel, it adds up.
        assert!(gaze(25.0, eyes_turned(0.5, 0.0)) > 35.0);
    }

    #[test]
    fn eyes_looking_down_from_a_level_head_are_off_the_lens() {
        let c = cfg();
        let s = derive_state(0.99, eyes_turned(0.0, -0.8), frontal_landmarks(), 100.0, &c);
        assert!(s.gaze_off_lens > 15.0, "{}", s.gaze_off_lens);
    }

    #[test]
    fn the_mouth_reads_a_dropped_jaw_and_parted_lips_alike() {
        let c = cfg();
        let open = |set: &[(usize, f32)]| {
            let mut b = [0f32; BLENDSHAPE_COUNT];
            for (i, v) in set {
                b[*i] = *v;
            }
            derive_state(0.99, b, frontal_landmarks(), 100.0, &c).mouth_open
        };
        assert!((open(&[(JAW_OPEN, 0.52)]) - 0.52).abs() < 1e-6);
        let lips = [(MOUTH_LOWER_DOWN_LEFT, 0.7), (MOUTH_LOWER_DOWN_RIGHT, 0.5)];
        assert!((open(&lips) - 0.6).abs() < 1e-6);
        assert_eq!(open(&[]), 0.0);
    }

    fn flat_grey(width: usize, height: usize, value: u8) -> Vec<u8> {
        vec![value; width * height * 3]
    }

    /// Paints the disk around each given landmark, as a light would a cheek.
    fn lit(rgb: &mut [u8], width: usize, lm: &[[f64; 3]], points: &[usize], value: u8) {
        for &point in points {
            let (cx, cy) = (lm[point][0] as i64, lm[point][1] as i64);
            for y in (cy - 14)..(cy + 14) {
                for x in (cx - 14)..(cx + 14) {
                    let i = (y as usize * width + x as usize) * 3;
                    rgb[i..i + 3].fill(value);
                }
            }
        }
    }

    /// The points [`face_light`] reads, placed on a level head.
    fn light_landmarks() -> Vec<[f64; 3]> {
        let mut lm = frontal_landmarks();
        lm[LIGHT_FOREHEAD] = [200.0, 120.0, 0.0];
        lm[LIGHT_RIGHT_CHEEK[0]] = [150.0, 210.0, 0.0];
        lm[LIGHT_RIGHT_CHEEK[1]] = [150.0, 240.0, 0.0];
        lm[LIGHT_LEFT_CHEEK[0]] = [250.0, 210.0, 0.0];
        lm[LIGHT_LEFT_CHEEK[1]] = [250.0, 240.0, 0.0];
        lm
    }

    fn level() -> HeadPose {
        HeadPose {
            yaw: 0.0,
            pitch: 0.0,
            roll: 0.0,
        }
    }

    #[test]
    fn light_across_a_face_is_read_cheek_against_cheek() {
        let c = cfg();
        let (w, h) = (400usize, 400usize);
        let mut rgb = flat_grey(w, h, 128);
        let lm = light_landmarks();
        // A stop in linear light is about 1.36x in sRGB around here: 188 and
        // 100 are 2 stops apart, within what rounding leaves of it.
        lit(&mut rgb, w, &lm, &LIGHT_RIGHT_CHEEK, 188);
        lit(&mut rgb, w, &lm, &LIGHT_LEFT_CHEEK, 100);
        let light = face_light(&rgb, w, h, &lm, level(), None, &c).unwrap();
        let across = light.across_face_stops.unwrap();
        assert!((across - 2.0).abs() < 0.2, "{across}");
        assert_eq!(light.against_surround_stops, None);
        assert_eq!(light.blown, 0.0);
    }

    #[test]
    fn a_turned_head_has_no_reading_across_it_and_is_read_on_the_near_cheek() {
        let c = cfg();
        let (w, h) = (400usize, 400usize);
        let mut rgb = flat_grey(w, h, 128);
        let lm = light_landmarks();
        lit(&mut rgb, w, &lm, &LIGHT_RIGHT_CHEEK, 200);
        lit(&mut rgb, w, &lm, &[LIGHT_FOREHEAD], 200);
        // The left cheek is going out of sight, whatever it reads.
        lit(&mut rgb, w, &lm, &LIGHT_LEFT_CHEEK, 10);
        let pose = HeadPose {
            yaw: 45.0,
            ..level()
        };
        let light = face_light(&rgb, w, h, &lm, pose, None, &c).unwrap();
        assert_eq!(light.across_face_stops, None);
        assert!((light.luminance - linear_channel(200)).abs() < 0.01);
    }

    /// Warm light clips the red channel on skin that still holds its detail; it
    /// takes two channels gone to call it blown.
    #[test]
    fn red_alone_at_the_top_is_not_blown_skin() {
        let c = cfg();
        let (w, h) = (400usize, 400usize);
        let lm = light_landmarks();
        let mut warm = flat_grey(w, h, 128);
        for px in warm.as_chunks_mut::<3>().0 {
            *px = [255, 200, 170];
        }
        let light = face_light(&warm, w, h, &lm, level(), None, &c).unwrap();
        assert_eq!(light.blown, 0.0);

        let mut white = flat_grey(w, h, 128);
        for px in white.as_chunks_mut::<3>().0 {
            *px = [255, 250, 240];
        }
        let light = face_light(&white, w, h, &lm, level(), None, &c).unwrap();
        assert_eq!(light.blown, 1.0);
    }

    #[test]
    fn the_surroundings_leave_the_faces_out_and_give_up_when_they_fill_the_frame() {
        let (w, h) = (400usize, 400usize);
        let mut rgb = flat_grey(w, h, 200);
        for y in 150..250 {
            for x in 150..250 {
                let i = (y * w + x) * 3;
                rgb[i..i + 3].fill(30);
            }
        }
        let geometry = |bbox: [f64; 4]| FaceGeometry {
            bbox,
            kps: [[0.0; 2]; 5],
        };
        let small = [geometry([150.0, 150.0, 250.0, 250.0])];
        let surround = surround_luminance(&rgb, w, h, &small).unwrap();
        assert!((surround - linear_channel(200)).abs() < 0.01, "{surround}");

        let filling = [geometry([0.0, 0.0, 400.0, 400.0])];
        assert_eq!(surround_luminance(&rgb, w, h, &filling), None);
    }

    #[test]
    fn the_smile_comes_from_its_blendshapes() {
        let c = cfg();
        let s = derive_state(
            0.99,
            shapes((0.0, 0.0), 0.9),
            frontal_landmarks(),
            100.0,
            &c,
        );
        assert!((s.smile - 0.9).abs() < 1e-6);
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
