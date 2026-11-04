//! Prints what the face state pipeline reads on the photos given, one JSON
//! object per face, for measuring a new signal or threshold against real
//! photographs before it goes into the ranking.
//!
//! ```bash
//! cargo run --release -p lrg-ml --example face_state_probe -- a.jpg b.jpg > faces.jsonl
//! cargo run --release -p lrg-ml --example face_state_probe -- --embed a.jpg > faces.jsonl
//! ```
//!
//! `--embed` adds each face's identity embedding, for scoring clustering.
//!
//! Needs the YuNet detector and the two face state models, found where the
//! server finds them (`LRG_FACE_MESH_ONNX` / `LRG_FACE_BLENDSHAPES_ONNX`, else
//! the models directory). Faces are detected at the size the file has; the
//! server detects on a copy of at most 2048 px and then analyses the original.

use lrg_imaging::cull_config::FaceMetricsConfig;
use lrg_ml::face_state::{FaceGeometry, FaceStateModel, BLENDSHAPE_NAMES};
use lrg_ml::faces::{FaceModel, FacePass};
use serde_json::{json, Map, Value};

fn main() {
    let cfg = FaceMetricsConfig::defaults();
    let faces = FaceModel::new(lrg_ml::model_paths::resolve_face());
    let state = FaceStateModel::new(lrg_ml::model_paths::resolve_face_state());
    assert!(faces.is_cached(), "face detection models not found");
    assert!(state.is_cached(), "face state models not found");

    let args: Vec<String> = std::env::args().skip(1).collect();
    let embed = args.iter().any(|a| a == "--embed");
    let pass = if embed {
        FacePass::Full
    } else {
        FacePass::QualityOnly
    };
    for path in args.iter().filter(|a| *a != "--embed") {
        let img = match image::open(path) {
            Ok(img) => img.to_rgb8(),
            Err(e) => {
                eprintln!("{path}: {e}");
                continue;
            }
        };
        let (w, h) = (img.width() as usize, img.height() as usize);
        let rgb = img.into_raw();
        let detected = match faces.detect_faces(&rgb, w, h, &cfg, pass) {
            Ok(detected) => detected,
            Err(e) => {
                eprintln!("{path}: {e}");
                continue;
            }
        };
        let geometry: Vec<FaceGeometry> = detected
            .iter()
            .map(|f| FaceGeometry {
                bbox: f.bbox.map(|v| v as f64),
                kps: f.kps,
            })
            .collect();
        let states = match state.analyze(&rgb, w, h, &geometry, &cfg) {
            Ok(states) => states,
            Err(e) => {
                eprintln!("{path}: {e}");
                continue;
            }
        };
        for (i, (face, st)) in detected.iter().zip(states).enumerate() {
            let Some(st) = st else { continue };
            let shapes: Map<String, Value> = BLENDSHAPE_NAMES
                .iter()
                .zip(st.blendshapes)
                .map(|(name, v)| (name.to_string(), json!(v)))
                .collect();
            println!(
                "{}",
                json!({
                    "file": path,
                    "face": i,
                    "width": w,
                    "height": h,
                    "bbox": face.bbox,
                    "det_score": face.det_score,
                    "area_ratio": face.area_ratio,
                    "presence": st.presence,
                    "eye_state": st.eye_state.as_str(),
                    "eye_closed": st.eye_closed,
                    "eye_squint": st.eye_squint,
                    "smile": st.smile,
                    "gaze_off_lens": st.gaze_off_lens,
                    "yaw": st.pose.yaw,
                    "pitch": st.pose.pitch,
                    "roll": st.pose.roll,
                    "light": st.light.map(|l| json!({
                        "luminance": l.luminance,
                        "against_surround_stops": l.against_surround_stops,
                        "across_face_stops": l.across_face_stops,
                        "blown": l.blown,
                    })),
                    "sharpness": face.sharpness,
                    "occlusion": face.occlusion,
                    "embedding": embed.then(|| face.embedding.clone()),
                    "blendshapes": shapes,
                    "landmarks": st.landmarks,
                })
            );
        }
    }
}
