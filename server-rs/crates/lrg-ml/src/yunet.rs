//! YuNet face detector (`face_detection_yunet_2023mar.onnx`) — port of
//! OpenCV's `FaceDetectorYNImpl` (`modules/objdetect/src/face_detect.cpp`),
//! configured for the 2023mar model: strides `[8, 16, 32]`, anchor-free
//! (one prediction per feature-map cell), 5-point landmarks.
//!
//! Replaces the SCRFD detector this project used to run out of InsightFace's
//! `buffalo_l` pack. The models are functionally interchangeable here — both
//! emit a box, a confidence and the same five landmarks — but YuNet is
//! MIT-licensed and 340 KB, so unlike `det_10g.onnx` it can be published with
//! this project and downloaded like every other model family.
//!
//! **Preprocessing note.** OpenCV feeds this network `blobFromImage(image)`
//! with all defaults: scale 1.0, no mean subtraction, `swapRB=false` on an
//! image `imread` produced, i.e. raw 0-255 floats in **BGR** order. Our
//! pixels are RGB, so the channel order is reversed here. That is the same
//! end state SCRFD's port arrived at, but for a different reason: there it
//! reproduced an upstream `swapRB=True`-on-RGB bug, here it is the genuinely
//! correct conversion.
//!
//! **Landmark order.** OpenCV documents YuNet's five points as right eye,
//! left eye, nose tip, right mouth corner, left mouth corner — naming them
//! from the *subject's* point of view, so point 0 sits on the image-left.
//! InsightFace's `arcface_dst` template names the same five points from the
//! *viewer's* point of view ("left eye" first), also image-left first. The
//! two orderings are therefore positionally identical and the landmarks feed
//! [`crate::umeyama`] unchanged. [`Detection::landmarks_look_frontal`] exists
//! to assert that empirically rather than on trust.

use crate::cv2_resize::resize_rgb;

const INPUT_SIZE: usize = 640;
const STRIDES: [usize; 3] = [8, 16, 32];

/// IoU above which a lower-scoring box is suppressed. OpenCV's YuNet default.
const NMS_THRESH: f32 = 0.3;

/// Minimum `sqrt(cls * obj)` confidence for a detection to be kept.
///
/// **Not comparable to the 0.5 this codebase used for SCRFD.** The two
/// detectors produce differently calibrated scores — SCRFD emits a single
/// sigmoid, YuNet the geometric mean of a classification and an objectness
/// head — so the numbers do not transfer. OpenCV's own samples default to
/// 0.9, which is tuned for "don't draw a box on a false positive" in a live
/// demo; that is the wrong trade here, where a missed face silently costs a
/// person cluster and a culling signal. 0.6 keeps YuNet's recall close to
/// what SCRFD@0.5 gave on the golden fixtures while still discarding the
/// low-confidence tail that would pollute clustering.
const DET_THRESH: f32 = 0.6;

#[derive(Debug, Clone)]
pub struct Detection {
    pub bbox: [f32; 4], // x1, y1, x2, y2, in original image coordinates
    pub score: f32,
    pub kps: [[f32; 2]; 5],
}

impl Detection {
    /// Whether the landmarks are ordered image-left eye first, as
    /// [`crate::umeyama::ARCFACE_DST`] expects.
    ///
    /// True for any roughly frontal, roughly upright face: point 0 (the
    /// subject's right eye) is left of point 1, and both sit above the mouth
    /// corners. A model swap that silently mirrored the landmark order would
    /// still produce plausible-looking crops — flipped ones — and only show up
    /// as quietly worse recognition, so the golden test checks this rather
    /// than trusting the upstream documentation.
    pub fn landmarks_look_frontal(&self) -> bool {
        let [re, le, _nose, rm, lm] = self.kps;
        re[0] < le[0] && re[1] < rm[1] && le[1] < lm[1]
    }
}

/// Letterbox into a 640x640 canvas: resize preserving aspect ratio so the
/// image fits, zero-pad the remainder. Returns (canvas, det_scale).
///
/// YuNet is fully convolutional and OpenCV simply sets the input size to the
/// frame's, but a fixed 640x640 is kept from the SCRFD port on purpose: it
/// makes every `Session::run` the same shape, so onnxruntime is not
/// re-planning the graph for each photo's dimensions, and it keeps the
/// `det_scale` bookkeeping below identical to what the face pipeline already
/// had. 640 is a multiple of 32, which the stride-32 head requires.
fn letterbox(pixels: &[u8], width: usize, height: usize) -> (Vec<u8>, f32) {
    let im_ratio = height as f64 / width as f64;
    let model_ratio = 1.0; // input_size (640,640) -> ratio 1
    let (new_width, new_height) = if im_ratio > model_ratio {
        let new_height = INPUT_SIZE;
        let new_width = (new_height as f64 / im_ratio) as usize;
        (new_width, new_height)
    } else {
        let new_width = INPUT_SIZE;
        let new_height = (new_width as f64 * im_ratio) as usize;
        (new_width, new_height)
    };
    let det_scale = new_height as f64 / height as f64;

    let resized = resize_rgb(pixels, width, height, new_width, new_height);
    let mut canvas = vec![0u8; INPUT_SIZE * INPUT_SIZE * 3];
    for y in 0..new_height {
        let src = y * new_width * 3;
        let dst = y * INPUT_SIZE * 3;
        canvas[dst..dst + new_width * 3].copy_from_slice(&resized[src..src + new_width * 3]);
    }
    (canvas, det_scale as f32)
}

/// `cv2.dnn.blobFromImage(canvas)` with OpenCV's defaults: NCHW, raw 0-255
/// floats, BGR channel order (see module docs). No mean or scale — YuNet
/// normalizes internally.
fn to_blob(canvas: &[u8]) -> Vec<f32> {
    let n = INPUT_SIZE * INPUT_SIZE;
    let mut out = vec![0.0f32; 3 * n];
    for i in 0..n {
        out[i] = canvas[i * 3 + 2] as f32; // channel 0 = B
        out[n + i] = canvas[i * 3 + 1] as f32; // channel 1 = G
        out[2 * n + i] = canvas[i * 3] as f32; // channel 2 = R
    }
    out
}

type StrideDecoded = (Vec<f32>, Vec<[f32; 4]>, Vec<[[f32; 2]; 5]>);

/// Decode one stride's four output tensors into detections above
/// [`DET_THRESH`], in the 640x640 letterbox frame. Port of
/// `FaceDetectorYNImpl::generateProposals`.
///
/// Anchor-free: prediction `r * cols + c` belongs to feature-map cell
/// `(c, r)`, whose origin in input pixels is `(c * stride, r * stride)`. The
/// box is centre/size in that cell's units — centre offsets added to the cell
/// index, extents exponentiated — unlike SCRFD's distance-to-each-edge
/// encoding.
fn decode_stride(
    cls: &[f32],
    obj: &[f32],
    bbox: &[f32],
    kps: &[f32],
    stride: usize,
    input_dim: usize,
) -> StrideDecoded {
    let grid = input_dim / stride;

    let mut out_scores = Vec::new();
    let mut out_boxes = Vec::new();
    let mut out_kps = Vec::new();

    for r in 0..grid {
        for c in 0..grid {
            let idx = r * grid + c;
            // OpenCV clamps both heads before combining them: these are raw
            // network outputs, not guaranteed to be in [0, 1], and a value
            // slightly above 1 would otherwise make `score` exceed 1 and skew
            // the culling visibility weight that consumes it.
            let cls_score = cls[idx].clamp(0.0, 1.0);
            let obj_score = obj[idx].clamp(0.0, 1.0);
            let score = (cls_score * obj_score).sqrt();
            if score < DET_THRESH {
                continue;
            }

            let b = &bbox[idx * 4..idx * 4 + 4];
            let cx = (c as f32 + b[0]) * stride as f32;
            let cy = (r as f32 + b[1]) * stride as f32;
            let w = b[2].exp() * stride as f32;
            let h = b[3].exp() * stride as f32;

            let k = &kps[idx * 10..idx * 10 + 10];
            let mut points = [[0.0f32; 2]; 5];
            for (j, point) in points.iter_mut().enumerate() {
                point[0] = (k[j * 2] + c as f32) * stride as f32;
                point[1] = (k[j * 2 + 1] + r as f32) * stride as f32;
            }

            out_scores.push(score);
            out_boxes.push([cx - w / 2.0, cy - h / 2.0, cx + w / 2.0, cy + h / 2.0]);
            out_kps.push(points);
        }
    }
    (out_scores, out_boxes, out_kps)
}

/// Greedy NMS matching `cv::dnn::NMSBoxes`: plain IoU (no `+1` area
/// convention — that was SCRFD's, and carrying it over here would suppress
/// slightly differently on small boxes), IoU > thresh suppressed.
fn nms(dets: &[(f32, [f32; 4])]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..dets.len()).collect();
    order.sort_by(|&a, &b| dets[b].0.partial_cmp(&dets[a].0).unwrap());

    let areas: Vec<f32> = dets
        .iter()
        .map(|(_, b)| (b[2] - b[0]) * (b[3] - b[1]))
        .collect();

    let mut keep = Vec::new();
    let mut suppressed = vec![false; dets.len()];
    for idx in 0..order.len() {
        let i = order[idx];
        if suppressed[i] {
            continue;
        }
        keep.push(i);
        for &j in order.iter().skip(idx + 1) {
            if suppressed[j] {
                continue;
            }
            let bi = dets[i].1;
            let bj = dets[j].1;
            let xx1 = bi[0].max(bj[0]);
            let yy1 = bi[1].max(bj[1]);
            let xx2 = bi[2].min(bj[2]);
            let yy2 = bi[3].min(bj[3]);
            let w = (xx2 - xx1).max(0.0);
            let h = (yy2 - yy1).max(0.0);
            let inter = w * h;
            let union = areas[i] + areas[j] - inter;
            if union > 0.0 && inter / union > NMS_THRESH {
                suppressed[j] = true;
            }
        }
    }
    keep
}

/// Number of output tensors the 2023mar graph emits.
pub const NUM_OUTPUTS: usize = 12;

/// The graph's output tensor names, in the order [`decode_outputs`] expects:
/// all three `cls` heads, then `obj`, then `bbox`, then `kps`. Identical to
/// the list OpenCV passes to `net.forward`.
pub const OUTPUT_NAMES: [&str; NUM_OUTPUTS] = [
    "cls_8", "cls_16", "cls_32", "obj_8", "obj_16", "obj_32", "bbox_8", "bbox_16", "bbox_32",
    "kps_8", "kps_16", "kps_32",
];

/// Run detection end to end given the 12 raw network output tensors, in the
/// order OpenCV requests them by name: `cls_{8,16,32}`, `obj_{8,16,32}`,
/// `bbox_{8,16,32}`, `kps_{8,16,32}` (each a flat row-major `[N, C]` buffer).
pub fn decode_outputs(outputs: &[&[f32]; NUM_OUTPUTS], det_scale: f32) -> Vec<Detection> {
    let mut all_scores = Vec::new();
    let mut all_boxes = Vec::new();
    let mut all_kps = Vec::new();

    for (idx, &stride) in STRIDES.iter().enumerate() {
        let (scores, boxes, kps) = decode_stride(
            outputs[idx],
            outputs[idx + 3],
            outputs[idx + 6],
            outputs[idx + 9],
            stride,
            INPUT_SIZE,
        );
        all_scores.extend(scores);
        all_boxes.extend(boxes);
        all_kps.extend(kps);
    }

    let dets: Vec<(f32, [f32; 4])> = all_scores
        .iter()
        .zip(&all_boxes)
        .map(|(&s, &b)| (s, b))
        .collect();
    let keep = nms(&dets);

    keep.into_iter()
        .map(|i| {
            let (score, bbox) = dets[i];
            let kps = all_kps[i];
            Detection {
                bbox: [
                    bbox[0] / det_scale,
                    bbox[1] / det_scale,
                    bbox[2] / det_scale,
                    bbox[3] / det_scale,
                ],
                score,
                kps: kps.map(|p| [p[0] / det_scale, p[1] / det_scale]),
            }
        })
        .collect()
}

pub struct Preprocessed {
    pub blob: Vec<f32>,
    pub det_scale: f32,
}

pub fn preprocess(pixels: &[u8], width: usize, height: usize) -> Preprocessed {
    let (canvas, det_scale) = letterbox(pixels, width, height);
    Preprocessed {
        blob: to_blob(&canvas),
        det_scale,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letterbox_preserves_aspect_and_scale() {
        // 4:3 landscape: 640x480 fits the canvas width exactly, scale 1.0.
        let pixels = vec![100u8; 640 * 480 * 3];
        let (canvas, scale) = letterbox(&pixels, 640, 480);
        assert_eq!(canvas.len(), INPUT_SIZE * INPUT_SIZE * 3);
        assert!((scale - 1.0).abs() < 1e-6);
    }

    #[test]
    fn blob_is_bgr_and_unnormalized() {
        // One pixel of pure red at the canvas origin; the rest is zero-pad.
        let mut canvas = vec![0u8; INPUT_SIZE * INPUT_SIZE * 3];
        canvas[0] = 200; // R
        canvas[1] = 100; // G
        canvas[2] = 50; // B
        let blob = to_blob(&canvas);
        let n = INPUT_SIZE * INPUT_SIZE;
        assert_eq!(blob[0], 50.0, "channel 0 must be B, raw 0-255");
        assert_eq!(blob[n], 100.0, "channel 1 must be G");
        assert_eq!(blob[2 * n], 200.0, "channel 2 must be R");
    }

    #[test]
    fn nms_suppresses_overlapping_lower_score() {
        let dets = vec![
            (0.9, [10.0, 10.0, 50.0, 50.0]),
            (0.8, [12.0, 12.0, 52.0, 52.0]), // heavy overlap with above
            (0.7, [200.0, 200.0, 240.0, 240.0]), // far away, kept
        ];
        let keep = nms(&dets);
        assert_eq!(keep, vec![0, 2]);
    }

    /// One synthetic detection placed at a known cell, decoded by hand, to
    /// pin the centre/size-with-`exp` encoding against SCRFD's
    /// distance-to-edges one. Getting this backwards produces boxes that are
    /// plausible but wrong, which no downstream assertion would catch.
    #[test]
    fn decodes_centre_size_encoding_at_the_right_cell() {
        let stride = 32usize;
        let grid = INPUT_SIZE / stride; // 20
        let n = grid * grid;
        let (c, r) = (5usize, 3usize);
        let idx = r * grid + c;

        let mut cls = vec![0.0f32; n];
        let mut obj = vec![0.0f32; n];
        let mut bbox = vec![0.0f32; n * 4];
        let kps = vec![0.0f32; n * 10];
        cls[idx] = 1.0;
        obj[idx] = 1.0;
        // Centre exactly on the cell origin, extents e^0 * stride = 32.
        bbox[idx * 4] = 0.0;
        bbox[idx * 4 + 1] = 0.0;
        bbox[idx * 4 + 2] = 0.0;
        bbox[idx * 4 + 3] = 0.0;

        let (scores, boxes, _) = decode_stride(&cls, &obj, &bbox, &kps, stride, INPUT_SIZE);
        assert_eq!(scores.len(), 1);
        assert!((scores[0] - 1.0).abs() < 1e-6);
        let cx = (c * stride) as f32;
        let cy = (r * stride) as f32;
        assert_eq!(
            boxes[0],
            [cx - 16.0, cy - 16.0, cx + 16.0, cy + 16.0],
            "box must be centre/size in cell units, not distance-to-edges"
        );
    }

    #[test]
    fn frontal_landmark_order_check_rejects_a_mirrored_face() {
        let upright = Detection {
            bbox: [0.0, 0.0, 100.0, 100.0],
            score: 0.9,
            kps: [
                [30.0, 40.0], // subject's right eye, image-left
                [70.0, 40.0], // subject's left eye
                [50.0, 60.0], // nose
                [35.0, 80.0], // right mouth corner
                [65.0, 80.0], // left mouth corner
            ],
        };
        assert!(upright.landmarks_look_frontal());

        let mut mirrored = upright.clone();
        mirrored.kps.swap(0, 1);
        assert!(!mirrored.landmarks_look_frontal());
    }
}
