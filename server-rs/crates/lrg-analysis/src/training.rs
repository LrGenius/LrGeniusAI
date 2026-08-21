//! Port of `services/training.py`'s pure logic: exposure-metric
//! computation, EXIF-derived categorical buckets, develop-settings
//! canonicalization for interpolation, scene-tag thresholding, and the
//! style-profile stats aggregation. I/O (the `edit_training` LanceDB
//! table, CLIP embedding) lives in `lrg-api::routes::training`.

use lrg_imaging::pil_resample::{resize_plane, Filter};
use serde_json::{json, Map, Value};

/// Scene-type probe texts for CLIP zero-shot classification, in the same
/// order as Python's `_SCENE_PROBES` dict (order matters for deterministic
/// tag ordering).
pub const SCENE_PROBES: &[(&str, &str)] = &[
    ("scene_portrait", "a portrait photo of a person"),
    ("scene_landscape", "a landscape or nature photo"),
    ("scene_architecture", "an architectural or building photo"),
    ("scene_wildlife", "a wildlife or animal photo"),
    ("scene_event", "an event, wedding, or celebration photo"),
    ("scene_street", "a street photography or urban scene photo"),
    ("scene_macro", "a macro or close-up detail photo"),
    ("scene_interior", "an interior or indoor room photo"),
    ("scene_exterior", "an outdoor or exterior photo"),
    (
        "scene_golden_hour",
        "a photo taken at golden hour or sunset",
    ),
    ("scene_studio", "a studio or controlled-light photo"),
    ("scene_action", "an action, sports, or motion photo"),
];
pub const SCENE_THRESHOLD: f64 = 0.22;

/// Port of `focal_length_bucket`.
pub fn focal_length_bucket(focal_length_mm: Option<f64>) -> &'static str {
    let Some(fl) = focal_length_mm else {
        return "unknown";
    };
    if fl < 20.0 {
        "ultra_wide"
    } else if fl < 35.0 {
        "wide"
    } else if fl < 70.0 {
        "normal"
    } else if fl < 135.0 {
        "short_tele"
    } else if fl < 300.0 {
        "tele"
    } else {
        "super_tele"
    }
}

/// Port of `time_of_day_bucket`. Takes the already-resolved local hour
/// (0..23); the Unix-timestamp -> local-hour conversion happens at the
/// API layer, keeping this crate free of a timezone dependency.
pub fn time_of_day_bucket_for_hour(hour: Option<u32>) -> &'static str {
    let Some(hour) = hour else {
        return "unknown";
    };
    if (5..8).contains(&hour) {
        "dawn"
    } else if (8..12).contains(&hour) {
        "morning"
    } else if (12..17).contains(&hour) {
        "afternoon"
    } else if (17..20).contains(&hour) {
        "evening"
    } else {
        "night"
    }
}

const LR_TO_CANONICAL: &[(&str, &str)] = &[
    ("Exposure2012", "exposure"),
    ("Contrast2012", "contrast"),
    ("Highlights2012", "highlights"),
    ("Shadows2012", "shadows"),
    ("Whites2012", "whites"),
    ("Blacks2012", "blacks"),
    ("Temp", "temperature"),
    ("Tint", "tint"),
    ("Texture", "texture"),
    ("Clarity2012", "clarity"),
    ("Dehaze", "dehaze"),
    ("Vibrance", "vibrance"),
    ("Saturation", "saturation"),
    ("Sharpness", "sharpening"),
    ("LuminanceSmoothing", "noise_reduction"),
    ("ColorNoiseReduction", "color_noise_reduction"),
    ("PostCropVignetteAmount", "vignette"),
    ("GrainAmount", "grain"),
    ("ParametricHighlights", "tone_curve_highlights"),
    ("ParametricLights", "tone_curve_lights"),
    ("ParametricDarks", "tone_curve_darks"),
    ("ParametricShadows", "tone_curve_shadows"),
];

/// Port of `normalize_develop_settings_for_style`.
pub fn normalize_develop_settings_for_style(
    develop_settings: &Map<String, Value>,
) -> Map<String, Value> {
    let mut canonical = Map::new();
    for &(lr_key, canon_key) in LR_TO_CANONICAL {
        if let Some(raw) = develop_settings.get(lr_key).and_then(Value::as_f64) {
            canonical.insert(
                canon_key.to_string(),
                json!((raw * 10000.0).round() / 10000.0),
            );
        }
    }
    canonical
}

fn round4(x: f64) -> f64 {
    (x * 10000.0).round() / 10000.0
}

fn clamp01(x: f64) -> f64 {
    x.clamp(0.0, 1.0)
}

fn percentile_linear(sorted: &[f64], p: f64) -> f64 {
    let n = sorted.len();
    if n == 0 {
        return 0.0;
    }
    if n == 1 {
        return sorted[0];
    }
    let rank = (p / 100.0) * (n as f64 - 1.0);
    let lo = rank.floor() as usize;
    let hi = rank.ceil() as usize;
    if lo == hi {
        return sorted[lo];
    }
    let frac = rank - lo as f64;
    sorted[lo] + (sorted[hi] - sorted[lo]) * frac
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExposureMetrics {
    pub exp_luminance_mean: f64,
    pub exp_luminance_std: f64,
    pub exp_highlight_ratio: f64,
    pub exp_shadow_ratio: f64,
    pub exp_midtone_ratio: f64,
    pub exp_colorfulness: f64,
    pub exp_warmth_proxy: f64,
    pub exp_contrast: f64,
}

impl ExposureMetrics {
    pub fn to_json(&self) -> Value {
        json!({
            "exp_luminance_mean": self.exp_luminance_mean,
            "exp_luminance_std": self.exp_luminance_std,
            "exp_highlight_ratio": self.exp_highlight_ratio,
            "exp_shadow_ratio": self.exp_shadow_ratio,
            "exp_midtone_ratio": self.exp_midtone_ratio,
            "exp_colorfulness": self.exp_colorfulness,
            "exp_warmth_proxy": self.exp_warmth_proxy,
            "exp_contrast": self.exp_contrast,
        })
    }
}

fn downscale_max512(pixels: &[u8], width: usize, height: usize) -> (Vec<u8>, usize, usize) {
    let max_dim = width.max(height);
    if max_dim <= 512 || width == 0 || height == 0 {
        return (pixels.to_vec(), width, height);
    }
    let scale = 512.0 / max_dim as f64;
    let new_w = ((width as f64 * scale).round() as usize).max(32);
    let new_h = ((height as f64 * scale).round() as usize).max(32);

    let n = width * height;
    let mut r = Vec::with_capacity(n);
    let mut g = Vec::with_capacity(n);
    let mut b = Vec::with_capacity(n);
    for i in 0..n {
        r.push(pixels[i * 3]);
        g.push(pixels[i * 3 + 1]);
        b.push(pixels[i * 3 + 2]);
    }
    let r2 = resize_plane(&r, width, height, new_w, new_h, Filter::Bilinear);
    let g2 = resize_plane(&g, width, height, new_w, new_h, Filter::Bilinear);
    let b2 = resize_plane(&b, width, height, new_w, new_h, Filter::Bilinear);
    let mut out = vec![0u8; new_w * new_h * 3];
    for i in 0..new_w * new_h {
        out[i * 3] = r2[i];
        out[i * 3 + 1] = g2[i];
        out[i * 3 + 2] = b2[i];
    }
    (out, new_w, new_h)
}

/// Port of `compute_exposure_metrics`, operating on already-decoded RGB8
/// pixels rather than raw file bytes (decoding happens once at the route
/// layer and is shared with the rest of the `/v1/edit/training` pipeline).
pub fn compute_exposure_metrics(pixels: &[u8], width: usize, height: usize) -> ExposureMetrics {
    if width == 0 || height == 0 {
        return ExposureMetrics {
            exp_luminance_mean: 0.5,
            exp_luminance_std: 0.0,
            exp_highlight_ratio: 0.0,
            exp_shadow_ratio: 0.0,
            exp_midtone_ratio: 0.0,
            exp_colorfulness: 0.0,
            exp_warmth_proxy: 0.5,
            exp_contrast: 0.0,
        };
    }
    let (pixels, width, height) = downscale_max512(pixels, width, height);
    let n = width * height;

    let mut gray = Vec::with_capacity(n);
    let mut rg = Vec::with_capacity(n);
    let mut yb = Vec::with_capacity(n);
    let mut r_vals = Vec::with_capacity(n);
    let mut b_vals = Vec::with_capacity(n);
    for i in 0..n {
        let r = pixels[i * 3] as f64 / 255.0;
        let g = pixels[i * 3 + 1] as f64 / 255.0;
        let b = pixels[i * 3 + 2] as f64 / 255.0;
        gray.push(0.299 * r + 0.587 * g + 0.114 * b);
        rg.push((r - g).abs());
        yb.push((0.5 * (r + g) - b).abs());
        r_vals.push(r);
        b_vals.push(b);
    }

    let lum_mean = gray.iter().sum::<f64>() / n as f64;
    let lum_var = gray.iter().map(|v| (v - lum_mean).powi(2)).sum::<f64>() / n as f64;
    let lum_std = lum_var.sqrt();

    let highlight_ratio = gray.iter().filter(|&&v| v >= 0.92).count() as f64 / n as f64;
    let shadow_ratio = gray.iter().filter(|&&v| v <= 0.08).count() as f64 / n as f64;
    let midtone_ratio = gray.iter().filter(|&&v| v > 0.2 && v < 0.8).count() as f64 / n as f64;

    let colorfulness_raw: f64 = (0..n)
        .map(|i| (rg[i].powi(2) + yb[i].powi(2)).sqrt())
        .sum::<f64>()
        / n as f64;
    let colorfulness = clamp01(colorfulness_raw / 0.35);

    let highlight_mask: Vec<usize> = (0..n).filter(|&i| gray[i] > 0.7).collect();
    let warmth_proxy = if !highlight_mask.is_empty() {
        let r_mean =
            highlight_mask.iter().map(|&i| r_vals[i]).sum::<f64>() / highlight_mask.len() as f64;
        let b_mean =
            highlight_mask.iter().map(|&i| b_vals[i]).sum::<f64>() / highlight_mask.len() as f64;
        clamp01((r_mean - b_mean + 1.0) / 2.0)
    } else {
        0.5
    };

    let mut sorted_gray = gray.clone();
    sorted_gray.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let lum_max = percentile_linear(&sorted_gray, 97.0);
    let lum_min = percentile_linear(&sorted_gray, 3.0);
    let contrast = if (lum_max + lum_min) > 0.0 {
        clamp01((lum_max - lum_min) / (lum_max + lum_min))
    } else {
        0.0
    };

    ExposureMetrics {
        exp_luminance_mean: round4(lum_mean),
        exp_luminance_std: round4(lum_std),
        exp_highlight_ratio: round4(highlight_ratio),
        exp_shadow_ratio: round4(shadow_ratio),
        exp_midtone_ratio: round4(midtone_ratio),
        exp_colorfulness: round4(colorfulness),
        exp_warmth_proxy: round4(warmth_proxy),
        exp_contrast: round4(contrast),
    }
}

/// Port of the threshold filter inside `compute_scene_tags`: given
/// (tag_name, cosine_similarity) pairs in `SCENE_PROBES` order, keep tags
/// at or above `SCENE_THRESHOLD`.
pub fn scene_tags_from_similarities(similarities: &[(String, f64)]) -> Vec<String> {
    similarities
        .iter()
        .filter(|(_, sim)| *sim >= SCENE_THRESHOLD)
        .map(|(name, _)| name.clone())
        .collect()
}

/// Port of `get_training_stats`'s aggregation loop, given the raw
/// metadata maps for every training row (as stored — `scene_tags` as a
/// JSON-array-string, numeric exposure fields as JSON numbers).
pub fn aggregate_training_stats(metadatas: &[Map<String, Value>]) -> Value {
    let count = metadatas.len();
    if count == 0 {
        return json!({
            "count": 0,
            "has_enough_examples": false,
            "readiness": "cold_start",
            "scene_distribution": {},
            "focal_buckets": {},
            "time_of_day": {},
            "camera_distribution": {},
            "exposure": {},
        });
    }

    let mut scene_dist: Map<String, Value> = Map::new();
    let mut focal_dist: Map<String, Value> = Map::new();
    let mut tod_dist: Map<String, Value> = Map::new();
    let mut camera_dist: Map<String, Value> = Map::new();
    let mut exp_means = Vec::new();
    let mut exp_contrasts = Vec::new();
    let mut exp_colorfulness = Vec::new();

    let bump = |map: &mut Map<String, Value>, key: &str| {
        let entry = map.entry(key.to_string()).or_insert(json!(0));
        if let Value::Number(n) = entry {
            *entry = json!(n.as_i64().unwrap_or(0) + 1);
        }
    };

    for meta in metadatas {
        let tags = meta
            .get("scene_tags")
            .and_then(Value::as_str)
            .and_then(|s| serde_json::from_str::<Vec<String>>(s).ok())
            .unwrap_or_default();
        for tag in &tags {
            bump(&mut scene_dist, tag);
        }

        let fb = meta
            .get("focal_length_bucket")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        bump(&mut focal_dist, fb);

        let tod = meta
            .get("time_of_day_bucket")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        bump(&mut tod_dist, tod);

        let cam = meta
            .get("camera_model")
            .and_then(Value::as_str)
            .or_else(|| meta.get("camera_make").and_then(Value::as_str))
            .unwrap_or("unknown");
        bump(&mut camera_dist, cam);

        if let Some(v) = meta.get("exp_luminance_mean").and_then(Value::as_f64) {
            exp_means.push(v);
        }
        if let Some(v) = meta.get("exp_contrast").and_then(Value::as_f64) {
            exp_contrasts.push(v);
        }
        if let Some(v) = meta.get("exp_colorfulness").and_then(Value::as_f64) {
            exp_colorfulness.push(v);
        }
    }

    let readiness = if count == 0 {
        "cold_start"
    } else if count < 10 {
        "warming_up"
    } else if count < 50 {
        "limited"
    } else {
        "active"
    };

    let mut exposure_stats = Map::new();
    if !exp_means.is_empty() {
        exposure_stats.insert(
            "mean_luminance".into(),
            json!(
                (exp_means.iter().sum::<f64>() / exp_means.len() as f64 * 1000.0).round() / 1000.0
            ),
        );
    }
    if !exp_contrasts.is_empty() {
        exposure_stats.insert(
            "mean_contrast".into(),
            json!(
                (exp_contrasts.iter().sum::<f64>() / exp_contrasts.len() as f64 * 1000.0).round()
                    / 1000.0
            ),
        );
    }
    if !exp_colorfulness.is_empty() {
        exposure_stats.insert(
            "mean_colorfulness".into(),
            json!(
                (exp_colorfulness.iter().sum::<f64>() / exp_colorfulness.len() as f64 * 1000.0)
                    .round()
                    / 1000.0
            ),
        );
    }

    json!({
        "count": count,
        "has_enough_examples": count >= 10,
        "readiness": readiness,
        "scene_distribution": scene_dist,
        "focal_buckets": focal_dist,
        "time_of_day": tod_dist,
        "camera_distribution": camera_dist,
        "exposure": exposure_stats,
    })
}

/// Port of `list_training_examples`'s per-row projection (sorting by
/// `captured_at` descending happens at the call site once ids are known).
pub fn training_example_view(photo_id: &str, meta: &Map<String, Value>) -> Value {
    let scene_tags: Vec<String> = meta
        .get("scene_tags")
        .and_then(Value::as_str)
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();
    json!({
        "photo_id": photo_id,
        "filename": meta.get("filename").and_then(Value::as_str).unwrap_or(""),
        "label": meta.get("label").and_then(Value::as_str).unwrap_or(""),
        "summary": meta.get("summary").and_then(Value::as_str).unwrap_or(""),
        "captured_at": meta.get("captured_at").and_then(Value::as_str).unwrap_or(""),
        "has_embedding": meta.get("has_embedding").and_then(Value::as_bool).unwrap_or(false),
        "focal_length_bucket": meta.get("focal_length_bucket").and_then(Value::as_str).unwrap_or("unknown"),
        "time_of_day_bucket": meta.get("time_of_day_bucket").and_then(Value::as_str).unwrap_or("unknown"),
        "scene_tags": scene_tags,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn focal_length_buckets_match_python_thresholds() {
        assert_eq!(focal_length_bucket(None), "unknown");
        assert_eq!(focal_length_bucket(Some(14.0)), "ultra_wide");
        assert_eq!(focal_length_bucket(Some(24.0)), "wide");
        assert_eq!(focal_length_bucket(Some(50.0)), "normal");
        assert_eq!(focal_length_bucket(Some(85.0)), "short_tele");
        assert_eq!(focal_length_bucket(Some(200.0)), "tele");
        assert_eq!(focal_length_bucket(Some(400.0)), "super_tele");
    }

    #[test]
    fn time_of_day_buckets_match_python_thresholds() {
        assert_eq!(time_of_day_bucket_for_hour(None), "unknown");
        assert_eq!(time_of_day_bucket_for_hour(Some(6)), "dawn");
        assert_eq!(time_of_day_bucket_for_hour(Some(10)), "morning");
        assert_eq!(time_of_day_bucket_for_hour(Some(15)), "afternoon");
        assert_eq!(time_of_day_bucket_for_hour(Some(18)), "evening");
        assert_eq!(time_of_day_bucket_for_hour(Some(2)), "night");
        assert_eq!(time_of_day_bucket_for_hour(Some(23)), "night");
    }

    #[test]
    fn normalize_develop_settings_maps_known_keys_only() {
        let mut dev = Map::new();
        dev.insert("Exposure2012".into(), json!(0.333333));
        dev.insert("UnknownKey".into(), json!(42));
        dev.insert("Temp".into(), json!(5600));
        let canonical = normalize_develop_settings_for_style(&dev);
        assert_eq!(canonical["exposure"], json!(0.3333));
        assert_eq!(canonical["temperature"], json!(5600.0));
        assert!(canonical.get("UnknownKey").is_none());
    }

    #[test]
    fn compute_exposure_metrics_uniform_gray_image() {
        // A flat mid-gray image should have near-zero std/contrast and
        // land squarely in the midtone bucket.
        let pixels = vec![128u8; 64 * 64 * 3];
        let m = compute_exposure_metrics(&pixels, 64, 64);
        assert!((m.exp_luminance_mean - 128.0 / 255.0).abs() < 0.01);
        assert!(m.exp_luminance_std < 0.01);
        assert_eq!(m.exp_highlight_ratio, 0.0);
        assert_eq!(m.exp_shadow_ratio, 0.0);
        assert_eq!(m.exp_midtone_ratio, 1.0);
    }

    #[test]
    fn compute_exposure_metrics_bright_white_image() {
        let pixels = vec![255u8; 32 * 32 * 3];
        let m = compute_exposure_metrics(&pixels, 32, 32);
        assert_eq!(m.exp_highlight_ratio, 1.0);
        assert_eq!(m.exp_shadow_ratio, 0.0);
    }

    #[test]
    fn scene_tags_filter_respects_threshold_and_order() {
        let sims = vec![
            ("scene_portrait".to_string(), 0.5),
            ("scene_landscape".to_string(), 0.1),
            ("scene_street".to_string(), 0.22),
        ];
        assert_eq!(
            scene_tags_from_similarities(&sims),
            vec!["scene_portrait".to_string(), "scene_street".to_string()]
        );
    }

    #[test]
    fn aggregate_stats_empty_is_cold_start() {
        let stats = aggregate_training_stats(&[]);
        assert_eq!(stats["count"], json!(0));
        assert_eq!(stats["readiness"], json!("cold_start"));
    }

    #[test]
    fn aggregate_stats_counts_distributions() {
        let mut m1 = Map::new();
        m1.insert("scene_tags".into(), json!("[\"scene_portrait\"]"));
        m1.insert("focal_length_bucket".into(), json!("normal"));
        m1.insert("time_of_day_bucket".into(), json!("afternoon"));
        m1.insert("exp_luminance_mean".into(), json!(0.5));
        let mut m2 = m1.clone();
        m2.insert("exp_luminance_mean".into(), json!(0.7));
        let stats = aggregate_training_stats(&[m1, m2]);
        assert_eq!(stats["count"], json!(2));
        assert_eq!(stats["scene_distribution"]["scene_portrait"], json!(2));
        assert_eq!(stats["focal_buckets"]["normal"], json!(2));
        assert_eq!(stats["exposure"]["mean_luminance"], json!(0.6));
    }
}
