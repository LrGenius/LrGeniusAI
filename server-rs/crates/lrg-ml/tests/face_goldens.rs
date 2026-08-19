//! End-to-end golden verification of the YuNet + FaceNet face pipeline
//! against the reference implementations — OpenCV's `FaceDetectorYN` and
//! `facenet-pytorch`'s Inception-ResNet-v1 — as captured by
//! `testdata/make_face_goldens.py` on a real photo (skimage's astronaut
//! sample, a genuine face).
//!
//! Requires the ONNX files `scripts/export_face_models.py` produces. They are
//! looked up exactly where the server looks (`LRG_YUNET_ONNX` /
//! `LRG_FACENET_ONNX`, else `~/.cache/lrgenius/models/`), so a machine that
//! has run the plugin's model download already has them. Skips when not found.

use lrg_ml::faces::{FaceModel, FaceModelPaths};

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    dot / (na * nb)
}

fn iou(a: [i64; 4], b: &[f64]) -> f64 {
    let b = [b[0] as i64, b[1] as i64, b[2] as i64, b[3] as i64];
    let x1 = a[0].max(b[0]);
    let y1 = a[1].max(b[1]);
    let x2 = a[2].min(b[2]);
    let y2 = a[3].min(b[3]);
    let inter = (x2 - x1).max(0) * (y2 - y1).max(0);
    let area_a = (a[2] - a[0]) * (a[3] - a[1]);
    let area_b = (b[2] - b[0]) * (b[3] - b[1]);
    inter as f64 / (area_a + area_b - inter) as f64
}

#[test]
fn detects_and_embeds_real_face_matching_reference_models() {
    let paths = lrg_ml::model_paths::resolve_face();
    if !paths.all_exist() {
        eprintln!(
            "face ONNX models not found at {} / {}; skipping face golden test",
            paths.det_onnx.display(),
            paths.rec_onnx.display()
        );
        return;
    }
    let model = FaceModel::new(FaceModelPaths {
        det_onnx: paths.det_onnx,
        rec_onnx: paths.rec_onnx,
    });

    let golden_path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../testdata/face_goldens.json"
    );
    let goldens: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(golden_path).unwrap()).unwrap();
    let case = &goldens["astronaut_512"];

    let img_path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../testdata/astronaut.jpg");
    let img_bytes = std::fs::read(img_path).expect("astronaut.jpg fixture missing");
    let decoded = image::load_from_memory(&img_bytes).unwrap().to_rgb8();
    let (width, height) = (decoded.width() as usize, decoded.height() as usize);
    let pixels = decoded.into_raw();

    let got = model
        .detect_faces(
            &pixels,
            width,
            height,
            &lrg_imaging::cull_config::FaceMetricsConfig::defaults(),
            lrg_ml::faces::FacePass::Full,
        )
        .unwrap();
    let expected_faces = case["faces"].as_array().unwrap();
    assert_eq!(got.len(), expected_faces.len(), "face count mismatch");

    for (face, expected) in got.iter().zip(expected_faces) {
        let expected_bbox: Vec<f64> = expected["bbox"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_f64().unwrap())
            .collect();
        let box_iou = iou(face.bbox, &expected_bbox);
        assert!(
            box_iou > 0.9,
            "bbox IoU {box_iou} too low: {:?} vs {expected_bbox:?}",
            face.bbox
        );

        let expected_score = expected["det_score"].as_f64().unwrap();
        assert!(
            (face.det_score - expected_score).abs() < 0.05,
            "det_score {} vs {expected_score}",
            face.det_score
        );

        let expected_emb: Vec<f32> = expected["embedding"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_f64().unwrap() as f32)
            .collect();
        assert_eq!(expected_emb.len(), 512, "golden embedding must be 512-d");
        // The whole chain is compared here, not just the recognizer: a
        // landmark decoded a few pixels off, or the alignment template scaled
        // differently, moves this well below 0.99 while leaving the bbox
        // assertion above perfectly happy.
        let cos = cosine(&face.embedding, &expected_emb);
        assert!(cos > 0.99, "embedding cosine {cos} too low");

        println!(
            "bbox_iou={box_iou:.4} det_score_diff={:.4} embedding_cosine={cos:.4}",
            (face.det_score - expected_score).abs()
        );
    }
}
