//! End-to-end verification of `group_and_sort_images` against real
//! output from `services/chroma.py::group_and_sort_images`, generated
//! by `make_grouping_goldens.py` against a real Chroma-backed call
//! (not a hand-derivation) covering: a near-duplicate burst pair (one
//! sharp winner, one weaker/rejected alternate), an isolated single
//! photo far away in time, and an edge case with no capture_time/phash.

use lrg_analysis::grouping::{group_and_sort_images, GroupingInput};
use serde_json::Value;

fn load(path: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn matches_real_python_output() {
    let inputs_path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../testdata/grouping_goldens_inputs.json"
    );
    let expected_path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../testdata/grouping_goldens.json"
    );
    let inputs = load(inputs_path);
    let expected = load(expected_path);

    let records: Vec<GroupingInput> = inputs
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            let embedding: Vec<f32> = r["embedding"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_f64().unwrap() as f32)
                .collect();
            let phash = r["phash"]
                .as_str()
                .map(|s| u64::from_str_radix(s, 16).unwrap());
            let metadata = r["metadata"].as_object().unwrap().clone();
            GroupingInput {
                photo_id: r["photo_id"].as_str().unwrap().to_string(),
                filename: r["filename"].as_str().unwrap().to_string(),
                capture_time: r["capture_time"].as_f64(),
                embedding: Some(embedding),
                phash,
                metadata,
            }
        })
        .collect();

    // Same call as the golden generator: phash_threshold="auto",
    // clip_threshold="auto" (both None here), time_delta=1, "default" preset.
    let groups = group_and_sort_images(records, None, None, 1, "default");

    let expected_groups = expected.as_array().unwrap();
    assert_eq!(groups.len(), expected_groups.len(), "group count mismatch");

    for (got, want) in groups.iter().zip(expected_groups) {
        assert_eq!(got.group_id, want["group_id"].as_str().unwrap());
        assert_eq!(got.group_type, want["group_type"].as_str().unwrap());
        assert_eq!(
            got.winner_photo_id,
            want["winner_photo_id"].as_str().unwrap()
        );
        let want_ids: Vec<String> = want["photo_ids"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        assert_eq!(got.photo_ids, want_ids, "group {}: photo_ids", got.group_id);

        let want_photos = want["photos"].as_array().unwrap();
        assert_eq!(got.photos.len(), want_photos.len());
        for (gp, wp) in got.photos.iter().zip(want_photos) {
            assert_eq!(
                gp.photo_id,
                wp["photo_id"].as_str().unwrap(),
                "group {} photo order",
                got.group_id
            );
            assert_eq!(gp.rank, wp["rank"].as_u64().unwrap() as usize);
            assert_eq!(gp.winner, wp["winner"].as_bool().unwrap());
            assert_eq!(
                gp.reject_candidate,
                wp["reject_candidate"].as_bool().unwrap(),
                "{}: reject_candidate",
                gp.photo_id
            );
            let want_score = wp["cull_score"].as_f64().unwrap();
            assert!(
                (gp.cull_score - want_score).abs() < 1e-4,
                "{}: cull_score {} vs {want_score}",
                gp.photo_id,
                gp.cull_score
            );
            let want_codes: Vec<String> = wp["reason_codes"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap().to_string())
                .collect();
            assert_eq!(gp.reason_codes, want_codes, "{}: reason_codes", gp.photo_id);
            assert_eq!(
                gp.explanation,
                wp["explanation"].as_str().unwrap(),
                "{}: explanation",
                gp.photo_id
            );
        }
        println!(
            "{} ({}) OK: {:?}",
            got.group_id, got.group_type, got.photo_ids
        );
    }
}
