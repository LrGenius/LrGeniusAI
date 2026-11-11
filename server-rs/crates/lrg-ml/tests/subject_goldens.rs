//! The subject finder (`lrg_ml::subject`) against the exported model run from
//! Python on the reference portrait, as captured by
//! `scripts/export_subject_rfdetr.py --goldens` into
//! `testdata/subject_goldens.json`.
//!
//! The preprocessing is checked first, at a few fixed positions of the input
//! tensor, so a port that resizes or normalises differently fails there rather
//! than as a slightly different score. The detections are then compared on
//! class, score, box and mask size.
//!
//! Needs `subject_rfdetr_seg_nano.onnx`, looked up where the server looks
//! (`LRG_SUBJECT_ONNX`, else the models directory). Skips without it unless
//! `LRG_REQUIRE_GOLDENS` names `subject` — see `common::assets_ready`.

use lrg_ml::subject::{preprocess, SubjectKind, SubjectModel, INPUT_SIZE};

mod common;

fn goldens() -> serde_json::Value {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../testdata/subject_goldens.json"
    );
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn portrait() -> (Vec<u8>, usize, usize) {
    let img = image::open(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../testdata/astronaut.jpg"
    ))
    .unwrap()
    .to_rgb8();
    let (w, h) = (img.width() as usize, img.height() as usize);
    (img.into_raw(), w, h)
}

/// Needs no model: the input tensor alone.
#[test]
fn the_input_tensor_matches_the_exporter() {
    let g = goldens();
    assert_eq!(g["input_size"].as_u64(), Some(INPUT_SIZE as u64));
    let (rgb, w, h) = portrait();
    let tensor = preprocess(&rgb, w, h);
    let plane = INPUT_SIZE * INPUT_SIZE;
    for sample in g["input_samples"].as_array().unwrap() {
        let s = sample.as_array().unwrap();
        let (c, y, x) = (
            s[0].as_u64().unwrap() as usize,
            s[1].as_u64().unwrap() as usize,
            s[2].as_u64().unwrap() as usize,
        );
        let want = s[3].as_f64().unwrap() as f32;
        let got = tensor[c * plane + y * INPUT_SIZE + x];
        assert!(
            (got - want).abs() < 1e-3,
            "input[{c},{y},{x}] = {got}, the exporter has {want}"
        );
    }
}

#[test]
fn the_portrait_is_found_as_a_person_like_the_exporter_found_it() {
    let paths = lrg_ml::model_paths::resolve_subject();
    if !common::assets_ready(
        "subject",
        paths.all_exist(),
        &format!("subject ONNX model not found at {}", paths.onnx.display()),
    ) {
        return;
    }
    let model = SubjectModel::new(paths);
    let (rgb, w, h) = portrait();
    let found = model.detect(&rgb, w, h).expect("detection runs");
    let want = goldens()["detections"].as_array().unwrap().clone();
    assert_eq!(found.len(), want.len(), "{found:?}");
    for (got, want) in found.iter().zip(&want) {
        assert_eq!(got.class_id as u64, want["class_id"].as_u64().unwrap());
        let score = want["score"].as_f64().unwrap() as f32;
        assert!((got.score - score).abs() < 0.01, "{} vs {score}", got.score);
        for (g, w) in got.bbox.iter().zip(want["box"].as_array().unwrap()) {
            let w = w.as_f64().unwrap() as f32;
            assert!((g - w.clamp(0.0, 1.0)).abs() < 0.01, "box {:?}", got.bbox);
        }
        let area = want["mask_area"].as_f64().unwrap() as f32;
        assert!(
            (got.mask_area() - area).abs() < 0.01,
            "mask {} vs {area}",
            got.mask_area()
        );
    }
    assert_eq!(found[0].kind, SubjectKind::Person);
}
