//! Port of `providers/base.py`'s `_prepare_system_prompt` /
//! `_prepare_user_prompt` — deterministic prompt string construction for
//! metadata generation.

use lrg_imaging::location::format_location_for_prompt;

use crate::types::{EditGenerationRequest, KeywordCategories, MetadataGenerationRequest};

pub const DEFAULT_SYSTEM_PROMPT: &str = "You are a professional photography analyst with expertise in object recognition and computer-generated image description. \nYou also try to identify famous buildings and landmarks as well as the location where the photo was taken. \nFurthermore, you aim to specify animal and plant species as accurately as possible. \nYou also describe objects—such as vehicle types and manufacturers—as specifically as you can.";

pub fn prepare_system_prompt(request: &MetadataGenerationRequest) -> String {
    request
        .system_prompt
        .clone()
        .unwrap_or_else(|| DEFAULT_SYSTEM_PROMPT.to_string())
}

/// Flatten nested keyword categories to a simple list (pre-order:
/// category, then its children), matching `_flatten_keyword_categories`.
pub fn flatten_keyword_categories(categories: &KeywordCategories) -> Vec<String> {
    match categories {
        KeywordCategories::Flat(list) => list.clone(),
        KeywordCategories::Nested(tree) => {
            let mut out = Vec::new();
            fn walk(tree: &crate::types::KeywordTree, out: &mut Vec<String>) {
                for (key, children) in tree.iter() {
                    out.push(key.clone());
                    if !children.is_empty() {
                        walk(children, out);
                    }
                }
            }
            walk(tree, &mut out);
            out
        }
    }
}

pub fn prepare_user_prompt(request: &MetadataGenerationRequest) -> String {
    let mut base_prompt = match &request.user_prompt {
        Some(p) => p.clone(),
        None => {
            let mut p = "Analyze the uploaded photo and generate the following data:\n".to_string();
            if request.generate_alt_text {
                p.push_str("* Alt text (with context for screen readers)\n");
            }
            if request.generate_caption {
                p.push_str("* Image caption\n");
            }
            if request.generate_title {
                p.push_str("* Image title\n");
            }
            if request.generate_keywords {
                p.push_str("* Keywords\n");
            }
            p
        }
    };

    base_prompt.push_str(&format!(
        "\n\nAll results should be generated in {}.",
        request.language
    ));

    let mut context_additions: Vec<String> = Vec::new();

    if let Some(location) = &request.location_data {
        if !location.is_empty() {
            if let Some(location_str) = format_location_for_prompt(location) {
                context_additions.push(format!("This photo was taken at: {location_str}"));
            }
        }
    }

    if request.submit_keywords {
        if let Some(kw) = &request.existing_keywords {
            let joined = kw
                .iter()
                .map(|k| k.trim())
                .filter(|k| !k.is_empty())
                .collect::<Vec<_>>()
                .join(", ");
            if !joined.is_empty() {
                context_additions.push(format!("Some keywords are: {joined}"));
            }
        }
    }

    if request.generate_keywords {
        if let Some(vocab) = &request.catalog_keywords {
            let joined = vocab
                .iter()
                .map(|k| k.trim())
                .filter(|k| !k.is_empty())
                .collect::<Vec<_>>()
                .join(", ");
            if !joined.is_empty() {
                context_additions.push(format!(
                    "Existing catalog vocabulary — prefer these terms over inventing new ones when semantically equivalent (you may still create new keywords for concepts not covered here): {joined}"
                ));
            }
        }
    }

    if let Some(ctx) = &request.user_context {
        if !ctx.trim().is_empty() {
            context_additions.push(format!("Context: {ctx}"));
        }
    }

    if request.submit_folder_names {
        if let Some(folders) = &request.folder_names {
            if folders.chars().any(|c| c.is_alphabetic()) {
                context_additions.push(format!("Folders: {folders}"));
            }
        }
    }

    if let Some(dt) = &request.date_time {
        if !dt.trim().is_empty() {
            context_additions.push(format!("Capture Time: {dt}"));
        }
    }

    if request.generate_keywords {
        if let Some(categories) = &request.keyword_categories {
            match categories {
                KeywordCategories::Nested(_) => {
                    let flat = flatten_keyword_categories(categories);
                    context_additions.push(format!(
                        "Please organize keywords into these categories: {}. Use the hierarchical structure to organize keywords logically.",
                        flat.join(", ")
                    ));
                }
                KeywordCategories::Flat(list) => {
                    context_additions.push(format!(
                        "Please organize keywords into these categories: {}",
                        list.join(", ")
                    ));
                }
            }
        }
    }

    if request.generate_keywords && request.bilingual_keywords {
        let secondary = request
            .keyword_secondary_language
            .as_deref()
            .unwrap_or("English")
            .trim();
        let secondary = if secondary.is_empty() {
            "English"
        } else {
            secondary
        };
        if secondary.to_lowercase() != request.language.to_lowercase() {
            context_additions.push(format!(
                "For keywords only, return each keyword as an object with fields `name` (in {}) and `synonyms` (array in {}). Include only true language equivalents; avoid duplicates and inflected-only variants.",
                request.language, secondary
            ));
        } else {
            context_additions.push(
                "For keywords, return each keyword as an object with fields `name` and `synonyms`. Use `synonyms` only for meaningful alternate terms and avoid duplicates."
                    .to_string(),
            );
        }
    }

    if request.generate_keywords && request.generate_aliases {
        let mut alias_instruction = "For each keyword, you may return an `aliases` array of at most 3 same-language linguistic synonyms of `name` — words that share the exact same core meaning and are interchangeable in any context, not just this photo (e.g. 'Kraftfahrzeug' / 'Pkw' for 'Auto'). Aliases serve as search deduplication: a user searching for either term must expect identical results. Do NOT include: related concepts, scene attributes, co-occurring elements, hypernyms, or hyponyms. Counter-example: 'Abendhimmel' is NOT a valid alias for 'Wolkenlos' — they may co-occur in this photo but describe different concepts. Omit the field entirely if no genuine linguistic synonym exists.".to_string();
        if request.bilingual_keywords {
            alias_instruction.push_str(" When `synonyms` is present, also return `synonym_aliases` with the same rules applied to each entry of `synonyms` (same secondary language as the translation).");
        }
        context_additions.push(alias_instruction);
    }

    if !context_additions.is_empty() {
        base_prompt.push_str("\n\n");
        base_prompt.push_str(&context_additions.join("\n"));
    }

    base_prompt
}

pub const DEFAULT_EDIT_SYSTEM_PROMPT: &str = "You are a senior Lightroom Classic retoucher producing high-end, client-ready edits. \
Return only a structured Lightroom edit recipe that strictly matches the provided JSON schema. \
Never output prose instructions, markdown, or fields not present in the schema. \
Prioritize natural color science, tonal separation, and believable micro-contrast unless an explicit stylized intent is given. \
Use the minimum number of controls needed for a strong result; avoid noisy over-adjustment. \
When local edits are useful, use only supported mask kinds: subject, sky, background.";

/// Port of `_prepare_edit_system_prompt`.
pub fn prepare_edit_system_prompt(request: &EditGenerationRequest) -> String {
    request
        .system_prompt
        .clone()
        .unwrap_or_else(|| DEFAULT_EDIT_SYSTEM_PROMPT.to_string())
}

const TRAINING_CANONICAL_KEYS: &[&str] = &[
    "Exposure2012",
    "Contrast2012",
    "Highlights2012",
    "Shadows2012",
    "Whites2012",
    "Blacks2012",
    "Temp",
    "Tint",
    "Texture",
    "Clarity2012",
    "Dehaze",
    "Vibrance",
    "Saturation",
    "Sharpness",
    "LuminanceSmoothing",
    "ColorNoiseReduction",
    "PostCropVignetteAmount",
    "GrainAmount",
    "SplitToningShadowHue",
    "SplitToningShadowSaturation",
    "SplitToningHighlightHue",
    "SplitToningHighlightSaturation",
    "SplitToningBalance",
    "ParametricHighlights",
    "ParametricLights",
    "ParametricDarks",
    "ParametricShadows",
];

/// Port of `_format_training_example`: serializes one few-shot training
/// example into a compact prompt-friendly string.
pub fn format_training_example(idx: usize, example: &serde_json::Value) -> String {
    let dev = example.get("develop_settings").cloned().unwrap_or_default();
    let label = example
        .get("label")
        .and_then(serde_json::Value::as_str)
        .or_else(|| example.get("filename").and_then(serde_json::Value::as_str))
        .map(str::to_string)
        .unwrap_or_else(|| format!("Example {idx}"));
    let summary = example.get("summary").and_then(serde_json::Value::as_str);

    let mut compact: Vec<(String, String)> = Vec::new();
    if let serde_json::Value::Object(dev) = &dev {
        for &key in TRAINING_CANONICAL_KEYS {
            let Some(v) = dev.get(key) else { continue };
            // Python's `round(v, 2)` + `str()` doesn't zero-pad (1.0 stays
            // "1.0"); this always shows 2 decimals. Harmless — this text
            // only feeds an LLM few-shot prompt, never parsed back.
            let formatted = match v {
                serde_json::Value::Number(n) if n.is_f64() => {
                    format!("{:.2}", n.as_f64().unwrap())
                }
                serde_json::Value::Number(n) => n.to_string(),
                _ => continue,
            };
            compact.push((key.to_string(), formatted));
        }
    }
    compact.sort_by(|a, b| a.0.cmp(&b.0));

    let mut lines = vec![format!("  [{idx}] {label}")];
    if let Some(summary) = summary {
        lines.push(format!("      Summary: {summary}"));
    }
    if compact.is_empty() {
        lines.push("      Settings: (no numeric develop settings captured)".to_string());
    } else {
        let params = compact
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join(", ");
        lines.push(format!("      Settings: {params}"));
    }
    lines.join("\n")
}

/// Port of `_prepare_edit_user_prompt`.
pub fn prepare_edit_user_prompt(request: &EditGenerationRequest) -> String {
    let mut base_prompt = match &request.user_prompt {
        Some(p) => p.clone(),
        None => "Analyze the uploaded photo and return a Lightroom edit recipe.\n\
* Add a concise summary of the intended look\n\
* Put broad corrections in `global`\n\
* Put local corrections in `masks` only when they produce clear benefit\n\
* Keep the result natural and premium unless the context explicitly asks for stylization\n\
* Do not include unchanged controls"
            .to_string(),
    };

    base_prompt.push_str(
        "\n\nEdit recipe rules:\n\
* Return only numeric Lightroom-friendly adjustments\n\
* Build edits in this order: white balance and exposure foundation -> tonal shaping -> color refinement -> detail/effects\n\
* For white balance use global `temperature` and `tint` (or `white_balance.temperature` / `white_balance.tint`)\n\
* Use global controls first; add masks only when global edits cannot solve the problem cleanly\n\
* Use masks only for subject, sky, or background\n\
* Keep saturation and clarity moderate; avoid brittle or crunchy output\n\
* Prefer highlight recovery and shadow shaping before aggressive contrast\n\
* If a curve-shaped tone response is needed (e.g. subtle S-curve, matte blacks, gentle roll-off), prefer `tone_curve.point_curve` and/or `tone_curve.extended_point_curve` over faking it with only contrast sliders\n\
* When using point curves, provide valid point pairs per channel in ascending x order and keep endpoints anchored near black/white unless a deliberate fade is requested\n\
* Use advanced controls (vignette sub-controls, sharpen detail/masking, noise detail, color NR detail/smoothness) only when clearly justified by image content\n\
* Use `lens_corrections` and `crop` only when they clearly improve the result\n\
* Add warnings when something seems uncertain or unsupported\n",
    );

    if !request.include_masks {
        base_prompt.push_str("* Do not return any masks; keep all edits global\n");
    }
    if !request.adjust_white_balance {
        base_prompt
            .push_str("* Do not adjust white balance (`temperature`, `tint`, `white_balance`)\n");
    }
    if !request.adjust_basic_tone {
        base_prompt.push_str("* Do not adjust global basic tone controls (`exposure`, `contrast`, `highlights`, `shadows`, `whites`, `blacks`)\n");
    }
    if !request.adjust_presence {
        base_prompt
            .push_str("* Do not adjust presence controls (`texture`, `clarity`, `dehaze`)\n");
    }
    if !request.adjust_color_mix {
        base_prompt
            .push_str("* Do not adjust color mix controls (`vibrance`, `saturation`, `hsl`)\n");
    }
    if !request.do_color_grading {
        base_prompt.push_str("* Do not use `color_grading`\n");
    }
    if !request.use_tone_curve {
        base_prompt.push_str("* Do not use `tone_curve` (neither parametric nor point curve)\n");
    } else if !request.use_point_curve {
        base_prompt.push_str("* Do not use `tone_curve.point_curve` or `tone_curve.extended_point_curve`; use only parametric tone curve sliders if needed\n");
    }
    if !request.adjust_detail {
        base_prompt.push_str("* Do not adjust detail controls (sharpening/noise reduction)\n");
    }
    if !request.adjust_effects {
        base_prompt.push_str("* Do not adjust effects controls (vignette/grain)\n");
    }
    if !request.adjust_lens_corrections {
        base_prompt.push_str("* Do not use `lens_corrections`\n");
    }
    if !request.allow_auto_crop {
        base_prompt.push_str("* Do not use `crop`\n");
    } else {
        match request.composition_mode.to_lowercase().as_str() {
            "none" => base_prompt.push_str("* Do not use `crop`\n"),
            "subtle" => base_prompt.push_str("* If using `crop`, keep it subtle: preserve overall framing and avoid aggressive trims\n"),
            "aggressive" => base_prompt.push_str("* Crop may be assertive when composition clearly improves; keep key subjects and avoid awkward cutoffs\n"),
            _ => {}
        }
    }

    let mut context_additions: Vec<String> = Vec::new();
    if let Some(intent) = &request.edit_intent {
        if !intent.is_empty() {
            context_additions.push(format!("Requested editing intent: {intent}"));
        }
    }

    let strength = request.style_strength.clamp(0.0, 1.0);
    if strength <= 0.25 {
        context_additions.push(
            "Style strength: very subtle (minimal slider movement, preserve original character)."
                .to_string(),
        );
    } else if strength <= 0.5 {
        context_additions.push(
            "Style strength: subtle to moderate (clean refinement, avoid strong stylization)."
                .to_string(),
        );
    } else if strength <= 0.75 {
        context_additions.push(
            "Style strength: moderate to strong (noticeable look while staying plausible)."
                .to_string(),
        );
    } else {
        context_additions.push(
            "Style strength: strong (bold look allowed, but avoid clipping and artifacts)."
                .to_string(),
        );
    }

    if let Some(ctx) = &request.user_context {
        if !ctx.is_empty() {
            context_additions.push(format!("Per-photo instructions: {ctx}"));
        }
    }
    if request.submit_keywords {
        if let Some(kw) = &request.existing_keywords {
            let joined = kw
                .iter()
                .map(|k| k.trim())
                .filter(|k| !k.is_empty())
                .collect::<Vec<_>>()
                .join(", ");
            if !joined.is_empty() {
                context_additions.push(format!("Existing keywords: {joined}"));
            }
        }
    }
    if request.submit_folder_names {
        if let Some(folders) = &request.folder_names {
            if !folders.is_empty() {
                context_additions.push(format!("Folder context: {folders}"));
            }
        }
    }
    if let Some(location) = &request.location_data {
        if !location.is_empty() {
            if let Some(location_str) = format_location_for_prompt(location) {
                context_additions.push(format!("Photo taken in: {location_str}"));
            }
        }
    }
    if let Some(dt) = &request.date_time {
        if !dt.is_empty() {
            context_additions.push(format!("Capture time: {dt}"));
        }
    }
    if !request.language.is_empty() {
        context_additions.push(format!(
            "Write `summary` and `warnings` in {}, but keep field names exactly as specified by the schema.",
            request.language
        ));
    }

    if !context_additions.is_empty() {
        base_prompt.push_str("\n\n");
        base_prompt.push_str(&context_additions.join("\n"));
    }

    if !request.training_examples.is_empty() {
        base_prompt.push_str("\n\n--- YOUR PERSONAL EDIT STYLE (few-shot examples) ---\n");
        base_prompt.push_str(
            "The following examples are from your own Lightroom edits on visually similar photos. \
Study the slider values and replicate this editing style for the current photo.\n",
        );
        for (i, example) in request.training_examples.iter().enumerate() {
            base_prompt.push_str(&format_training_example(i + 1, example));
            base_prompt.push('\n');
        }
        base_prompt.push_str("--- END OF STYLE EXAMPLES ---\n");
    }

    base_prompt
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn base_request() -> MetadataGenerationRequest {
        MetadataGenerationRequest {
            language: "English".to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn default_system_prompt_used_when_not_overridden() {
        let req = base_request();
        assert_eq!(prepare_system_prompt(&req), DEFAULT_SYSTEM_PROMPT);
    }

    #[test]
    fn custom_system_prompt_overrides_default() {
        let mut req = base_request();
        req.system_prompt = Some("custom".to_string());
        assert_eq!(prepare_system_prompt(&req), "custom");
    }

    #[test]
    fn default_user_prompt_lists_requested_fields_in_order() {
        let mut req = base_request();
        req.generate_alt_text = true;
        req.generate_caption = true;
        req.generate_title = true;
        req.generate_keywords = true;
        let prompt = prepare_user_prompt(&req);
        assert!(prompt.contains("* Alt text"));
        assert!(prompt.contains("* Image caption"));
        assert!(prompt.contains("* Image title"));
        assert!(prompt.contains("* Keywords"));
        assert!(prompt.contains("All results should be generated in English."));
        let alt_pos = prompt.find("Alt text").unwrap();
        let kw_pos = prompt.find("Keywords").unwrap();
        assert!(alt_pos < kw_pos);
    }

    #[test]
    fn location_context_uses_shared_format_location_for_prompt() {
        let mut req = base_request();
        req.location_data = Some(lrg_imaging::location::LocationTags {
            city: Some("Munich".to_string()),
            country: Some("Germany".to_string()),
            ..Default::default()
        });
        let prompt = prepare_user_prompt(&req);
        assert!(prompt.contains("This photo was taken at: Munich, Germany"));
    }

    #[test]
    fn existing_keywords_only_included_when_submit_keywords_true() {
        let mut req = base_request();
        req.existing_keywords = Some(vec!["cat".to_string(), "dog".to_string()]);
        req.submit_keywords = false;
        assert!(!prepare_user_prompt(&req).contains("Some keywords are"));
        req.submit_keywords = true;
        let prompt = prepare_user_prompt(&req);
        assert!(prompt.contains("Some keywords are: cat, dog"));
    }

    #[test]
    fn folder_names_skipped_when_purely_non_alphabetic() {
        let mut req = base_request();
        req.submit_folder_names = true;
        req.folder_names = Some("2024 / 03".to_string());
        assert!(!prepare_user_prompt(&req).contains("Folders:"));
        req.folder_names = Some("2024 Vacation".to_string());
        assert!(prepare_user_prompt(&req).contains("Folders: 2024 Vacation"));
    }

    #[test]
    fn bilingual_keywords_instruction_differs_for_same_vs_different_language() {
        let mut req = base_request();
        req.generate_keywords = true;
        req.bilingual_keywords = true;
        req.keyword_secondary_language = Some("English".to_string());
        // language == secondary -> the "same synonyms" instruction
        let prompt = prepare_user_prompt(&req);
        assert!(prompt.contains("Use `synonyms` only for meaningful alternate terms"));

        req.keyword_secondary_language = Some("German".to_string());
        let prompt2 = prepare_user_prompt(&req);
        assert!(prompt2.contains("in German"));
    }

    #[test]
    fn nested_keyword_categories_flatten_in_preorder() {
        use crate::types::KeywordTree;
        let tree: KeywordTree = KeywordTree(vec![
            (
                "People".to_string(),
                KeywordTree(vec![
                    ("Family".to_string(), KeywordTree(vec![])),
                    ("Friends".to_string(), KeywordTree(vec![])),
                ]),
            ),
            ("Places".to_string(), KeywordTree(vec![])),
        ]);
        let flat = flatten_keyword_categories(&KeywordCategories::Nested(tree));
        assert_eq!(flat, vec!["People", "Family", "Friends", "Places"]);
    }

    fn base_edit_request() -> EditGenerationRequest {
        EditGenerationRequest::new(
            Vec::new(),
            "uuid-1".to_string(),
            "openai".to_string(),
            "gpt-5".to_string(),
        )
    }

    #[test]
    fn default_edit_system_prompt_used_when_not_overridden() {
        let req = base_edit_request();
        assert_eq!(prepare_edit_system_prompt(&req), DEFAULT_EDIT_SYSTEM_PROMPT);
    }

    #[test]
    fn custom_edit_system_prompt_overrides_default() {
        let mut req = base_edit_request();
        req.system_prompt = Some("custom edit prompt".to_string());
        assert_eq!(prepare_edit_system_prompt(&req), "custom edit prompt");
    }

    #[test]
    fn disabled_controls_add_matching_negative_instructions() {
        let mut req = base_edit_request();
        req.adjust_white_balance = false;
        req.use_tone_curve = true;
        req.use_point_curve = false;
        req.allow_auto_crop = false;
        let prompt = prepare_edit_user_prompt(&req);
        assert!(prompt.contains("Do not adjust white balance"));
        assert!(prompt.contains("Do not use `tone_curve.point_curve`"));
        assert!(prompt.contains("Do not use `crop`"));
    }

    #[test]
    fn composition_mode_subtle_vs_aggressive() {
        let mut req = base_edit_request();
        req.composition_mode = "aggressive".to_string();
        let prompt = prepare_edit_user_prompt(&req);
        assert!(prompt.contains("Crop may be assertive"));

        req.composition_mode = "subtle".to_string();
        let prompt = prepare_edit_user_prompt(&req);
        assert!(prompt.contains("keep it subtle"));
    }

    #[test]
    fn style_strength_bands_produce_expected_text() {
        let mut req = base_edit_request();
        req.style_strength = 0.1;
        assert!(prepare_edit_user_prompt(&req).contains("very subtle"));
        req.style_strength = 0.9;
        assert!(prepare_edit_user_prompt(&req).contains("Style strength: strong"));
    }

    #[test]
    fn training_examples_are_injected_as_few_shot_block() {
        let mut req = base_edit_request();
        req.training_examples = vec![json!({
            "label": "Sunset portrait",
            "summary": "Warm golden-hour look",
            "develop_settings": {"Exposure2012": 0.33, "Contrast2012": 12},
        })];
        let prompt = prepare_edit_user_prompt(&req);
        assert!(prompt.contains("YOUR PERSONAL EDIT STYLE"));
        assert!(prompt.contains("[1] Sunset portrait"));
        assert!(prompt.contains("Summary: Warm golden-hour look"));
        assert!(prompt.contains("Contrast2012=12"));
        assert!(prompt.contains("END OF STYLE EXAMPLES"));
    }

    #[test]
    fn edit_prompt_includes_language_instruction() {
        let mut req = base_edit_request();
        req.language = "German".to_string();
        let prompt = prepare_edit_user_prompt(&req);
        assert!(prompt.contains("Write `summary` and `warnings` in German"));
    }
}
