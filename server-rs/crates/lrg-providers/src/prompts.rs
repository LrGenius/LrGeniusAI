//! Port of `providers/base.py`'s `_prepare_system_prompt` /
//! `_prepare_user_prompt` — deterministic prompt string construction for
//! metadata generation.

use lrg_imaging::location::format_location_for_prompt;

use crate::types::{KeywordCategories, MetadataGenerationRequest};

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

#[cfg(test)]
mod tests {
    use super::*;

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
}
