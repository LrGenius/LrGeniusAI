//! The style blend over the real `getDevelopSettings()` fixtures in
//! `testdata/develop/lua/`: every generated fixture is canonicalised the way
//! the training route stores it, blended with equal weight, and turned into
//! the recipe the installed plugin reads.
//!
//! Checks that the fields added in step 1f (detail, vignette and grain
//! sliders, HSL, colour grading shadows/highlights/balance, the parametric
//! curve splits) come out of real data, not only out of hand-built maps, and
//! that keys some fixtures lack are renormalised over the fixtures that have
//! them.

use std::path::PathBuf;

use lrg_analysis::style_engine::{
    canonical_to_edit_recipe, interpolate_recipes, TrainingCandidate,
};
use lrg_analysis::training::{canonical_key, canonicalize_develop_settings};
use lrg_develop::FileKindHint;
use serde_json::Value;

fn lua_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata/develop/lua")
}

/// The generated fixtures (real rows, scrubbed): `(name, blob)`, sorted.
fn generated_fixtures() -> Vec<(String, Value)> {
    let manifest: Value = serde_json::from_str(
        &std::fs::read_to_string(lua_dir().join("manifest.json")).expect("manifest"),
    )
    .expect("manifest is JSON");
    let mut out: Vec<(String, Value)> = manifest["files"]
        .as_array()
        .expect("manifest files")
        .iter()
        .map(|f| {
            let name = f["file"].as_str().unwrap().to_string();
            let text = std::fs::read_to_string(lua_dir().join(&name)).unwrap();
            (name, serde_json::from_str(&text).unwrap())
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

fn candidates() -> Vec<(TrainingCandidate, f64)> {
    generated_fixtures()
        .into_iter()
        .map(|(name, blob)| {
            let c = canonicalize_develop_settings(&blob, FileKindHint::Raw)
                .unwrap_or_else(|e| panic!("{name}: {e}"));
            let candidate = TrainingCandidate {
                photo_id: name,
                canonical_settings: c.settings,
                white_balance: c.white_balance,
                ..Default::default()
            };
            (candidate, 1.0)
        })
        .collect()
}

#[test]
fn real_blobs_carry_the_new_canonical_keys() {
    let pool = candidates();
    assert!(pool.len() >= 10, "only {} generated fixtures", pool.len());
    for name in [
        "sharpen_radius",
        "sharpen_detail",
        "noise_reduction_detail",
        "color_noise_reduction_smoothness",
        "vignette_midpoint",
        "vignette_feather",
        "grain_size",
        "grain_roughness",
        "hsl_blue_saturation",
        "color_grading_shadows_saturation",
        "color_grading_balance",
        "tone_curve_shadow_split",
        "tone_curve_highlight_split",
    ] {
        let with = pool
            .iter()
            .filter(|(c, _)| c.canonical_settings.contains_key(name))
            .count();
        assert!(with > 0, "no fixture yields {name}");
    }
    // Colour grading midtones/global/blending never become canonical: the
    // installed plugin would warn about them.
    for (c, _) in &pool {
        assert!(
            !c.canonical_settings
                .keys()
                .any(|k| k.contains("midtone_hue") || k.contains("blending")),
            "{}: {:?}",
            c.photo_id,
            c.canonical_settings.keys().collect::<Vec<_>>()
        );
    }
}

#[test]
fn blending_real_blobs_gives_the_recipe_the_installed_plugin_reads() {
    let pool = candidates();
    let blend = interpolate_recipes(&pool);
    let recipe = canonical_to_edit_recipe(&blend.settings, "", None);
    let global = &recipe["global"];

    for field in [
        "sharpen_radius",
        "sharpen_detail",
        "sharpen_masking",
        "noise_reduction_detail",
        "noise_reduction_contrast",
        "color_noise_reduction_detail",
        "color_noise_reduction_smoothness",
        "vignette_midpoint",
        "vignette_roundness",
        "vignette_feather",
        "vignette_highlights",
        "grain_size",
        "grain_roughness",
    ] {
        assert!(global[field].is_number(), "global.{field} in {global}");
    }
    for colour in [
        "red", "orange", "yellow", "green", "aqua", "blue", "purple", "magenta",
    ] {
        for field in ["hue", "saturation", "luminance"] {
            assert!(
                global["hsl"][colour][field].is_i64(),
                "hsl.{colour}.{field} in {global}"
            );
        }
    }
    let grading = global["color_grading"].as_object().unwrap();
    let mut keys: Vec<&str> = grading.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, ["balance", "highlights", "shadows"], "{grading:?}");
    for zone in ["shadows", "highlights"] {
        let z = grading[zone].as_object().unwrap();
        assert!(z.keys().all(|k| k == "hue" || k == "saturation"), "{z:?}");
        let hue = z["hue"].as_i64().expect("toned fixtures give a hue");
        assert!((0..360).contains(&hue), "{zone} hue {hue}");
    }
    let curve = global["tone_curve"].as_object().unwrap();
    let (s, m, h) = (
        curve["shadow_split"].as_i64().unwrap(),
        curve["midtone_split"].as_i64().unwrap(),
        curve["highlight_split"].as_i64().unwrap(),
    );
    assert!(s < m && m < h, "{s} {m} {h}");
    assert!(blend.warnings.is_empty(), "{:?}", blend.warnings);
}

#[test]
fn keys_missing_from_some_fixtures_are_renormalised_over_the_others() {
    let pool = candidates();
    let blended = interpolate_recipes(&pool).settings;
    // Every plain (linear) key that some fixture lacks: its blend is the mean
    // over the fixtures that carry it, not over all of them.
    let mut checked = 0;
    let mut distinguishing = 0;
    for key in lrg_analysis::training::canonical_keys() {
        if key.recipe_path[0] == "color_grading"
            && matches!(key.recipe_path.last(), Some(&"hue") | Some(&"saturation"))
        {
            continue; // a zone's mean colour vector, see below
        }
        let values: Vec<f64> = pool
            .iter()
            .filter_map(|(c, _)| c.canonical_settings.get(key.name).and_then(Value::as_f64))
            .collect();
        if values.is_empty() || values.len() == pool.len() {
            continue;
        }
        let mean = values.iter().sum::<f64>() / values.len() as f64;
        let over_all = values.iter().sum::<f64>() / pool.len() as f64;
        let got = blended[key.name].as_f64().unwrap();
        assert!(
            (got - mean).abs() <= 0.5,
            "{}: {got} is not the mean {mean} over the fixtures that have it",
            key.name
        );
        checked += 1;
        if (mean - over_all).abs() > 0.5 {
            distinguishing += 1;
        }
    }
    assert!(checked > 0, "no fixture lacks a canonical key");
    assert!(
        distinguishing > 0,
        "the fixtures no longer tell renormalised and naive means apart"
    );
}

#[test]
fn toning_hues_from_real_blobs_ignore_untoned_examples() {
    // Only the fixtures with a shadow saturation above 0 weigh in; the ones
    // that keep a stale hue at saturation 0 must not move the mean.
    let pool = candidates();
    let toned: Vec<(TrainingCandidate, f64)> = pool
        .iter()
        .filter(|(c, _)| {
            c.canonical_settings
                .get("color_grading_shadows_saturation")
                .and_then(Value::as_f64)
                .is_some_and(|s| s > 0.0)
        })
        .cloned()
        .collect();
    assert!(!toned.is_empty() && toned.len() < pool.len());
    assert!(
        pool.iter().any(|(c, _)| {
            let get = |k: &str| c.canonical_settings.get(k).and_then(Value::as_f64);
            get("color_grading_shadows_saturation") == Some(0.0)
                && get("color_grading_shadows_hue").is_some_and(|h| h != 0.0)
        }),
        "this test needs a fixture with a stale hue at saturation 0"
    );
    let all = interpolate_recipes(&pool).settings;
    let only_toned = interpolate_recipes(&toned).settings;
    assert_eq!(
        all["color_grading_shadows_hue"],
        only_toned["color_grading_shadows_hue"]
    );
    let key = canonical_key("color_grading_shadows_hue").unwrap();
    assert_eq!(key.spec.name, "SplitToningShadowHue");
    assert_eq!(key.recipe_path, ["color_grading", "shadows", "hue"]);
}
