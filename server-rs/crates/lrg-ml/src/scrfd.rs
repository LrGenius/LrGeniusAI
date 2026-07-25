//! SCRFD face detector (buffalo_l `det_10g.onnx`) — port of
//! `insightface.model_zoo.scrfd.SCRFD`, configured for its 9-output
//! variant (`fmc=3`, strides `[8, 16, 32]`, 2 anchors/location, 5-point
//! kps).
//!
//! Preprocessing note: `FaceAnalysis.get(img)` is handed an RGB array
//! (from `Image.open(...).convert("RGB")`), but `cv2.dnn.blobFromImage`
//! is called with `swapRB=True` — which, applied to an already-RGB
//! array, actually *produces BGR* in the network input. That is exactly
//! reproduced here (channel order reversed, not "corrected"): this is
//! what the trained/tested production pipeline does, bug or not.

use crate::cv2_resize::resize_rgb;

const INPUT_SIZE: usize = 640;
const NMS_THRESH: f32 = 0.4;
const DET_THRESH: f32 = 0.5;
const STRIDES: [usize; 3] = [8, 16, 32];
const NUM_ANCHORS: usize = 2;
const INPUT_MEAN: f32 = 127.5;
const INPUT_STD: f32 = 128.0;

#[derive(Debug, Clone)]
pub struct Detection {
    pub bbox: [f32; 4], // x1, y1, x2, y2, in original image coordinates
    pub score: f32,
    pub kps: [[f32; 2]; 5],
}

/// Letterbox into a 640x640 canvas: resize preserving aspect ratio so
/// the image fits, zero-pad the remainder. Returns (canvas, det_scale).
/// Port of `SCRFD.detect`'s resize/pad block.
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

/// `cv2.dnn.blobFromImage(canvas, 1/128, (640,640), (127.5,...), swapRB=True)`:
/// NCHW, channel order reversed (see module docs), `(v - 127.5) / 128`.
fn to_blob(canvas: &[u8]) -> Vec<f32> {
    let n = INPUT_SIZE * INPUT_SIZE;
    let mut out = vec![0.0f32; 3 * n];
    for i in 0..n {
        let r = canvas[i * 3] as f32;
        let g = canvas[i * 3 + 1] as f32;
        let b = canvas[i * 3 + 2] as f32;
        // swapRB on an RGB source yields BGR channel order.
        out[i] = (b - INPUT_MEAN) / INPUT_STD; // channel 0 = B
        out[n + i] = (g - INPUT_MEAN) / INPUT_STD; // channel 1 = G
        out[2 * n + i] = (r - INPUT_MEAN) / INPUT_STD; // channel 2 = R
    }
    out
}

fn anchor_centers(height: usize, width: usize, stride: usize) -> Vec<[f32; 2]> {
    let mut centers = Vec::with_capacity(height * width * NUM_ANCHORS);
    for y in 0..height {
        for x in 0..width {
            let c = [(x * stride) as f32, (y * stride) as f32];
            for _ in 0..NUM_ANCHORS {
                centers.push(c);
            }
        }
    }
    centers
}

type StrideDecoded = (Vec<f32>, Vec<[f32; 4]>, Vec<[[f32; 2]; 5]>);

/// Decode raw network outputs (already sliced per stride) into
/// detections above `DET_THRESH`, in the 640x640 letterbox frame.
fn decode_stride(
    scores: &[f32],
    bbox_preds: &[f32],
    kps_preds: &[f32],
    stride: usize,
    input_dim: usize,
) -> StrideDecoded {
    let grid = input_dim / stride;
    let centers = anchor_centers(grid, grid, stride);

    let mut out_scores = Vec::new();
    let mut out_boxes = Vec::new();
    let mut out_kps = Vec::new();

    for (i, &score) in scores.iter().enumerate() {
        if score < DET_THRESH {
            continue;
        }
        let c = centers[i];
        let bp = &bbox_preds[i * 4..i * 4 + 4];
        let bbox = [
            c[0] - bp[0] * stride as f32,
            c[1] - bp[1] * stride as f32,
            c[0] + bp[2] * stride as f32,
            c[1] + bp[3] * stride as f32,
        ];
        let kp = &kps_preds[i * 10..i * 10 + 10];
        let mut kps = [[0.0f32; 2]; 5];
        for j in 0..5 {
            kps[j][0] = c[0] + kp[j * 2] * stride as f32;
            kps[j][1] = c[1] + kp[j * 2 + 1] * stride as f32;
        }
        out_scores.push(score);
        out_boxes.push(bbox);
        out_kps.push(kps);
    }
    (out_scores, out_boxes, out_kps)
}

/// Greedy NMS matching SCRFD's `nms()`: `+1` area convention, IoU <= thresh kept.
fn nms(dets: &[(f32, [f32; 4])]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..dets.len()).collect();
    order.sort_by(|&a, &b| dets[b].0.partial_cmp(&dets[a].0).unwrap());

    let areas: Vec<f32> = dets
        .iter()
        .map(|(_, b)| (b[2] - b[0] + 1.0) * (b[3] - b[1] + 1.0))
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
            let w = (xx2 - xx1 + 1.0).max(0.0);
            let h = (yy2 - yy1 + 1.0).max(0.0);
            let inter = w * h;
            let ovr = inter / (areas[i] + areas[j] - inter);
            if ovr > NMS_THRESH {
                suppressed[j] = true;
            }
        }
    }
    keep
}

/// Run detection end to end given the 9 raw network output tensors, in
/// the exact order the ONNX graph emits them: scores@8,16,32,
/// bbox@8,16,32, kps@8,16,32 (each a flat row-major `[N, C]` buffer).
pub fn decode_outputs(outputs: &[&[f32]; 9], det_scale: f32) -> Vec<Detection> {
    let mut all_scores = Vec::new();
    let mut all_boxes = Vec::new();
    let mut all_kps = Vec::new();

    for (idx, &stride) in STRIDES.iter().enumerate() {
        let (scores, boxes, kps) = decode_stride(
            outputs[idx],
            outputs[idx + 3],
            outputs[idx + 6],
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
        // 4:3 landscape, matches the SCRFD detect() manual trace: 640x480 -> new (640,480).
        let pixels = vec![100u8; 640 * 480 * 3];
        let (canvas, scale) = letterbox(&pixels, 640, 480);
        assert_eq!(canvas.len(), INPUT_SIZE * INPUT_SIZE * 3);
        assert!((scale - 1.0).abs() < 1e-6);
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
}
