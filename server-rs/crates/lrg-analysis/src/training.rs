//! Port of `services/training.py`'s pure logic: exposure-metric
//! computation, EXIF-derived categorical buckets, develop-settings
//! canonicalization for interpolation, scene-tag thresholding, and the
//! style-profile stats aggregation. I/O (the `edit_training` LanceDB
//! table, CLIP embedding) lives in `lrg-api::routes::training`.

use std::sync::LazyLock;

use lrg_develop::model::{WbFamily, WbMode, WbSetting};
use lrg_develop::registry::{self, round_half_even, KeySpec, Level, NumFmt};
use lrg_develop::{FileKind, FileKindHint, Finite, ParseError, ParseWarning, WarningKind};
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

/// Version of the canonical form stored with each training example
/// (`canonical_settings`, `white_balance`), written as `canonical_version`.
///
/// Rows without the field are version 1: the old alias table, which read a
/// `Temp` key Lightroom never writes (so no example ever carried a white
/// balance) and blended `Tint` without its file kind. Readers re-derive
/// anything older than this from the stored `develop_settings` blob instead
/// of trusting the frozen fields; bump it whenever [`canonical_keys`] or the
/// white-balance extraction changes.
pub const CANONICAL_VERSION: u64 = 2;

/// One learnable global key: the canonical name the style engine blends
/// under, where it goes in the edit recipe, and its registry row.
#[derive(Debug, Clone, Copy)]
pub struct CanonicalKey {
    /// `"exposure"`, `"tone_curve_shadows"`: the recipe alias without the
    /// `global.` prefix, dots as underscores (the names version 1 used).
    pub name: &'static str,
    /// The recipe path below `global` (`["tone_curve", "shadows"]`).
    pub recipe_path: &'static [&'static str],
    /// The registry row (range, value kind and number format decide the
    /// blend's rounding).
    pub spec: &'static KeySpec,
}

/// Every registry key with a `global.*` recipe alias, in registry order.
///
/// The registry is the only source: white balance has no alias on purpose
/// (it is a mode plus a key family, not two numbers, see
/// [`canonicalize_develop_settings`]).
pub fn canonical_keys() -> &'static [CanonicalKey] {
    static KEYS: LazyLock<Vec<CanonicalKey>> = LazyLock::new(|| {
        registry::iter()
            .filter(|(_, spec)| spec.level == Level::Global)
            .filter_map(|(_, spec)| {
                let path = spec.recipe_alias?.strip_prefix("global.")?;
                let recipe_path: Vec<&'static str> = path.split('.').collect();
                Some(CanonicalKey {
                    name: Box::leak(path.replace('.', "_").into_boxed_str()),
                    recipe_path: Box::leak(recipe_path.into_boxed_slice()),
                    spec,
                })
            })
            .collect()
    });
    &KEYS
}

/// Looks up a canonical key by its name.
pub fn canonical_key(name: &str) -> Option<&'static CanonicalKey> {
    canonical_keys().iter().find(|k| k.name == name)
}

/// A training example's develop settings in the form the style engine
/// learns from.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CanonicalExample {
    /// Canonical name → value in the key's stored unit (see
    /// [`canonical_keys`]).
    pub settings: Map<String, Value>,
    /// The example's white balance, typed: mode, key family, numbers.
    pub white_balance: Option<WbSetting>,
    /// Raw or not, from the white-balance key family (a caller's `is_raw`
    /// hint only where the settings carry no white-balance numbers).
    pub file_kind: Option<FileKind>,
    /// Everything the reader noticed, in reading order.
    pub warnings: Vec<ParseWarning>,
}

/// Reads a `develop_settings` blob (the plugin's JSON.lua encoding of
/// `photo:getDevelopSettings()`, `[]` for none) into the canonical form.
///
/// Settings older than PV2012 (`"6.7"`) are read but not learned: they come
/// back empty, with the reader's `UnsupportedProcessVersion` warning.
pub fn canonicalize_develop_settings(
    develop_settings: &Value,
    hint: FileKindHint,
) -> Result<CanonicalExample, ParseError> {
    let (settings, warnings) = lrg_develop::lua::from_lua_value(develop_settings, hint)?;
    let file_kind = settings.file_kind;
    let learnable = !warnings
        .iter()
        .any(|w| matches!(w.kind, WarningKind::UnsupportedProcessVersion { .. }));
    if !learnable {
        return Ok(CanonicalExample {
            file_kind,
            warnings,
            ..CanonicalExample::default()
        });
    }
    let mut canonical = Map::new();
    for key in canonical_keys() {
        let Some(id) = registry::lookup(Level::Global, key.spec.name) else {
            continue;
        };
        let Some(value) = settings.get(id).and_then(|v| v.as_finite()) else {
            continue;
        };
        canonical.insert(key.name.to_string(), number_json(key.spec, value.get()));
    }
    // Only Custom is a choice of numbers. As Shot, Auto and the named presets
    // carry the Kelvin Lightroom resolved for *that* frame; keeping it would
    // invite averaging one scene's light into another.
    //
    // With both key families present the family is ambiguous: the reader
    // falls back to the caller's hint, but that hint (the plugin's own
    // guess from the same keys) is no independent evidence, so a Kelvin
    // could land on the offset scale or the other way round. Nothing is
    // learned then, which is also what the training route tells the user.
    let ambiguous_family = warnings
        .iter()
        .any(|w| matches!(w.kind, WarningKind::ConflictingFileKind));
    let white_balance = settings
        .wb()
        .filter(|_| !ambiguous_family)
        .map(|wb| match wb.mode {
            WbMode::Custom => wb,
            _ => WbSetting {
                temperature: None,
                tint: None,
                ..wb
            },
        });
    Ok(CanonicalExample {
        settings: canonical,
        white_balance,
        file_kind,
        warnings,
    })
}

/// [`canonicalize_develop_settings`] on the stored text form.
pub fn canonicalize_develop_settings_str(
    develop_settings: &str,
    hint: FileKindHint,
) -> Result<CanonicalExample, ParseError> {
    let max = lrg_develop::lua::MAX_LUA_JSON_BYTES;
    if develop_settings.len() > max {
        return Err(ParseError::TooLarge {
            size: develop_settings.len(),
            max,
        });
    }
    let value: Value = serde_json::from_str(develop_settings)?;
    canonicalize_develop_settings(&value, hint)
}

/// Port of `normalize_develop_settings_for_style`: the canonical settings
/// of a blob, or none when it cannot be read.
pub fn normalize_develop_settings_for_style(develop_settings: &Value) -> Map<String, Value> {
    canonicalize_develop_settings(develop_settings, FileKindHint::Unknown)
        .map(|c| c.settings)
        .unwrap_or_default()
}

/// A number as JSON in the precision of `spec`: an integer for integer keys,
/// otherwise rounded to the key's XMP decimals (4 when it has none).
pub fn number_json(spec: &KeySpec, x: f64) -> Value {
    if spec.kind.lua_form().is_integer() {
        if let Some(i) = Finite::new(x).ok().and_then(round_half_even) {
            return json!(i);
        }
    }
    let decimals = match spec.fmt {
        NumFmt::Fixed(d) => i32::from(d),
        _ => 4,
    };
    let factor = 10f64.powi(decimals);
    json!((x * factor).round() / factor)
}

/// A white balance as JSON: `{mode, family, temperature?, tint?}` with
/// `family` `"raw"` or `"non_raw"` and the numbers in the family's own keys'
/// precision. The same shape is stored with each training example and sent
/// as the style recipe's `white_balance`.
pub fn white_balance_json(wb: &WbSetting) -> Value {
    let mut out = Map::new();
    out.insert("mode".into(), json!(wb.mode.as_str()));
    out.insert(
        "family".into(),
        json!(match wb.family {
            WbFamily::Raw => "raw",
            WbFamily::NonRaw => "non_raw",
        }),
    );
    let spec = |name| registry::lookup(Level::Global, name).map(|id| id.spec());
    if let (Some(t), Some(spec)) = (wb.temperature, spec(wb.family.temperature_key())) {
        out.insert("temperature".into(), number_json(spec, t.get()));
    }
    if let (Some(t), Some(spec)) = (wb.tint, spec(wb.family.tint_key())) {
        out.insert("tint".into(), number_json(spec, t.get()));
    }
    Value::Object(out)
}

/// Reads [`white_balance_json`] back; `None` for anything else.
pub fn white_balance_from_json(value: &Value) -> Option<WbSetting> {
    let mode = WbMode::parse(value.get("mode")?.as_str()?)?;
    let family = match value.get("family")?.as_str()? {
        "raw" => WbFamily::Raw,
        "non_raw" => WbFamily::NonRaw,
        _ => return None,
    };
    let number = |k: &str| {
        value
            .get(k)
            .and_then(Value::as_f64)
            .and_then(|x| Finite::new(x).ok())
    };
    Some(WbSetting {
        mode,
        family,
        temperature: number("temperature"),
        tint: number("tint"),
    })
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

    fn fixture(name: &str) -> Value {
        let path = format!(
            "{}/../../testdata/develop/lua/{name}",
            env!("CARGO_MANIFEST_DIR")
        );
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap()
    }

    #[test]
    fn normalize_develop_settings_maps_registry_aliases_only() {
        let dev = json!({
            "Exposure2012": 0.333333,
            "Contrast2012": 12,
            "ParametricShadows": -8,
            "UnknownKey": 42,
            "WhiteBalance": "Custom",
            "Temperature": 5600,
            "Tint": 5,
        });
        let canonical = normalize_develop_settings_for_style(&dev);
        assert_eq!(
            canonical["exposure"],
            json!(0.33),
            "Exposure2012 has 2 decimals"
        );
        assert_eq!(canonical["contrast"], json!(12));
        assert_eq!(canonical["tone_curve_shadows"], json!(-8));
        assert!(canonical.get("UnknownKey").is_none());
        assert!(
            canonical.get("temperature").is_none() && canonical.get("tint").is_none(),
            "white balance is typed, never a canonical number: {canonical:?}"
        );
    }

    #[test]
    fn canonical_keys_come_from_the_registry_aliases() {
        let names: Vec<&str> = canonical_keys().iter().map(|k| k.name).collect();
        assert!(names.contains(&"exposure"));
        assert!(names.contains(&"tone_curve_highlights"));
        assert!(!names.iter().any(|n| n.contains("temp") || *n == "tint"));
        let tc = canonical_key("tone_curve_lights").unwrap();
        assert_eq!(tc.recipe_path, ["tone_curve", "lights"]);
        assert_eq!(tc.spec.name, "ParametricLights");
    }

    #[test]
    fn a_custom_raw_example_carries_its_white_balance() {
        let c =
            canonicalize_develop_settings(&fixture("lensblur_object.json"), FileKindHint::Unknown)
                .unwrap();
        let wb = c.white_balance.expect("Custom with Temperature/Tint");
        assert_eq!(wb.mode, WbMode::Custom);
        assert_eq!(wb.family, WbFamily::Raw);
        assert_eq!(wb.temperature.map(Finite::get), Some(5432.0));
        assert_eq!(wb.tint.map(Finite::get), Some(23.0));
        assert_eq!(c.file_kind, Some(FileKind::Raw));
        assert!(c.warnings.is_empty(), "{:?}", c.warnings);
        assert_eq!(c.settings["exposure"], json!(0.15));
    }

    #[test]
    fn an_as_shot_example_keeps_the_mode_without_numbers() {
        // As Shot's Kelvin is the camera's reading for that frame, not a
        // choice the photographer made.
        let c =
            canonicalize_develop_settings(&fixture("filterlist_filters.json"), FileKindHint::Raw)
                .unwrap();
        let wb = c.white_balance.unwrap();
        assert_eq!((wb.mode, wb.family), (WbMode::AsShot, WbFamily::Raw));
        assert_eq!((wb.temperature, wb.tint), (None, None));
    }

    #[test]
    fn incremental_keys_make_a_non_raw_example() {
        let c = canonicalize_develop_settings(
            &fixture("hand_non_raw_white_balance.json"),
            FileKindHint::Raw,
        )
        .unwrap();
        let wb = c.white_balance.unwrap();
        assert_eq!(wb.family, WbFamily::NonRaw);
        assert_eq!(wb.temperature.map(Finite::get), Some(12.0));
        assert_eq!(wb.tint.map(Finite::get), Some(-4.0));
        assert_eq!(
            c.file_kind,
            Some(FileKind::NonRaw),
            "the keys beat the hint"
        );
        let kinds: Vec<&str> = c.warnings.iter().map(|w| w.kind.name()).collect();
        assert_eq!(kinds, ["FileKindMismatch"]);
    }

    #[test]
    fn both_key_families_teach_no_white_balance_whatever_the_hint() {
        for hint in [
            FileKindHint::Raw,
            FileKindHint::NonRaw,
            FileKindHint::Unknown,
        ] {
            let c = canonicalize_develop_settings(&fixture("hand_wb_both_families.json"), hint)
                .unwrap();
            assert_eq!(c.white_balance, None, "hint {hint:?}");
            assert!(
                c.warnings
                    .iter()
                    .any(|w| matches!(w.kind, WarningKind::ConflictingFileKind)),
                "hint {hint:?}: {:?}",
                c.warnings
            );
        }
        // The file kind still follows the hint; only white balance is dropped.
        let c = canonicalize_develop_settings(
            &fixture("hand_wb_both_families.json"),
            FileKindHint::Raw,
        )
        .unwrap();
        assert_eq!(c.file_kind, Some(FileKind::Raw));
    }

    #[test]
    fn an_empty_blob_is_empty_without_an_error() {
        let c = canonicalize_develop_settings(&json!([]), FileKindHint::Raw).unwrap();
        assert!(c.settings.is_empty());
        assert_eq!(c.white_balance, None);
        assert_eq!(c.file_kind, Some(FileKind::Raw), "only the hint is left");
        assert!(c.warnings.is_empty());
        let c = canonicalize_develop_settings_str("[]", FileKindHint::Unknown).unwrap();
        assert!(c.settings.is_empty());
    }

    #[test]
    fn a_non_table_blob_is_an_error() {
        assert!(canonicalize_develop_settings(&json!([1, 2]), FileKindHint::Unknown).is_err());
        assert!(canonicalize_develop_settings_str("not json", FileKindHint::Unknown).is_err());
    }

    #[test]
    fn settings_older_than_pv2012_are_not_learned() {
        let c = canonicalize_develop_settings(&fixture("hand_pv_2010.json"), FileKindHint::Unknown)
            .unwrap();
        assert!(c.settings.is_empty());
        assert_eq!(c.white_balance, None);
        assert!(c
            .warnings
            .iter()
            .any(|w| matches!(w.kind, WarningKind::UnsupportedProcessVersion { .. })));
    }

    #[test]
    fn numbers_take_their_keys_precision() {
        let spec = |n| registry::lookup(Level::Global, n).unwrap().spec();
        assert_eq!(
            number_json(spec("Contrast2012"), 12.5),
            json!(12),
            "half to even"
        );
        assert_eq!(number_json(spec("Contrast2012"), 13.5), json!(14));
        assert_eq!(number_json(spec("Exposure2012"), 0.666666), json!(0.67));
        assert_eq!(number_json(spec("Temperature"), 5350.67), json!(5351));
    }

    #[test]
    fn white_balance_json_round_trips() {
        let wb = WbSetting {
            mode: WbMode::Custom,
            family: WbFamily::Raw,
            temperature: Some(Finite::new_const(5600.0)),
            tint: Some(Finite::new_const(-5.0)),
        };
        let j = white_balance_json(&wb);
        assert_eq!(
            j,
            json!({"mode": "Custom", "family": "raw", "temperature": 5600, "tint": -5})
        );
        assert_eq!(white_balance_from_json(&j), Some(wb));
        let auto = json!({"mode": "Auto", "family": "non_raw"});
        let wb = white_balance_from_json(&auto).unwrap();
        assert_eq!(
            (wb.mode, wb.family, wb.temperature),
            (WbMode::Auto, WbFamily::NonRaw, None)
        );
        assert_eq!(white_balance_from_json(&json!({"mode": "Custom"})), None);
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
