//! The face state pipeline (`lrg_ml::face_state`) against Python MediaPipe's
//! own FaceLandmarker on the reference portrait, as captured by
//! `scripts/export_face_state.py --goldens` into
//! `testdata/face_state_goldens.json`.
//!
//! Needs the YuNet detector (the `face` family) to find the face the way the
//! backend does, and the two ONNX files `export_face_state.py` produces (the
//! `facestate` family), looked up where the server looks
//! (`LRG_FACE_MESH_ONNX` / `LRG_FACE_BLENDSHAPES_ONNX`, else the models
//! directory). Skips without them unless `LRG_REQUIRE_GOLDENS` names the
//! family — see `common::assets_ready`.
//!
//! The comparison is not exact and should not be: MediaPipe finds the face
//! with its own detector (BlazeFace), this pipeline with YuNet, so the first
//! region differs; the second, built from the mesh's own landmarks, converges.
//! The blendshape model *is* compared exactly, on MediaPipe's landmarks with
//! the eyelids moved shut — the closed-eye path no photograph here shows.

use lrg_imaging::cull_config::FaceMetricsConfig;
use lrg_ml::face_state::{
    EyeState, FaceGeometry, FaceStateModel, BLENDSHAPE_LANDMARKS, BLENDSHAPE_NAMES,
};
use lrg_ml::faces::{FaceModel, FacePass};

mod common;

fn goldens() -> serde_json::Value {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../testdata/face_state_goldens.json"
    );
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn model() -> Option<FaceStateModel> {
    let paths = lrg_ml::model_paths::resolve_face_state();
    common::assets_ready(
        "facestate",
        paths.all_exist(),
        &format!(
            "face state ONNX models not found at {} / {}",
            paths.mesh_onnx.display(),
            paths.blendshapes_onnx.display()
        ),
    )
    .then(|| FaceStateModel::new(paths))
}

fn blendshape(g: &serde_json::Value, name: &str) -> f64 {
    g[name]
        .as_f64()
        .unwrap_or_else(|| panic!("no {name} in goldens"))
}

#[test]
fn the_landmark_subset_matches_the_exporter() {
    let g = goldens();
    let recorded: Vec<usize> = g["blendshape_landmarks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_u64().unwrap() as usize)
        .collect();
    assert_eq!(recorded, BLENDSHAPE_LANDMARKS.to_vec());
    let names: Vec<&str> = g["blendshapes"]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    for name in names {
        assert!(
            BLENDSHAPE_NAMES.contains(&name),
            "unknown blendshape {name}"
        );
    }
}

#[test]
fn closed_eyelids_through_the_blendshape_model_match_the_reference() {
    let Some(model) = model() else { return };
    let g = goldens();
    let mut landmarks: Vec<[f64; 3]> = g["landmarks_px"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| {
            let p = p.as_array().unwrap();
            [p[0].as_f64().unwrap(), p[1].as_f64().unwrap(), 0.0]
        })
        .collect();
    for pair in g["eyelid_pairs"].as_array().unwrap() {
        let (upper, lower) = (
            pair[0].as_u64().unwrap() as usize,
            pair[1].as_u64().unwrap() as usize,
        );
        landmarks[upper] = landmarks[lower];
    }
    let shapes = model.blendshapes_for(&landmarks).unwrap();
    let want = &g["eyes_closed_blendshapes"];
    for (i, name) in BLENDSHAPE_NAMES.iter().enumerate() {
        let w = blendshape(want, name);
        assert!(
            (shapes[i] as f64 - w).abs() < 1e-4,
            "{name}: got {}, reference {w}",
            shapes[i]
        );
    }
}

#[test]
fn the_reference_portrait_reads_like_mediapipe() {
    let Some(model) = model() else { return };
    let face_paths = lrg_ml::model_paths::resolve_face();
    if !common::assets_ready(
        "face",
        face_paths.all_exist(),
        "face ONNX models not found (needed to find the face)",
    ) {
        return;
    }
    let faces = FaceModel::new(face_paths);
    let img = image::open(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../testdata/astronaut.jpg"
    ))
    .unwrap()
    .to_rgb8();
    let (w, h) = (img.width() as usize, img.height() as usize);
    let rgb = img.into_raw();
    let cfg = FaceMetricsConfig::defaults();
    let detected = faces
        .detect_faces(&rgb, w, h, &cfg, FacePass::QualityOnly)
        .unwrap();
    assert_eq!(detected.len(), 1, "one face on the reference portrait");
    let geometry = FaceGeometry {
        bbox: detected[0].bbox.map(|v| v as f64),
        kps: detected[0].kps,
    };
    let state = model.analyze(&rgb, w, h, &[geometry], &cfg).unwrap()[0]
        .clone()
        .expect("the face is large enough to analyse");

    let g = goldens();
    let reference: Vec<[f64; 2]> = g["landmarks_px"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| [p[0].as_f64().unwrap(), p[1].as_f64().unwrap()])
        .collect();
    let iod = ((reference[33][0] - reference[263][0]).powi(2)
        + (reference[33][1] - reference[263][1]).powi(2))
    .sqrt();
    let mean_err = state
        .landmarks
        .iter()
        .zip(&reference)
        .map(|(a, b)| ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt())
        .sum::<f64>()
        / reference.len() as f64;
    assert!(
        mean_err / iod < 0.03,
        "landmarks {:.2}% of the interocular distance off MediaPipe's",
        100.0 * mean_err / iod
    );

    let want = &g["blendshapes"];
    for name in [
        "eyeBlinkLeft",
        "eyeBlinkRight",
        "mouthSmileLeft",
        "mouthSmileRight",
    ] {
        let i = BLENDSHAPE_NAMES.iter().position(|n| *n == name).unwrap();
        let (got, w) = (state.blendshapes[i] as f64, blendshape(want, name));
        assert!((got - w).abs() < 0.08, "{name}: got {got}, MediaPipe {w}");
    }
    assert!(state.presence > 0.9, "presence {}", state.presence);
    assert_eq!(state.eye_state, EyeState::Open);
    assert!(
        state.smile > 0.7,
        "the astronaut is smiling: {}",
        state.smile
    );
    assert!(
        state.yaw_degrees.abs() < 30.0,
        "near frontal: {}",
        state.yaw_degrees
    );
}
