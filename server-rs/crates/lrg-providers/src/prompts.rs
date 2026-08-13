//! Port of `providers/base.py`'s `_prepare_system_prompt` /
//! `_prepare_user_prompt` — deterministic prompt string construction for
//! metadata generation.
//!
//! **Ordering contract:** context is emitted run-constant-first, per-photo-last
//! (see [`SplitPrompt`]), and every provider sends the image *after* the text.
//! Together those two facts make `[system][stable]` a contiguous token prefix
//! that is byte-identical across all photos of an indexing run, which is the
//! only thing prefix-KV reuse (LM Studio, Ollama, `lrg-llama`) and cloud prompt
//! caching (OpenAI, Gemini) can act on. Moving a per-photo fact up into the
//! stable half silently truncates that prefix to nothing — add new context via
//! the right bucket, not wherever it reads best.

use lrg_imaging::location::format_location_for_prompt;

use crate::keyword_taxonomy::{CategoryLabels, KeywordLeafEncoding};
use crate::types::{EditGenerationRequest, KeywordCategories, MetadataGenerationRequest};

pub const DEFAULT_SYSTEM_PROMPT: &str = "You are a professional photography analyst with expertise in object recognition and computer-generated image description. \nYou also try to identify famous buildings and landmarks as well as the location where the photo was taken. \nFurthermore, you aim to specify animal and plant species as accurately as possible. \nYou also describe objects—such as vehicle types and manufacturers—as specifically as you can.";

pub fn prepare_system_prompt(request: &MetadataGenerationRequest) -> String {
    request
        .system_prompt
        .clone()
        .unwrap_or_else(|| DEFAULT_SYSTEM_PROMPT.to_string())
}

/// A user prompt split into the half that stays byte-identical for a whole
/// indexing run and the half that changes per photo.
///
/// Every llama.cpp-based backend (LM Studio, Ollama, and our own in-process
/// engine) can reuse a cached KV prefix, but only up to the first token that
/// differs from the previous request. Emitting run-constant context (catalog
/// vocabulary, keyword taxonomy, bilingual/alias rules) *before* per-photo
/// facts (location, capture time, …) is what makes that prefix long enough to
/// be worth anything — the reverse order caps reuse at a few dozen tokens.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SplitPrompt {
    /// Constant for the whole run; safe to pin in a KV cache.
    pub stable: String,
    /// Varies per photo; must be re-evaluated every time.
    pub per_photo: String,
}

impl SplitPrompt {
    /// The two halves as a single string, for providers that send one text
    /// block. The separator belongs to `per_photo` so that `stable` is always
    /// an exact prefix of the result.
    pub fn joined(&self) -> String {
        if self.per_photo.is_empty() {
            self.stable.clone()
        } else {
            format!("{}\n\n{}", self.stable, self.per_photo)
        }
    }
}

fn split_from_parts(base: String, stable: Vec<String>, per_photo: Vec<String>) -> SplitPrompt {
    let mut stable_text = base;
    if !stable.is_empty() {
        stable_text.push_str("\n\n");
        stable_text.push_str(&stable.join("\n"));
    }
    SplitPrompt {
        stable: stable_text,
        per_photo: per_photo.join("\n"),
    }
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
    prepare_user_prompt_split(request).joined()
}

pub fn prepare_user_prompt_split(request: &MetadataGenerationRequest) -> SplitPrompt {
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

    // Run-constant context first, per-photo context second — see `SplitPrompt`.
    let mut context_additions: Vec<String> = Vec::new();
    let mut per_photo_additions: Vec<String> = Vec::new();

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

    if request.generate_keywords {
        if let Some(categories) = &request.keyword_categories {
            let labels = CategoryLabels::from_categories(categories);
            if !labels.is_empty() {
                // These are exactly the strings the schema's `category` enum
                // permits, so the prompt and the grammar cannot drift apart.
                let joined = labels.labels().collect::<Vec<_>>().join(", ");
                context_additions.push(format!(
                    "Return keywords as a list of groups, each with a `category` and its \
                     `items`. Use exactly these category names: {joined}. Include a group \
                     ONLY for categories that genuinely apply to this photo — omit every \
                     other category entirely rather than returning it with an empty list."
                ));
            }
        }
    }

    // The layout of one keyword. Positional rather than named, because field
    // names are decoded one token at a time on every keyword of every photo
    // while this explanation is prefilled once per run — see
    // `keyword_taxonomy::KeywordLeafEncoding` for the measurement.
    if request.generate_keywords {
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
        let primary = request.language.as_str();
        // When both languages are the same, the second slot is not a
        // translation but a second way of saying it. Saying "translate to
        // German" to a German-language request produces echoes.
        let same_language = secondary.to_lowercase() == primary.to_lowercase();

        match KeywordLeafEncoding::for_request(request.bilingual_keywords, request.generate_aliases)
        {
            KeywordLeafEncoding::Plain => {}
            KeywordLeafEncoding::Aliased => context_additions.push(
                "Return each keyword as an array of strings: the keyword itself first, then any \
                 further terms for it. Example: [\"Auto\", \"Pkw\"]. Return a single-element \
                 array like [\"Auto\"] when there is no further term."
                    .to_string(),
            ),
            KeywordLeafEncoding::Translated => context_additions.push(if same_language {
                "Return each keyword as a two-element array: the keyword first, then one \
                 meaningful alternate term for it. Example: [\"Auto\", \"Wagen\"]. Avoid \
                 duplicates."
                    .to_string()
            } else {
                format!(
                    "Return each keyword as a two-element array: the keyword in {primary} first, \
                     then its {secondary} equivalent. Example: [\"Berg\", \"mountain\"]. Include \
                     only true language equivalents; avoid duplicates and inflected-only variants."
                )
            }),
            KeywordLeafEncoding::TranslatedAliased => context_additions.push(if same_language {
                "Return each keyword as two arrays: the first holds the keyword followed by any \
                 further terms for it, the second holds one meaningful alternate term followed by \
                 any further terms for that. Example: [[\"Auto\", \"Pkw\"], [\"Wagen\", \
                 \"Kraftfahrzeug\"]]. Use [[\"Auto\"], [\"Wagen\"]] when there are no further \
                 terms."
                    .to_string()
            } else {
                format!(
                    "Return each keyword as two arrays: the first holds the {primary} keyword \
                     followed by any further {primary} terms for it, the second holds its \
                     {secondary} equivalent followed by any further {secondary} terms. Example: \
                     [[\"Auto\", \"Pkw\"], [\"car\", \"automobile\"]]. Use [[\"Auto\"], \
                     [\"car\"]] when there are no further terms. Include only true language \
                     equivalents; avoid duplicates and inflected-only variants."
                )
            }),
        }
    }

    if request.generate_keywords && request.generate_aliases {
        context_additions.push(
            "At most 3 further terms per keyword, and only true linguistic synonyms — words that \
             share the exact same core meaning and are interchangeable in any context, not just \
             this photo (e.g. 'Kraftfahrzeug' / 'Pkw' for 'Auto'). They serve as search \
             deduplication: a user searching for either term must expect identical results. Do \
             NOT include: related concepts, scene attributes, co-occurring elements, hypernyms, \
             or hyponyms. Counter-example: 'Abendhimmel' is NOT a valid further term for \
             'Wolkenlos' — they may co-occur in this photo but describe different concepts. Never \
             pad by repeating a term you have already given; leave the list short instead."
                .to_string(),
        );
    }

    // --- per-photo context: everything below changes from photo to photo ---

    if let Some(location) = &request.location_data {
        if !location.is_empty() {
            if let Some(location_str) = format_location_for_prompt(location) {
                per_photo_additions.push(format!("This photo was taken at: {location_str}"));
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
                per_photo_additions.push(format!("Some keywords are: {joined}"));
            }
        }
    }

    if request.submit_folder_names {
        if let Some(folders) = &request.folder_names {
            if folders.chars().any(|c| c.is_alphabetic()) {
                per_photo_additions.push(format!("Folders: {folders}"));
            }
        }
    }

    if let Some(dt) = &request.date_time {
        if !dt.trim().is_empty() {
            per_photo_additions.push(format!("Capture Time: {dt}"));
        }
    }

    split_from_parts(base_prompt, context_additions, per_photo_additions)
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
    prepare_edit_user_prompt_split(request).joined()
}

pub fn prepare_edit_user_prompt_split(request: &EditGenerationRequest) -> SplitPrompt {
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

    // Run-constant context first, per-photo context second — see `SplitPrompt`.
    let mut context_additions: Vec<String> = Vec::new();
    let mut per_photo_additions: Vec<String> = Vec::new();
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
    if !request.language.is_empty() {
        context_additions.push(format!(
            "Write `summary` and `warnings` in {}, but keep field names exactly as specified by the schema.",
            request.language
        ));
    }

    // --- per-photo context: everything below changes from photo to photo ---

    if request.submit_keywords {
        if let Some(kw) = &request.existing_keywords {
            let joined = kw
                .iter()
                .map(|k| k.trim())
                .filter(|k| !k.is_empty())
                .collect::<Vec<_>>()
                .join(", ");
            if !joined.is_empty() {
                per_photo_additions.push(format!("Existing keywords: {joined}"));
            }
        }
    }
    if request.submit_folder_names {
        if let Some(folders) = &request.folder_names {
            if !folders.is_empty() {
                per_photo_additions.push(format!("Folder context: {folders}"));
            }
        }
    }
    if let Some(location) = &request.location_data {
        if !location.is_empty() {
            if let Some(location_str) = format_location_for_prompt(location) {
                per_photo_additions.push(format!("Photo taken in: {location_str}"));
            }
        }
    }
    if let Some(dt) = &request.date_time {
        if !dt.is_empty() {
            per_photo_additions.push(format!("Capture time: {dt}"));
        }
    }

    // Few-shot examples are retrieved per photo (nearest training neighbours),
    // so they belong to the volatile half even though they are the largest
    // block in the prompt.
    if !request.training_examples.is_empty() {
        let mut block = "\n--- YOUR PERSONAL EDIT STYLE (few-shot examples) ---\n".to_string();
        block.push_str(
            "The following examples are from your own Lightroom edits on visually similar photos. \
Study the slider values and replicate this editing style for the current photo.\n",
        );
        for (i, example) in request.training_examples.iter().enumerate() {
            block.push_str(&format_training_example(i + 1, example));
            block.push('\n');
        }
        block.push_str("--- END OF STYLE EXAMPLES ---\n");
        per_photo_additions.push(block);
    }

    split_from_parts(base_prompt, context_additions, per_photo_additions)
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
        // language == secondary: asking for a translation into the language
        // already in use just produces echoes, so the second slot is described
        // as an alternate term instead.
        let prompt = prepare_user_prompt(&req);
        assert!(prompt.contains("then one meaningful alternate term"));
        assert!(!prompt.contains("equivalent"));

        req.keyword_secondary_language = Some("German".to_string());
        let prompt2 = prepare_user_prompt(&req);
        assert!(prompt2.contains("then its German equivalent"));
    }

    #[test]
    fn every_keyword_layout_is_explained_with_an_example() {
        // The positional layout is only readable if the prompt says what each
        // slot means — the schema alone conveys none of it.
        for (bilingual, aliases) in [(false, true), (true, false), (true, true)] {
            let mut req = base_request();
            req.generate_keywords = true;
            req.bilingual_keywords = bilingual;
            req.generate_aliases = aliases;
            let prompt = prepare_user_prompt(&req);
            assert!(
                prompt.contains("Example: ["),
                "bilingual={bilingual} aliases={aliases} needs a worked example"
            );
        }
    }

    #[test]
    fn the_keyword_layout_stays_in_the_run_constant_half() {
        // It is identical for every photo of a run; in `per_photo` it would
        // cut the reusable prefix at the first keyword instruction.
        let mut req = base_request();
        req.generate_keywords = true;
        req.bilingual_keywords = true;
        req.generate_aliases = true;
        let split = prepare_user_prompt_split(&req);
        assert!(split.stable.contains("Return each keyword as two arrays"));
        assert!(!split.per_photo.contains("Return each keyword"));
    }

    /// The whole point of the split: run-constant context must sit in
    /// `stable`, per-photo facts in `per_photo`, so `stable` is a reusable
    /// KV-cache prefix across every photo of an indexing run.
    #[test]
    fn split_puts_run_constant_context_before_per_photo_context() {
        let mut req = base_request();
        req.generate_keywords = true;
        req.catalog_keywords = Some(vec!["Landschaft".to_string(), "Portrait".to_string()]);
        req.keyword_categories = Some(KeywordCategories::Flat(vec!["People".to_string()]));
        req.user_context = Some("client shoot".to_string());
        req.submit_keywords = true;
        req.existing_keywords = Some(vec!["cat".to_string()]);
        req.submit_folder_names = true;
        req.folder_names = Some("Iceland Trip".to_string());
        req.date_time = Some("2026-08-07 10:00".to_string());
        req.location_data = Some(lrg_imaging::location::LocationTags {
            city: Some("Reykjavik".to_string()),
            ..Default::default()
        });

        let split = prepare_user_prompt_split(&req);

        for run_constant in [
            "Existing catalog vocabulary",
            "Landschaft, Portrait",
            "Use exactly these category names",
            "Context: client shoot",
        ] {
            assert!(
                split.stable.contains(run_constant),
                "expected {run_constant:?} in the stable half"
            );
            assert!(!split.per_photo.contains(run_constant));
        }

        for per_photo in [
            "This photo was taken at: Reykjavik",
            "Some keywords are: cat",
            "Folders: Iceland Trip",
            "Capture Time: 2026-08-07 10:00",
        ] {
            assert!(
                split.per_photo.contains(per_photo),
                "expected {per_photo:?} in the per-photo half"
            );
            assert!(!split.stable.contains(per_photo));
        }
    }

    #[test]
    fn stable_half_is_a_prefix_of_the_joined_prompt() {
        let mut req = base_request();
        req.generate_keywords = true;
        req.catalog_keywords = Some(vec!["Landschaft".to_string()]);
        req.date_time = Some("2026-08-07".to_string());

        let split = prepare_user_prompt_split(&req);
        let joined = prepare_user_prompt(&req);
        assert!(joined.starts_with(&split.stable));
        assert!(joined.ends_with(&split.per_photo));
        assert!(!split.per_photo.is_empty());
    }

    #[test]
    fn joined_equals_stable_when_no_per_photo_context() {
        let req = base_request();
        let split = prepare_user_prompt_split(&req);
        assert!(split.per_photo.is_empty());
        assert_eq!(split.joined(), split.stable);
    }

    /// The schema lets a model omit categories that do not apply; without
    /// this instruction it will keep emitting every one of them with an empty
    /// list, which is exactly the output-token waste the flat group shape
    /// exists to remove.
    #[test]
    fn category_prompt_permits_omitting_categories_and_lists_the_enum_labels() {
        use crate::types::KeywordTree;
        let mut req = base_request();
        req.generate_keywords = true;
        // "Landschaft" is ambiguous, so the prompt must offer the full paths —
        // the same strings the schema's `category` enum permits.
        req.keyword_categories = Some(KeywordCategories::Nested(KeywordTree(vec![
            (
                "Natur".to_string(),
                KeywordTree(vec![("Landschaft".to_string(), KeywordTree(vec![]))]),
            ),
            (
                "Reise".to_string(),
                KeywordTree(vec![("Landschaft".to_string(), KeywordTree(vec![]))]),
            ),
        ])));

        let prompt = prepare_user_prompt(&req);
        assert!(prompt.contains("Natur/Landschaft"));
        assert!(prompt.contains("Reise/Landschaft"));
        assert!(
            prompt.contains("omit every other category entirely"),
            "the prompt must tell the model that omission is expected"
        );
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
    fn edit_split_keeps_training_examples_in_the_per_photo_half() {
        let mut req = base_edit_request();
        req.edit_intent = Some("moody".to_string());
        req.language = "German".to_string();
        req.submit_folder_names = true;
        req.folder_names = Some("Wedding".to_string());
        req.training_examples = vec![json!({
            "label": "Sunset portrait",
            "develop_settings": {"Exposure2012": 0.33},
        })];

        let split = prepare_edit_user_prompt_split(&req);
        assert!(split.stable.contains("Requested editing intent: moody"));
        assert!(split
            .stable
            .contains("Write `summary` and `warnings` in German"));
        assert!(split.per_photo.contains("Folder context: Wedding"));
        assert!(split.per_photo.contains("YOUR PERSONAL EDIT STYLE"));
        assert!(!split.stable.contains("YOUR PERSONAL EDIT STYLE"));
        assert!(prepare_edit_user_prompt(&req).starts_with(&split.stable));
    }

    #[test]
    fn edit_prompt_includes_language_instruction() {
        let mut req = base_edit_request();
        req.language = "German".to_string();
        let prompt = prepare_edit_user_prompt(&req);
        assert!(prompt.contains("Write `summary` and `warnings` in German"));
    }
}
