//! ort-backed RF-DETR-Seg Nano (Roboflow, Apache-2.0): finds the objects in a
//! photo — people, animals, vehicles, things — each with a score, a box and a
//! mask.
//!
//! Culling needs to know *where the subject is*. A frame whose subject is soft
//! on a sharp background missed focus; a frame whose subject is sharp on a soft
//! background is a shallow depth of field or a pan, and is the keeper. The
//! frame-wide and tiled sharpness cannot tell the two apart, and faces only
//! answer it for people facing the camera. What is measured on the mask lives
//! in `lrg_imaging::subject_focus`; this module only finds the objects.
//!
//! Exported by `scripts/export_subject_rfdetr.py`, which pins the checkpoint,
//! checks the export against PyTorch and writes the goldens
//! `tests/subject_goldens.rs` holds this port to.

use std::path::PathBuf;

use ort::session::Session;
use ort::value::Tensor;

use crate::tower::{build_session, NotLoaded, Tower};

/// Identifier for what this build stores from the model, written with every
/// reading as `cull_subject_model`. **Bump it whenever stored readings would
/// no longer compare with new ones**: other weights, another export, or a
/// change to [`preprocess`] or [`decode`].
pub const MODEL_ID: &str = "rfdetr-seg-nano-312/fp32";

/// The square the model looks at. The photo is resized to it without keeping
/// the aspect ratio, as RF-DETR's own `predict()` does.
pub const INPUT_SIZE: usize = 312;

/// Side of each object's mask: a quarter of the input, over the whole frame.
pub const MASK_SIZE: usize = 78;

/// Queries (object slots) and class slots of the exported graph.
pub const QUERIES: usize = 100;
pub const CLASS_SLOTS: usize = 91;

/// Below this an object is not reported. RF-DETR's own default.
pub const MIN_SCORE: f32 = 0.3;

const MEAN: [f32; 3] = [0.485, 0.456, 0.406];
const STD: [f32; 3] = [0.229, 0.224, 0.225];

#[derive(Debug, thiserror::Error)]
pub enum SubjectError {
    #[error("ort error: {0}")]
    Ort(#[from] ort::Error),
    #[error("model not loaded")]
    NotLoaded,
    #[error("the model returned {0} values for {1}, expected {2}")]
    Shape(usize, &'static str, usize),
}

impl From<NotLoaded> for SubjectError {
    fn from(_: NotLoaded) -> Self {
        SubjectError::NotLoaded
    }
}

/// What kind of thing a COCO class is, as culling cares about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubjectKind {
    Person,
    Animal,
    Vehicle,
    /// A thing that can be what a photo is of: a ball, a cake, a vase.
    Object,
    /// Furniture and street fixtures — a bench, a table, a traffic light.
    /// Found, but never the subject.
    Scenery,
}

impl SubjectKind {
    /// By COCO category id, which is the class slot's index in this export.
    pub fn of_class(class_id: usize) -> SubjectKind {
        match class_id {
            1 => SubjectKind::Person,
            2..=9 => SubjectKind::Vehicle,
            16..=25 => SubjectKind::Animal,
            10 | 11 | 13 | 14 | 15 | 62 | 63 | 64 | 65 | 67 | 70 | 72 | 78 | 79 | 81 | 82 => {
                SubjectKind::Scenery
            }
            _ => SubjectKind::Object,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            SubjectKind::Person => "person",
            SubjectKind::Animal => "animal",
            SubjectKind::Vehicle => "vehicle",
            SubjectKind::Object => "object",
            SubjectKind::Scenery => "scenery",
        }
    }
}

/// One object the model found.
#[derive(Debug, Clone)]
pub struct Detection {
    /// COCO category id (1 = person, 16 = bird, …).
    pub class_id: usize,
    pub kind: SubjectKind,
    pub score: f32,
    /// `x0, y0, x1, y1`, as fractions of the frame.
    pub bbox: [f32; 4],
    /// [`MASK_SIZE`]² cells, row-major, over the whole frame: true where the
    /// object is.
    pub mask: Vec<bool>,
}

impl Detection {
    /// The share of the frame the mask covers.
    pub fn mask_area(&self) -> f32 {
        self.mask.iter().filter(|&&m| m).count() as f32 / self.mask.len().max(1) as f32
    }
}

pub struct SubjectPaths {
    pub onnx: PathBuf,
}

impl SubjectPaths {
    pub fn all_exist(&self) -> bool {
        self.onnx.is_file()
    }
}

/// One RGB8 image as the model takes it: `1×3×INPUT_SIZE×INPUT_SIZE`,
/// normalised with ImageNet's statistics.
///
/// Bilinear on the float image with half-pixel centres and no antialiasing —
/// what RF-DETR's `predict()` does, and therefore what it was trained to see.
/// Not `cv2_resize::resize_rgb`, which rounds back to 8 bits in between.
pub fn preprocess(pixels: &[u8], width: usize, height: usize) -> Vec<f32> {
    let n = INPUT_SIZE;
    let plane = n * n;
    let mut out = vec![0.0f32; 3 * plane];
    if width == 0 || height == 0 || pixels.len() < width * height * 3 {
        return out;
    }
    let sample = |out_i: usize, in_len: usize| -> (usize, usize, f32) {
        let s = ((out_i as f64 + 0.5) * (in_len as f64 / n as f64) - 0.5)
            .clamp(0.0, (in_len - 1) as f64);
        let i0 = s.floor() as usize;
        let i1 = (i0 + 1).min(in_len - 1);
        (i0, i1, (s - i0 as f64) as f32)
    };
    let xs: Vec<(usize, usize, f32)> = (0..n).map(|x| sample(x, width)).collect();
    for oy in 0..n {
        let (y0, y1, fy) = sample(oy, height);
        for (ox, &(x0, x1, fx)) in xs.iter().enumerate() {
            for c in 0..3 {
                let p = |x: usize, y: usize| pixels[(y * width + x) * 3 + c] as f32 / 255.0;
                let top = p(x0, y0) * (1.0 - fx) + p(x1, y0) * fx;
                let bottom = p(x0, y1) * (1.0 - fx) + p(x1, y1) * fx;
                let v = top * (1.0 - fy) + bottom * fy;
                out[c * plane + oy * n + ox] = (v - MEAN[c]) / STD[c];
            }
        }
    }
    out
}

fn sigmoid(x: f32) -> f32 {
    1.0 / (1.0 + (-x).exp())
}

/// The model's three outputs as objects: per query its best class, kept when
/// that class scores at least `min_score`, best first. RF-DETR scores every
/// class with its own sigmoid and needs no non-maximum suppression.
///
/// `dets` is `QUERIES×4` normalised `cx, cy, w, h`; `logits` is
/// `QUERIES×CLASS_SLOTS`; `masks` is `QUERIES×MASK_SIZE²` logits.
pub fn decode(
    dets: &[f32],
    logits: &[f32],
    masks: &[f32],
    min_score: f32,
) -> Result<Vec<Detection>, SubjectError> {
    if dets.len() != QUERIES * 4 {
        return Err(SubjectError::Shape(dets.len(), "dets", QUERIES * 4));
    }
    if logits.len() != QUERIES * CLASS_SLOTS {
        return Err(SubjectError::Shape(
            logits.len(),
            "labels",
            QUERIES * CLASS_SLOTS,
        ));
    }
    let cells = MASK_SIZE * MASK_SIZE;
    if masks.len() != QUERIES * cells {
        return Err(SubjectError::Shape(masks.len(), "masks", QUERIES * cells));
    }
    let mut out = Vec::new();
    for q in 0..QUERIES {
        let row = &logits[q * CLASS_SLOTS..(q + 1) * CLASS_SLOTS];
        let (class_id, logit) =
            row.iter()
                .copied()
                .enumerate()
                .fold((0, f32::NEG_INFINITY), |best, (i, v)| {
                    if v > best.1 {
                        (i, v)
                    } else {
                        best
                    }
                });
        let score = sigmoid(logit);
        if score < min_score {
            continue;
        }
        let [cx, cy, w, h] = [
            dets[q * 4],
            dets[q * 4 + 1],
            dets[q * 4 + 2],
            dets[q * 4 + 3],
        ];
        let bbox = [
            (cx - w / 2.0).clamp(0.0, 1.0),
            (cy - h / 2.0).clamp(0.0, 1.0),
            (cx + w / 2.0).clamp(0.0, 1.0),
            (cy + h / 2.0).clamp(0.0, 1.0),
        ];
        let mask = masks[q * cells..(q + 1) * cells]
            .iter()
            .map(|&m| m > 0.0)
            .collect();
        out.push(Detection {
            class_id,
            kind: SubjectKind::of_class(class_id),
            score,
            bbox,
            mask,
        });
    }
    out.sort_by(|a, b| b.score.total_cmp(&a.score));
    Ok(out)
}

/// Lazily-loaded, idle-unloadable object finder. One instance is shared behind
/// an `Arc` in `AppState`, like [`crate::aesthetic::AestheticModel`].
pub struct SubjectModel {
    paths: SubjectPaths,
    tower: Tower<Session>,
}

impl SubjectModel {
    pub fn new(paths: SubjectPaths) -> Self {
        SubjectModel {
            paths,
            tower: Tower::new("subject"),
        }
    }

    pub fn is_cached(&self) -> bool {
        self.paths.all_exist()
    }

    /// A constant, so `/v1/index/unprocessed` can compare against it without
    /// loading the model.
    pub fn model_id(&self) -> &'static str {
        MODEL_ID
    }

    pub fn unload(&self) {
        self.tower.unload_with_reason("detection", "requested");
    }

    pub fn unload_if_idle(&self) {
        self.tower.unload_if_idle("detection");
    }

    /// `("loaded" | "not_loaded" | "failed", error)`, as `/health` reports it.
    pub fn status(&self) -> (&'static str, Option<String>) {
        self.tower.status()
    }

    /// The objects in one RGB8 image, best first.
    pub fn detect(
        &self,
        pixels: &[u8],
        width: usize,
        height: usize,
    ) -> Result<Vec<Detection>, SubjectError> {
        self.tower.with(
            "detection",
            || build_session(&self.paths.onnx),
            |session| {
                let input = Tensor::from_array((
                    [1, 3, INPUT_SIZE, INPUT_SIZE],
                    preprocess(pixels, width, height),
                ))?;
                let outputs = session.run(ort::inputs!["input" => input])?;
                let (_, dets) = outputs["dets"].try_extract_tensor::<f32>()?;
                let (_, logits) = outputs["labels"].try_extract_tensor::<f32>()?;
                let (_, masks) = outputs["masks"].try_extract_tensor::<f32>()?;
                decode(dets, logits, masks, MIN_SCORE)
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coco_classes_map_to_the_kinds_culling_cares_about() {
        assert_eq!(SubjectKind::of_class(1), SubjectKind::Person);
        assert_eq!(SubjectKind::of_class(16), SubjectKind::Animal); // bird
        assert_eq!(SubjectKind::of_class(25), SubjectKind::Animal); // giraffe
        assert_eq!(SubjectKind::of_class(3), SubjectKind::Vehicle); // car
        assert_eq!(SubjectKind::of_class(37), SubjectKind::Object); // sports ball
        assert_eq!(SubjectKind::of_class(67), SubjectKind::Scenery); // dining table
        assert_eq!(SubjectKind::of_class(15), SubjectKind::Scenery); // bench
    }

    #[test]
    fn preprocessing_keeps_a_flat_colour_flat_and_normalised() {
        let (w, h) = (40, 30);
        let pixels: Vec<u8> = std::iter::repeat_n([255u8, 128, 0], w * h)
            .flatten()
            .collect();
        let t = preprocess(&pixels, w, h);
        let plane = INPUT_SIZE * INPUT_SIZE;
        assert_eq!(t.len(), 3 * plane);
        let expect = |c: usize, v: f32| (v - MEAN[c]) / STD[c];
        for (c, v) in [(0, 1.0), (1, 128.0 / 255.0), (2, 0.0)] {
            for i in [0, plane / 2, plane - 1] {
                assert!((t[c * plane + i] - expect(c, v)).abs() < 1e-5, "c{c} i{i}");
            }
        }
    }

    fn outputs_with(query: usize, class_id: usize, logit: f32) -> (Vec<f32>, Vec<f32>, Vec<f32>) {
        let mut dets = vec![0.5f32; QUERIES * 4];
        dets[query * 4..query * 4 + 4].copy_from_slice(&[0.5, 0.5, 0.4, 0.6]);
        let mut logits = vec![-10.0f32; QUERIES * CLASS_SLOTS];
        logits[query * CLASS_SLOTS + class_id] = logit;
        let cells = MASK_SIZE * MASK_SIZE;
        let mut masks = vec![-5.0f32; QUERIES * cells];
        for y in 20..60 {
            for x in 30..50 {
                masks[query * cells + y * MASK_SIZE + x] = 3.0;
            }
        }
        (dets, logits, masks)
    }

    #[test]
    fn decoding_keeps_confident_objects_with_box_and_mask() {
        let (dets, logits, masks) = outputs_with(7, 16, 2.0);
        let found = decode(&dets, &logits, &masks, MIN_SCORE).unwrap();
        assert_eq!(found.len(), 1);
        let bird = &found[0];
        assert_eq!((bird.class_id, bird.kind), (16, SubjectKind::Animal));
        assert!((bird.score - sigmoid(2.0)).abs() < 1e-6);
        let expected = [0.3, 0.2, 0.7, 0.8];
        for (got, want) in bird.bbox.iter().zip(expected) {
            assert!((got - want).abs() < 1e-6);
        }
        let area = 40.0 * 20.0 / (MASK_SIZE * MASK_SIZE) as f32;
        assert!((bird.mask_area() - area).abs() < 1e-6);
    }

    #[test]
    fn decoding_drops_unsure_objects_and_refuses_the_wrong_shape() {
        let (dets, logits, masks) = outputs_with(3, 1, -2.0);
        assert!(decode(&dets, &logits, &masks, MIN_SCORE)
            .unwrap()
            .is_empty());
        assert!(matches!(
            decode(&dets[..8], &logits, &masks, MIN_SCORE),
            Err(SubjectError::Shape(8, "dets", _))
        ));
    }
}
