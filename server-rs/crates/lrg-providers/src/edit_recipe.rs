//! Port of `utils/edit_recipe.py`: the canonical Lightroom edit-recipe
//! contract shared by every edit-capable provider — field ranges, the
//! OpenAI/Gemini JSON schemas built from those ranges, LLM-output
//! normalization/clamping, and per-control filtering for the `/edit`
//! endpoint's UI toggles.

use std::sync::OnceLock;

use serde_json::{json, Map, Value};

const GLOBAL_FIELD_RANGES: &[(&str, (f64, f64))] = &[
    ("exposure", (-5.0, 5.0)),
    ("contrast", (-100.0, 100.0)),
    ("highlights", (-100.0, 100.0)),
    ("shadows", (-100.0, 100.0)),
    ("whites", (-100.0, 100.0)),
    ("blacks", (-100.0, 100.0)),
    ("temperature", (2000.0, 50000.0)),
    ("tint", (-150.0, 150.0)),
    ("texture", (-100.0, 100.0)),
    ("clarity", (-100.0, 100.0)),
    ("dehaze", (-100.0, 100.0)),
    ("vibrance", (-100.0, 100.0)),
    ("saturation", (-100.0, 100.0)),
    ("sharpening", (0.0, 150.0)),
    ("sharpen_radius", (0.5, 3.0)),
    ("sharpen_detail", (0.0, 100.0)),
    ("sharpen_masking", (0.0, 100.0)),
    ("noise_reduction", (0.0, 100.0)),
    ("noise_reduction_detail", (0.0, 100.0)),
    ("noise_reduction_contrast", (0.0, 100.0)),
    ("color_noise_reduction", (0.0, 100.0)),
    ("color_noise_reduction_detail", (0.0, 100.0)),
    ("color_noise_reduction_smoothness", (0.0, 100.0)),
    ("vignette", (-100.0, 100.0)),
    ("vignette_midpoint", (0.0, 100.0)),
    ("vignette_roundness", (-100.0, 100.0)),
    ("vignette_feather", (0.0, 100.0)),
    ("vignette_highlights", (0.0, 100.0)),
    ("grain", (0.0, 100.0)),
    ("grain_size", (0.0, 100.0)),
    ("grain_roughness", (0.0, 100.0)),
];

const MASK_ADJUSTMENT_RANGES: &[(&str, (f64, f64))] = &[
    ("exposure", (-5.0, 5.0)),
    ("contrast", (-100.0, 100.0)),
    ("highlights", (-100.0, 100.0)),
    ("shadows", (-100.0, 100.0)),
    ("whites", (-100.0, 100.0)),
    ("blacks", (-100.0, 100.0)),
    ("temperature", (-100.0, 100.0)),
    ("tint", (-100.0, 100.0)),
    ("texture", (-100.0, 100.0)),
    ("clarity", (-100.0, 100.0)),
    ("dehaze", (-100.0, 100.0)),
    ("saturation", (-100.0, 100.0)),
    ("sharpness", (-100.0, 100.0)),
    ("noise", (-100.0, 100.0)),
    ("moire", (-100.0, 100.0)),
];

const HSL_CHANNELS: &[&str] = &[
    "red", "orange", "yellow", "green", "aqua", "blue", "purple", "magenta",
];
const COLOR_GRADING_RANGES: &[(&str, (f64, f64))] = &[
    ("hue", (0.0, 360.0)),
    ("saturation", (0.0, 100.0)),
    ("luminance", (-100.0, 100.0)),
];
const MASK_KINDS: &[&str] = &["subject", "sky", "background"];

fn number_schema(minimum: f64, maximum: f64) -> Value {
    json!({"type": "number", "minimum": minimum, "maximum": maximum})
}

fn build_hsl_schema() -> Value {
    let mut properties = Map::new();
    for &channel in HSL_CHANNELS {
        properties.insert(
            channel.to_string(),
            json!({
                "type": "object",
                "properties": {
                    "hue": number_schema(-100.0, 100.0),
                    "saturation": number_schema(-100.0, 100.0),
                    "luminance": number_schema(-100.0, 100.0),
                },
                "required": ["hue", "saturation", "luminance"],
                "additionalProperties": false,
            }),
        );
    }
    json!({
        "type": "object",
        "properties": properties,
        "required": HSL_CHANNELS,
        "additionalProperties": false,
    })
}

fn build_color_grading_schema() -> Value {
    let mut properties = Map::new();
    for region in ["shadows", "midtones", "highlights"] {
        let mut region_props = Map::new();
        let mut region_required = Vec::new();
        for &(key, (min, max)) in COLOR_GRADING_RANGES {
            region_props.insert(key.to_string(), number_schema(min, max));
            region_required.push(key);
        }
        properties.insert(
            region.to_string(),
            json!({
                "type": "object",
                "properties": region_props,
                "required": region_required,
                "additionalProperties": false,
            }),
        );
    }
    properties.insert(
        "global".to_string(),
        json!({
            "type": "object",
            "properties": {
                "hue": number_schema(0.0, 360.0),
                "saturation": number_schema(0.0, 100.0),
            },
            "required": ["hue", "saturation"],
            "additionalProperties": false,
        }),
    );
    properties.insert("blending".to_string(), number_schema(0.0, 100.0));
    properties.insert("balance".to_string(), number_schema(-100.0, 100.0));

    json!({
        "type": "object",
        "properties": properties,
        "required": ["shadows", "midtones", "highlights", "global", "blending", "balance"],
        "additionalProperties": false,
    })
}

fn build_global_schema() -> Value {
    let mut properties = Map::new();
    let mut required: Vec<String> = Vec::new();
    for &(field_name, (min, max)) in GLOBAL_FIELD_RANGES {
        properties.insert(field_name.to_string(), number_schema(min, max));
        required.push(field_name.to_string());
    }
    properties.insert("hsl".to_string(), build_hsl_schema());
    properties.insert(
        "white_balance".to_string(),
        json!({
            "type": "object",
            "properties": {
                "temperature": number_schema(2000.0, 50000.0),
                "tint": number_schema(-150.0, 150.0),
            },
            "required": ["temperature", "tint"],
            "additionalProperties": false,
        }),
    );
    properties.insert(
        "crop".to_string(),
        json!({
            "type": "object",
            "properties": {
                "left": number_schema(0.0, 1.0),
                "right": number_schema(0.0, 1.0),
                "top": number_schema(0.0, 1.0),
                "bottom": number_schema(0.0, 1.0),
                "angle": number_schema(-45.0, 45.0),
            },
            "required": ["left", "right", "top", "bottom", "angle"],
            "additionalProperties": false,
        }),
    );
    properties.insert("color_grading".to_string(), build_color_grading_schema());
    properties.insert(
        "tone_curve".to_string(),
        json!({
            "type": "object",
            "properties": {
                "highlights": number_schema(-100.0, 100.0),
                "lights": number_schema(-100.0, 100.0),
                "darks": number_schema(-100.0, 100.0),
                "shadows": number_schema(-100.0, 100.0),
                "shadow_split": number_schema(0.0, 100.0),
                "midtone_split": number_schema(0.0, 100.0),
                "highlight_split": number_schema(0.0, 100.0),
                "point_curve": {
                    "type": "object",
                    "properties": {
                        "master": {"type": "array", "items": number_schema(0.0, 255.0)},
                        "red": {"type": "array", "items": number_schema(0.0, 255.0)},
                        "green": {"type": "array", "items": number_schema(0.0, 255.0)},
                        "blue": {"type": "array", "items": number_schema(0.0, 255.0)},
                    },
                    "required": ["master", "red", "green", "blue"],
                    "additionalProperties": false,
                },
                "extended_point_curve": {
                    "type": "object",
                    "properties": {
                        "master": {"type": "array", "items": number_schema(0.0, 4096.0)},
                        "red": {"type": "array", "items": number_schema(0.0, 4096.0)},
                        "green": {"type": "array", "items": number_schema(0.0, 4096.0)},
                        "blue": {"type": "array", "items": number_schema(0.0, 4096.0)},
                    },
                    "required": ["master", "red", "green", "blue"],
                    "additionalProperties": false,
                },
            },
            "required": [
                "highlights", "lights", "darks", "shadows", "shadow_split",
                "midtone_split", "highlight_split", "point_curve", "extended_point_curve",
            ],
            "additionalProperties": false,
        }),
    );
    required.extend(
        [
            "hsl",
            "white_balance",
            "crop",
            "color_grading",
            "tone_curve",
        ]
        .map(String::from),
    );

    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false,
    })
}

fn build_mask_schema() -> Value {
    let mut adjustment_properties = Map::new();
    let mut adjustment_required = Vec::new();
    for &(field_name, (min, max)) in MASK_ADJUSTMENT_RANGES {
        adjustment_properties.insert(field_name.to_string(), number_schema(min, max));
        adjustment_required.push(field_name);
    }
    json!({
        "type": "object",
        "properties": {
            "kind": {"type": "string", "enum": MASK_KINDS},
            "name": {"type": "string"},
            "invert": {"type": "boolean"},
            "adjustments": {
                "type": "object",
                "properties": adjustment_properties,
                "required": adjustment_required,
                "additionalProperties": false,
            },
        },
        "required": ["kind", "name", "invert", "adjustments"],
        "additionalProperties": false,
    })
}

fn build_openai_edit_recipe_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "summary": {"type": "string"},
            "global": build_global_schema(),
            "masks": {"type": "array", "items": build_mask_schema()},
            "warnings": {"type": "array", "items": {"type": "string"}},
        },
        "required": ["summary", "global", "masks", "warnings"],
        "additionalProperties": false,
    })
}

/// The OpenAI-flavor structured-output schema for edit-recipe generation.
/// Built once and cached, matching the Python module's import-time constant.
pub fn openai_edit_recipe_schema() -> &'static Value {
    static SCHEMA: OnceLock<Value> = OnceLock::new();
    SCHEMA.get_or_init(build_openai_edit_recipe_schema)
}

fn convert_openai_schema_to_gemini(schema: &Value) -> Value {
    match schema.get("type").and_then(Value::as_str) {
        Some("object") => {
            let mut properties = Map::new();
            if let Some(Value::Object(props)) = schema.get("properties") {
                for (key, value) in props {
                    properties.insert(key.clone(), convert_openai_schema_to_gemini(value));
                }
            }
            let mut result = Map::new();
            result.insert("type".into(), json!("OBJECT"));
            result.insert("properties".into(), Value::Object(properties));
            if let Some(required) = schema.get("required") {
                result.insert("required".into(), required.clone());
            }
            Value::Object(result)
        }
        Some("array") => json!({
            "type": "ARRAY",
            "items": convert_openai_schema_to_gemini(&schema["items"]),
        }),
        Some("string") => {
            let mut result = Map::new();
            result.insert("type".into(), json!("STRING"));
            if let Some(e) = schema.get("enum") {
                result.insert("enum".into(), e.clone());
            }
            Value::Object(result)
        }
        Some("boolean") => json!({"type": "BOOLEAN"}),
        Some("integer") => {
            let mut result = Map::new();
            result.insert("type".into(), json!("INTEGER"));
            if let Some(m) = schema.get("minimum") {
                result.insert("minimum".into(), m.clone());
            }
            if let Some(m) = schema.get("maximum") {
                result.insert("maximum".into(), m.clone());
            }
            Value::Object(result)
        }
        Some("number") => {
            let mut result = Map::new();
            result.insert("type".into(), json!("NUMBER"));
            if let Some(m) = schema.get("minimum") {
                result.insert("minimum".into(), m.clone());
            }
            if let Some(m) = schema.get("maximum") {
                result.insert("maximum".into(), m.clone());
            }
            Value::Object(result)
        }
        // Gemini doesn't support additionalProperties; strip it from any
        // passthrough shape (there is none in this schema today, but this
        // mirrors the Python fallback branch exactly).
        _ => {
            let mut result = schema.clone();
            if let Value::Object(obj) = &mut result {
                obj.remove("additionalProperties");
            }
            result
        }
    }
}

/// The Gemini-flavor `responseSchema` for edit-recipe generation, derived
/// from [`openai_edit_recipe_schema`] by the same conversion Python uses.
pub fn gemini_edit_recipe_schema() -> &'static Value {
    static SCHEMA: OnceLock<Value> = OnceLock::new();
    SCHEMA.get_or_init(|| convert_openai_schema_to_gemini(openai_edit_recipe_schema()))
}

fn clamp_number(value: Option<&Value>, minimum: f64, maximum: f64) -> Option<f64> {
    let numeric = match value? {
        Value::Number(n) => n.as_f64()?,
        Value::String(s) => s.trim().parse::<f64>().ok()?,
        Value::Bool(b) => {
            if *b {
                1.0
            } else {
                0.0
            }
        }
        _ => return None,
    };
    // Python's round() is banker's rounding; f64::round() rounds half away
    // from zero. Immaterial here — inputs are LLM-generated decimals, never
    // exactly on a .00005 boundary in practice.
    Some((numeric.clamp(minimum, maximum) * 10000.0).round() / 10000.0)
}

fn normalize_text(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(s)) => s.trim().to_string(),
        _ => String::new(),
    }
}

fn normalize_warning_list(value: Option<&Value>) -> Vec<String> {
    let Some(Value::Array(arr)) = value else {
        return Vec::new();
    };
    arr.iter()
        .map(|item| normalize_text(Some(item)))
        .filter(|text| !text.is_empty())
        .collect()
}

fn normalize_crop_settings(crop: &Value, warnings: &mut Vec<String>) -> Map<String, Value> {
    let Value::Object(crop) = crop else {
        return Map::new();
    };
    let mut normalized = Map::new();

    let mut has_canonical_edges = false;
    for key in ["left", "right", "top", "bottom"] {
        if let Some(c) = clamp_number(crop.get(key), 0.0, 1.0) {
            normalized.insert(key.to_string(), json!(c));
            has_canonical_edges = true;
        }
    }

    // Compatibility shape frequently produced by LLMs: x/y/width/height
    // (+rotation) where x/y is top-left.
    if !has_canonical_edges {
        let x = clamp_number(crop.get("x"), 0.0, 1.0);
        let y = clamp_number(crop.get("y"), 0.0, 1.0);
        let width = clamp_number(crop.get("width"), 0.0, 1.0);
        let height = clamp_number(crop.get("height"), 0.0, 1.0);
        if let (Some(x), Some(y), Some(width), Some(height)) = (x, y, width, height) {
            normalized.insert("left".into(), json!(x));
            normalized.insert("top".into(), json!(y));
            normalized.insert(
                "right".into(),
                json!(1.0_f64.min(((x + width) * 10000.0).round() / 10000.0)),
            );
            normalized.insert(
                "bottom".into(),
                json!(1.0_f64.min(((y + height) * 10000.0).round() / 10000.0)),
            );
        }
    }

    let angle = clamp_number(crop.get("angle"), -45.0, 45.0)
        .or_else(|| clamp_number(crop.get("rotation"), -45.0, 45.0));
    if let Some(angle) = angle {
        normalized.insert("angle".into(), json!(angle));
    }

    // Require a valid rectangle before returning crop edges.
    let left = normalized.get("left").and_then(Value::as_f64);
    let right = normalized.get("right").and_then(Value::as_f64);
    let top = normalized.get("top").and_then(Value::as_f64);
    let bottom = normalized.get("bottom").and_then(Value::as_f64);

    if let (Some(l), Some(r)) = (left, right) {
        if l >= r {
            warnings.push(
                "Ignored crop: left edge was not smaller than right edge after normalization."
                    .to_string(),
            );
            normalized.remove("left");
            normalized.remove("right");
        }
    }
    if let (Some(t), Some(b)) = (top, bottom) {
        if t >= b {
            warnings.push(
                "Ignored crop: top edge was not smaller than bottom edge after normalization."
                    .to_string(),
            );
            normalized.remove("top");
            normalized.remove("bottom");
        }
    }

    // If only one side of an edge pair exists, drop it to avoid invalid
    // partial crops.
    if normalized.contains_key("left") != normalized.contains_key("right") {
        normalized.remove("left");
        normalized.remove("right");
    }
    if normalized.contains_key("top") != normalized.contains_key("bottom") {
        normalized.remove("top");
        normalized.remove("bottom");
    }

    normalized
}

fn normalize_global_settings(
    global_settings: &Value,
    warnings: &mut Vec<String>,
) -> Map<String, Value> {
    let Value::Object(gs) = global_settings else {
        return Map::new();
    };
    let mut normalized = Map::new();

    for &(field_name, (min, max)) in GLOBAL_FIELD_RANGES {
        if !gs.contains_key(field_name) {
            continue;
        }
        if let Some(c) = clamp_number(gs.get(field_name), min, max) {
            normalized.insert(field_name.to_string(), json!(c));
        }
    }

    if let Some(Value::Object(wb)) = gs.get("white_balance") {
        if !normalized.contains_key("temperature") {
            if let Some(t) = clamp_number(wb.get("temperature"), 2000.0, 50000.0) {
                normalized.insert("temperature".into(), json!(t));
            }
        }
        if !normalized.contains_key("tint") {
            if let Some(t) = clamp_number(wb.get("tint"), -150.0, 150.0) {
                normalized.insert("tint".into(), json!(t));
            }
        }
    }

    if let Some(crop @ Value::Object(_)) = gs.get("crop") {
        let normalized_crop = normalize_crop_settings(crop, warnings);
        if !normalized_crop.is_empty() {
            normalized.insert("crop".into(), Value::Object(normalized_crop));
        } else {
            warnings.push(
                "Ignored crop: no supported crop fields were returned by the model.".to_string(),
            );
        }
    }

    if let Some(Value::Object(tc)) = gs.get("tone_curve") {
        let mut normalized_curve = Map::new();
        for key in ["highlights", "lights", "darks", "shadows"] {
            if let Some(c) = clamp_number(tc.get(key), -100.0, 100.0) {
                normalized_curve.insert(key.to_string(), json!(c));
            }
        }
        for key in ["shadow_split", "midtone_split", "highlight_split"] {
            if let Some(c) = clamp_number(tc.get(key), 0.0, 100.0) {
                normalized_curve.insert(key.to_string(), json!(c));
            }
        }

        if let Some(Value::Object(pc)) = tc.get("point_curve") {
            let mut normalized_pc = Map::new();
            for channel in ["master", "red", "green", "blue"] {
                let points = normalize_point_curve_points(pc.get(channel), 0.0, 255.0);
                if !points.is_empty() {
                    normalized_pc.insert(channel.to_string(), json!(points));
                }
            }
            if !normalized_pc.is_empty() {
                normalized_curve.insert("point_curve".into(), Value::Object(normalized_pc));
            }
        }

        if let Some(Value::Object(epc)) = tc.get("extended_point_curve") {
            let mut normalized_epc = Map::new();
            for channel in ["master", "red", "green", "blue"] {
                let points = normalize_point_curve_points(epc.get(channel), 0.0, 4096.0);
                if !points.is_empty() {
                    normalized_epc.insert(channel.to_string(), json!(points));
                }
            }
            if !normalized_epc.is_empty() {
                normalized_curve
                    .insert("extended_point_curve".into(), Value::Object(normalized_epc));
            }
        }

        if !normalized_curve.is_empty() {
            normalized.insert("tone_curve".into(), Value::Object(normalized_curve));
        }
    }

    if let Some(Value::Object(hsl)) = gs.get("hsl") {
        let mut normalized_hsl = Map::new();
        for &channel in HSL_CHANNELS {
            let Some(Value::Object(channel_data)) = hsl.get(channel) else {
                continue;
            };
            let mut normalized_channel = Map::new();
            for key in ["hue", "saturation", "luminance"] {
                if let Some(c) = clamp_number(channel_data.get(key), -100.0, 100.0) {
                    normalized_channel.insert(key.to_string(), json!(c));
                }
            }
            if !normalized_channel.is_empty() {
                normalized_hsl.insert(channel.to_string(), Value::Object(normalized_channel));
            }
        }
        if !normalized_hsl.is_empty() {
            normalized.insert("hsl".into(), Value::Object(normalized_hsl));
        }
    }

    if let Some(Value::Object(cg)) = gs.get("color_grading") {
        let mut normalized_grading = Map::new();
        for region in ["shadows", "midtones", "highlights"] {
            let Some(Value::Object(region_data)) = cg.get(region) else {
                continue;
            };
            let mut normalized_region = Map::new();
            for &(key, (min, max)) in COLOR_GRADING_RANGES {
                if let Some(c) = clamp_number(region_data.get(key), min, max) {
                    normalized_region.insert(key.to_string(), json!(c));
                }
            }
            if !normalized_region.is_empty() {
                normalized_grading.insert(region.to_string(), Value::Object(normalized_region));
            }
        }

        if let Some(Value::Object(global_region)) = cg.get("global") {
            let mut normalized_global_region = Map::new();
            if let Some(c) = clamp_number(global_region.get("hue"), 0.0, 360.0) {
                normalized_global_region.insert("hue".into(), json!(c));
            }
            if let Some(c) = clamp_number(global_region.get("saturation"), 0.0, 100.0) {
                normalized_global_region.insert("saturation".into(), json!(c));
            }
            if !normalized_global_region.is_empty() {
                normalized_grading.insert("global".into(), Value::Object(normalized_global_region));
            }
        }

        if let Some(c) = clamp_number(cg.get("blending"), 0.0, 100.0) {
            normalized_grading.insert("blending".into(), json!(c));
        }
        if let Some(c) = clamp_number(cg.get("balance"), -100.0, 100.0) {
            normalized_grading.insert("balance".into(), json!(c));
        }

        if !normalized_grading.is_empty() {
            normalized.insert("color_grading".into(), Value::Object(normalized_grading));
        }
    }

    normalized
}

/// Accepts flat `[x1, y1, x2, y2, ...]` or `[{x,y}, ...]` / `[[x,y], ...]`.
fn normalize_point_curve_points(value: Option<&Value>, minimum: f64, maximum: f64) -> Vec<i64> {
    let Some(Value::Array(arr)) = value else {
        return Vec::new();
    };
    if arr.len() < 2 {
        return Vec::new();
    }

    let mut points: Vec<i64> = Vec::new();
    let is_pair_shape = matches!(arr.first(), Some(Value::Object(_)) | Some(Value::Array(_)));
    if is_pair_shape {
        for item in arr {
            let (x_val, y_val) = match item {
                Value::Object(m) => (m.get("x"), m.get("y")),
                Value::Array(a) if a.len() >= 2 => (a.first(), a.get(1)),
                _ => (None, None),
            };
            let x = clamp_number(x_val, minimum, maximum);
            let y = clamp_number(y_val, minimum, maximum);
            if let (Some(x), Some(y)) = (x, y) {
                points.push(x.round() as i64);
                points.push(y.round() as i64);
            }
        }
    } else {
        for numeric in arr {
            if let Some(c) = clamp_number(Some(numeric), minimum, maximum) {
                points.push(c.round() as i64);
            }
        }
    }

    // Need at least two points and an even number of entries.
    if points.len() % 2 == 1 {
        points.pop();
    }
    if points.len() < 4 {
        return Vec::new();
    }
    points
}

fn normalize_masks(masks: Option<&Value>, warnings: &mut Vec<String>) -> Vec<Value> {
    let Some(Value::Array(arr)) = masks else {
        return Vec::new();
    };

    let mut normalized_masks = Vec::new();
    for (index, mask) in arr.iter().enumerate() {
        let Value::Object(mask_obj) = mask else {
            warnings.push(format!("Ignored mask #{}: expected an object.", index + 1));
            continue;
        };

        let kind = normalize_text(mask_obj.get("kind")).to_lowercase();
        if !MASK_KINDS.contains(&kind.as_str()) {
            let label = if kind.is_empty() { "unknown" } else { &kind };
            warnings.push(format!(
                "Ignored mask #{}: unsupported kind '{label}'.",
                index + 1
            ));
            continue;
        }

        let Some(Value::Object(raw_adjustments)) = mask_obj.get("adjustments") else {
            warnings.push(format!("Ignored mask '{kind}': adjustments were missing."));
            continue;
        };

        let mut normalized_adjustments = Map::new();
        for &(field_name, (min, max)) in MASK_ADJUSTMENT_RANGES {
            if !raw_adjustments.contains_key(field_name) {
                continue;
            }
            if let Some(c) = clamp_number(raw_adjustments.get(field_name), min, max) {
                normalized_adjustments.insert(field_name.to_string(), json!(c));
            }
        }

        if normalized_adjustments.is_empty() {
            warnings.push(format!(
                "Ignored mask '{kind}': no supported adjustments were returned."
            ));
            continue;
        }

        let mut normalized_mask = Map::new();
        normalized_mask.insert("kind".into(), json!(kind));
        normalized_mask.insert("adjustments".into(), Value::Object(normalized_adjustments));
        let name = normalize_text(mask_obj.get("name"));
        if !name.is_empty() {
            normalized_mask.insert("name".into(), json!(name));
        }
        if let Some(Value::Bool(b)) = mask_obj.get("invert") {
            normalized_mask.insert("invert".into(), json!(*b));
        }
        normalized_masks.push(Value::Object(normalized_mask));
    }

    normalized_masks
}

/// Port of `normalize_edit_recipe`: validate + clamp an LLM-produced edit
/// recipe into the canonical shape, collecting human-readable warnings for
/// anything dropped along the way.
pub fn normalize_edit_recipe(parsed_data: &Value) -> Value {
    let mut warnings: Vec<String> = Vec::new();
    let Value::Object(data) = parsed_data else {
        return json!({
            "summary": "",
            "global": {},
            "masks": [],
            "warnings": ["LLM returned an invalid edit recipe payload."],
        });
    };

    warnings.extend(normalize_warning_list(data.get("warnings")));
    let global =
        normalize_global_settings(data.get("global").unwrap_or(&Value::Null), &mut warnings);
    let masks = normalize_masks(data.get("masks"), &mut warnings);
    let mut summary = normalize_text(data.get("summary"));
    if summary.is_empty() {
        summary = "AI-generated Lightroom edit recipe".to_string();
    }

    json!({
        "summary": summary,
        "global": global,
        "masks": masks,
        "warnings": warnings,
    })
}

fn get_bool(controls: &Map<String, Value>, key: &str, default: bool) -> bool {
    controls
        .get(key)
        .and_then(Value::as_bool)
        .unwrap_or(default)
}

fn drop_keys(map: &mut Map<String, Value>, keys: &[&str]) {
    for key in keys {
        map.remove(*key);
    }
}

/// Port of `filter_edit_recipe_by_controls`: strip global/mask fields the
/// plugin's UI toggles ("controls") say the user disabled, before sending
/// the recipe back to Lightroom.
pub fn filter_edit_recipe_by_controls(recipe: &Value, controls: &Map<String, Value>) -> Value {
    let Value::Object(recipe_obj) = recipe else {
        return recipe.clone();
    };
    let mut filtered = recipe_obj.clone();

    let mut global_settings = match filtered.get("global") {
        Some(Value::Object(g)) => g.clone(),
        _ => Map::new(),
    };

    if !get_bool(controls, "adjust_white_balance", true) {
        drop_keys(
            &mut global_settings,
            &["temperature", "tint", "white_balance"],
        );
    }
    if !get_bool(controls, "adjust_basic_tone", true) {
        drop_keys(
            &mut global_settings,
            &[
                "exposure",
                "contrast",
                "highlights",
                "shadows",
                "whites",
                "blacks",
            ],
        );
    }
    if !get_bool(controls, "adjust_presence", true) {
        drop_keys(&mut global_settings, &["texture", "clarity", "dehaze"]);
    }
    if !get_bool(controls, "adjust_color_mix", true) {
        drop_keys(&mut global_settings, &["vibrance", "saturation", "hsl"]);
    }
    if !get_bool(controls, "do_color_grading", true) {
        drop_keys(&mut global_settings, &["color_grading"]);
    }
    if !get_bool(controls, "use_tone_curve", true) {
        drop_keys(&mut global_settings, &["tone_curve"]);
    } else if !get_bool(controls, "use_point_curve", true) {
        if let Some(Value::Object(tone_curve)) = global_settings.get_mut("tone_curve") {
            tone_curve.remove("point_curve");
            tone_curve.remove("extended_point_curve");
            if tone_curve.is_empty() {
                global_settings.remove("tone_curve");
            }
        }
    }
    if !get_bool(controls, "adjust_detail", true) {
        drop_keys(
            &mut global_settings,
            &[
                "sharpening",
                "sharpen_radius",
                "sharpen_detail",
                "sharpen_masking",
                "noise_reduction",
                "noise_reduction_detail",
                "noise_reduction_contrast",
                "color_noise_reduction",
                "color_noise_reduction_detail",
                "color_noise_reduction_smoothness",
            ],
        );
    }
    if !get_bool(controls, "adjust_effects", true) {
        drop_keys(
            &mut global_settings,
            &[
                "vignette",
                "vignette_midpoint",
                "vignette_roundness",
                "vignette_feather",
                "vignette_highlights",
                "grain",
                "grain_size",
                "grain_roughness",
            ],
        );
    }
    if !get_bool(controls, "adjust_lens_corrections", true) {
        drop_keys(&mut global_settings, &["lens_corrections"]);
    }

    let composition_mode = controls
        .get("composition_mode")
        .and_then(Value::as_str)
        .unwrap_or("subtle")
        .to_lowercase();
    if composition_mode == "none" || !get_bool(controls, "allow_auto_crop", true) {
        drop_keys(&mut global_settings, &["crop"]);
    }

    filtered.insert("global".into(), Value::Object(global_settings));

    let masks = filtered.get("masks").cloned();
    if !get_bool(controls, "include_masks", true) {
        filtered.insert("masks".into(), json!([]));
    } else if let Some(Value::Array(masks)) = masks {
        let mut allowed: std::collections::HashSet<&str> =
            MASK_ADJUSTMENT_RANGES.iter().map(|&(k, _)| k).collect();
        if !get_bool(controls, "adjust_white_balance", true) {
            allowed.remove("temperature");
            allowed.remove("tint");
        }
        if !get_bool(controls, "adjust_basic_tone", true) {
            for k in [
                "exposure",
                "contrast",
                "highlights",
                "shadows",
                "whites",
                "blacks",
            ] {
                allowed.remove(k);
            }
        }
        if !get_bool(controls, "adjust_presence", true) {
            for k in ["texture", "clarity", "dehaze"] {
                allowed.remove(k);
            }
        }
        if !get_bool(controls, "adjust_color_mix", true) {
            allowed.remove("saturation");
        }
        if !get_bool(controls, "adjust_detail", true) {
            for k in ["sharpness", "noise", "moire"] {
                allowed.remove(k);
            }
        }

        let mut kept_masks = Vec::new();
        for mask in masks {
            let Value::Object(mask_obj) = &mask else {
                continue;
            };
            let Some(Value::Object(adjustments)) = mask_obj.get("adjustments") else {
                continue;
            };
            let kept_adjustments: Map<String, Value> = adjustments
                .iter()
                .filter(|(k, _)| allowed.contains(k.as_str()))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            if kept_adjustments.is_empty() {
                continue;
            }
            let mut next_mask = mask_obj.clone();
            next_mask.insert("adjustments".into(), Value::Object(kept_adjustments));
            kept_masks.push(Value::Object(next_mask));
        }
        filtered.insert("masks".into(), Value::Array(kept_masks));
    }

    Value::Object(filtered)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamps_global_values_and_discards_invalid_mask() {
        let recipe = normalize_edit_recipe(&json!({
            "summary": "  Brighten portrait  ",
            "global": {
                "exposure": 9,
                "contrast": -120,
                "temperature": "5600",
                "white_balance": {"temperature": 6200, "tint": 13},
                "crop": {"left": -0.1, "right": 1.2, "top": 0.05, "bottom": 0.95, "angle": 60},
                "vignette": -40,
                "vignette_feather": 120,
                "sharpen_radius": 4.0,
                "noise_reduction_detail": 85,
                "grain_size": 33,
                "tone_curve": {
                    "highlights": 30,
                    "shadow_split": -5,
                    "midtone_split": 50,
                    "highlight_split": 150,
                    "point_curve": {
                        "master": [0, 0, 64, 56, 128, 136, 255, 255],
                        "red": [{"x": 0, "y": 0}, {"x": 255, "y": 250}],
                        "green": [[0, 3], [255, 255]],
                        "blue": [0, 0, 255],
                    },
                    "extended_point_curve": {
                        "master": [0, 0, 84, 68, 130, 147, 183, 193, 448, 448],
                        "red": [{"x": 0, "y": 0}, {"x": 512, "y": 490}],
                    },
                },
            },
            "masks": [
                {"kind": "subject", "adjustments": {"exposure": 2.3, "clarity": -150}},
                {"kind": "person", "adjustments": {"exposure": 1}},
            ],
            "warnings": ["  keep skin natural  "],
        }));

        assert_eq!(recipe["summary"], "Brighten portrait");
        assert_eq!(recipe["global"]["exposure"], 5.0);
        assert_eq!(recipe["global"]["contrast"], -100.0);
        assert_eq!(recipe["global"]["temperature"], 5600.0);
        assert_eq!(recipe["global"]["crop"]["left"], 0.0);
        assert_eq!(recipe["global"]["crop"]["right"], 1.0);
        assert_eq!(recipe["global"]["crop"]["top"], 0.05);
        assert_eq!(recipe["global"]["crop"]["bottom"], 0.95);
        assert_eq!(recipe["global"]["crop"]["angle"], 45.0);
        assert_eq!(recipe["global"]["vignette"], -40.0);
        assert_eq!(recipe["global"]["vignette_feather"], 100.0);
        assert_eq!(recipe["global"]["sharpen_radius"], 3.0);
        assert_eq!(recipe["global"]["noise_reduction_detail"], 85.0);
        assert_eq!(recipe["global"]["grain_size"], 33.0);
        assert_eq!(recipe["global"]["tone_curve"]["highlights"], 30.0);
        assert_eq!(recipe["global"]["tone_curve"]["shadow_split"], 0.0);
        assert_eq!(recipe["global"]["tone_curve"]["midtone_split"], 50.0);
        assert_eq!(recipe["global"]["tone_curve"]["highlight_split"], 100.0);
        assert_eq!(
            recipe["global"]["tone_curve"]["point_curve"]["master"],
            json!([0, 0, 64, 56, 128, 136, 255, 255])
        );
        assert_eq!(
            recipe["global"]["tone_curve"]["point_curve"]["red"],
            json!([0, 0, 255, 250])
        );
        assert_eq!(
            recipe["global"]["tone_curve"]["point_curve"]["green"],
            json!([0, 3, 255, 255])
        );
        assert!(recipe["global"]["tone_curve"]["point_curve"]
            .get("blue")
            .is_none());
        assert_eq!(
            recipe["global"]["tone_curve"]["extended_point_curve"]["master"],
            json!([0, 0, 84, 68, 130, 147, 183, 193, 448, 448])
        );
        assert_eq!(
            recipe["global"]["tone_curve"]["extended_point_curve"]["red"],
            json!([0, 0, 512, 490])
        );
        assert!(recipe["global"].get("white_balance").is_none());
        let masks = recipe["masks"].as_array().unwrap();
        assert_eq!(masks.len(), 1);
        assert_eq!(masks[0]["kind"], "subject");
        assert_eq!(masks[0]["adjustments"]["clarity"], -100.0);
        let warnings: Vec<&str> = recipe["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .map(|w| w.as_str().unwrap())
            .collect();
        assert!(warnings.contains(&"keep skin natural"));
        assert!(warnings.iter().any(|w| w.contains("unsupported kind")));
    }

    #[test]
    fn handles_invalid_payload() {
        let recipe = normalize_edit_recipe(&json!("invalid"));
        assert_eq!(recipe["global"], json!({}));
        assert_eq!(recipe["masks"], json!([]));
        assert!(!recipe["warnings"].as_array().unwrap().is_empty());
    }

    #[test]
    fn white_balance_object_maps_to_temperature_and_tint() {
        let recipe = normalize_edit_recipe(&json!({
            "summary": "WB tweak",
            "global": {"white_balance": {"temperature": 70000, "tint": -200}},
            "masks": [],
            "warnings": [],
        }));
        assert_eq!(recipe["global"]["temperature"], 50000.0);
        assert_eq!(recipe["global"]["tint"], -150.0);
    }

    #[test]
    fn filter_recipe_by_category_controls() {
        let recipe = normalize_edit_recipe(&json!({
            "summary": "Category control test",
            "global": {
                "exposure": 0.5,
                "temperature": 5600,
                "clarity": 20,
                "vibrance": 15,
                "color_grading": {"shadows": {"hue": 220, "saturation": 20}},
                "tone_curve": {
                    "highlights": 25,
                    "point_curve": {"master": [0, 0, 128, 140, 255, 255]},
                },
                "crop": {"left": 0.02, "right": 0.98, "top": 0.03, "bottom": 0.97},
                "sharpening": 40,
                "vignette": -25,
                "lens_corrections": {"enable_profile_corrections": true},
            },
            "masks": [
                {"kind": "subject", "adjustments": {"exposure": 0.3, "clarity": 10, "sharpness": 25}},
            ],
            "warnings": [],
        }));

        let controls: Map<String, Value> = json!({
            "include_masks": true,
            "adjust_white_balance": false,
            "adjust_basic_tone": true,
            "adjust_presence": false,
            "adjust_color_mix": false,
            "do_color_grading": false,
            "use_tone_curve": true,
            "use_point_curve": false,
            "adjust_detail": false,
            "adjust_effects": false,
            "adjust_lens_corrections": false,
            "allow_auto_crop": true,
            "composition_mode": "none",
        })
        .as_object()
        .unwrap()
        .clone();

        let filtered = filter_edit_recipe_by_controls(&recipe, &controls);

        assert!(filtered["global"].get("exposure").is_some());
        assert!(filtered["global"].get("temperature").is_none());
        assert!(filtered["global"].get("clarity").is_none());
        assert!(filtered["global"].get("vibrance").is_none());
        assert!(filtered["global"].get("color_grading").is_none());
        assert!(filtered["global"].get("sharpening").is_none());
        assert!(filtered["global"].get("vignette").is_none());
        assert!(filtered["global"].get("lens_corrections").is_none());
        assert!(filtered["global"].get("crop").is_none());
        assert!(filtered["global"].get("tone_curve").is_some());
        assert!(filtered["global"]["tone_curve"]
            .get("point_curve")
            .is_none());

        let masks = filtered["masks"].as_array().unwrap();
        assert_eq!(masks.len(), 1);
        assert!(masks[0]["adjustments"].get("exposure").is_some());
        assert!(masks[0]["adjustments"].get("clarity").is_none());
        assert!(masks[0]["adjustments"].get("sharpness").is_none());
    }

    #[test]
    fn crop_xywh_shape_is_normalized() {
        let recipe = normalize_edit_recipe(&json!({
            "summary": "xywh crop",
            "global": {"crop": {"x": 0.1, "y": 0.2, "width": 0.7, "height": 0.6, "rotation": 12}},
            "masks": [],
            "warnings": [],
        }));

        assert!(recipe["global"].get("crop").is_some());
        assert_eq!(recipe["global"]["crop"]["left"], 0.1);
        assert_eq!(recipe["global"]["crop"]["top"], 0.2);
        assert_eq!(recipe["global"]["crop"]["right"], 0.8);
        assert_eq!(recipe["global"]["crop"]["bottom"], 0.8);
        assert_eq!(recipe["global"]["crop"]["angle"], 12.0);
    }

    #[test]
    fn invalid_crop_shape_adds_warning() {
        let recipe = normalize_edit_recipe(&json!({
            "summary": "bad crop",
            "global": {"crop": {"x": 0.3, "width": 0.4}},
            "masks": [],
            "warnings": [],
        }));

        assert!(recipe["global"].get("crop").is_none());
        let warnings: Vec<&str> = recipe["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .map(|w| w.as_str().unwrap())
            .collect();
        assert!(warnings.iter().any(|w| w.contains("Ignored crop")));
    }

    #[test]
    fn openai_schema_has_expected_top_level_shape() {
        let schema = openai_edit_recipe_schema();
        assert_eq!(schema["type"], "object");
        assert_eq!(schema["additionalProperties"], false);
        assert_eq!(
            schema["required"],
            json!(["summary", "global", "masks", "warnings"])
        );
        assert_eq!(
            schema["properties"]["global"]["properties"]["exposure"]["minimum"],
            -5.0
        );
    }

    #[test]
    fn gemini_schema_uppercases_types_and_drops_additional_properties() {
        let schema = gemini_edit_recipe_schema();
        assert_eq!(schema["type"], "OBJECT");
        assert!(schema.get("additionalProperties").is_none());
        assert_eq!(schema["properties"]["summary"]["type"], "STRING");
        assert_eq!(schema["properties"]["masks"]["type"], "ARRAY");
        assert_eq!(
            schema["properties"]["masks"]["items"]["properties"]["kind"]["type"],
            "STRING"
        );
        assert_eq!(
            schema["properties"]["global"]["properties"]["exposure"]["type"],
            "NUMBER"
        );
    }
}
