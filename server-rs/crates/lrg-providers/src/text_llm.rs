//! Port of `services/keywords.py`'s `_call_llm_text` +
//! `validate_clusters_with_llm`: a plain-text (non-structured-output)
//! chat call across all four providers, used to LLM-validate CLIP
//! keyword-similarity clusters before suggesting merges to the user.
//! Deliberately separate from the metadata/edit-recipe request types —
//! this is a much smaller, schema-free call shape.

use serde_json::Value;

use crate::provider::{build_provider, ProviderSelection};

/// Which provider/model/creds to use for a plain-text LLM call.
pub struct TextLlmConfig {
    pub provider: String,
    pub model: Option<String>,
    pub api_key: Option<String>,
    pub ollama_base_url: Option<String>,
    pub lmstudio_base_url: Option<String>,
    /// Present only when an in-process model is loaded; see
    /// [`crate::provider::ProviderSelection::local_engine`].
    pub local_engine: Option<crate::local::SharedLocalEngine>,
}

/// Port of `_call_llm_text`. Returns `None` on any failure (network,
/// missing config, unexpected response shape) — callers fall back to the
/// raw CLIP candidates, matching Python's try/except-and-continue.
pub async fn call_llm_text(
    config: &TextLlmConfig,
    system_prompt: &str,
    user_prompt: &str,
) -> Option<String> {
    let provider = build_provider(&ProviderSelection {
        name: config.provider.clone(),
        api_key: config.api_key.clone(),
        ollama_base_url: config.ollama_base_url.clone(),
        lmstudio_base_url: config.lmstudio_base_url.clone(),
        local_engine: config.local_engine.clone(),
    })
    .ok()?;
    provider
        .generate_text(config.model.as_deref(), system_prompt, user_prompt)
        .await
}

const VALIDATION_SYSTEM: &str = "You are a photo library metadata expert. Decide which groups of similar-sounding keywords from a photography catalog are true synonyms that should be merged into one keyword.";
const MAX_MEMBERS: usize = 15;

fn build_validation_prompt(chunk: &[Vec<String>]) -> String {
    let groups_text: String = chunk
        .iter()
        .enumerate()
        .map(|(i, group)| {
            let truncated: Vec<&String> = group.iter().take(MAX_MEMBERS).collect();
            format!("{}. {}", i + 1, serde_json::to_string(&truncated).unwrap())
        })
        .collect::<Vec<_>>()
        .join("\n");

    format!(
        "Below are groups of keywords that a similarity model found to be related. \
For each group, decide if the members describe the exact same concept \
(true synonyms \u{2192} merge them) or distinct concepts (keep separate).\n\n\
Rules:\n\
- Merge only true synonyms (e.g. 'Car' and 'Automobile').\n\
- Do NOT merge related-but-different concepts (e.g. 'Cat' and 'Kitten' are different life stages).\n\
- You may split a group: only include the members that are genuine synonyms.\n\
- Put the clearest, most common name first \u{2014} that becomes the canonical keyword.\n\
- If no members of a group should be merged, return an empty list [].\n\n\
Groups:\n{groups_text}\n\n\
Return a JSON array with exactly one element per group, in the same order. \
Each element:\n\
  - Merge: [\"BestName\", \"synonym1\", ...] \u{2014} canonical name first, at least 2 items\n\
  - No merge: []\n\n\
Return only the JSON array, no other text."
    )
}

/// Port of the JSON-extraction half of `validate_clusters_with_llm`:
/// strip markdown fences, find the first `[`, parse the first complete
/// JSON value from that point (ignoring any trailing prose), align
/// length with `expected_len`, and keep only groups with >=2 clean
/// members. Returns `None` if no JSON array could be recovered at all.
fn parse_validation_response(raw: &str, expected_len: usize) -> Option<Vec<Vec<String>>> {
    let mut text = raw.trim();
    if let Some(rest) = text.strip_prefix("```") {
        text = rest.split_once('\n').map(|(_, rest)| rest).unwrap_or(rest);
    }
    if let Some(idx) = text.rfind("```") {
        text = text[..idx].trim();
    }

    let bracket = text.find('[')?;
    let mut deserializer = serde_json::Deserializer::from_str(&text[bracket..]);
    let parsed: Value = serde::de::Deserialize::deserialize(&mut deserializer).ok()?;
    let Value::Array(items) = parsed else {
        return None;
    };

    let mut items = items;
    items.resize(expected_len, Value::Array(Vec::new()));

    let mut validated = Vec::new();
    for item in items {
        let Value::Array(members) = item else {
            continue;
        };
        let clean: Vec<String> = members
            .iter()
            .filter_map(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect();
        if clean.len() >= 2 {
            validated.push(clean);
        }
    }
    Some(validated)
}

/// Port of `validate_clusters_with_llm`: chunk candidate clusters, ask
/// the LLM to confirm/split each group, and fall back to the raw CLIP
/// candidates (any group with >=2 members) for any chunk whose LLM call
/// fails or returns something unparseable.
pub async fn validate_clusters_with_llm(
    candidate_clusters: &[Vec<String>],
    config: &TextLlmConfig,
) -> Vec<Vec<String>> {
    const CHUNK_SIZE: usize = 15;
    if candidate_clusters.is_empty() {
        return Vec::new();
    }

    let mut validated = Vec::new();
    for chunk in candidate_clusters.chunks(CHUNK_SIZE) {
        let user_prompt = build_validation_prompt(chunk);
        let raw = call_llm_text(config, VALIDATION_SYSTEM, &user_prompt).await;

        let fallback = || {
            chunk
                .iter()
                .filter(|g| g.len() >= 2)
                .cloned()
                .collect::<Vec<_>>()
        };
        match raw {
            None => validated.extend(fallback()),
            Some(text) => match parse_validation_response(&text, chunk.len()) {
                Some(groups) => validated.extend(groups),
                None => validated.extend(fallback()),
            },
        }
    }
    validated
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_validation_response_extracts_json_array_ignoring_prose() {
        let raw =
            "Sure, here you go:\n```json\n[[\"Car\", \"Automobile\"], []]\n```\nHope that helps!";
        let parsed = parse_validation_response(raw, 2).unwrap();
        assert_eq!(
            parsed,
            vec![vec!["Car".to_string(), "Automobile".to_string()]]
        );
    }

    #[test]
    fn parse_validation_response_pads_short_responses() {
        let raw = "[[\"A\", \"B\"]]";
        let parsed = parse_validation_response(raw, 3).unwrap();
        assert_eq!(parsed.len(), 1);
    }

    #[test]
    fn parse_validation_response_drops_single_member_groups() {
        let raw = "[[\"OnlyOne\"], [\"A\", \"B\"]]";
        let parsed = parse_validation_response(raw, 2).unwrap();
        assert_eq!(parsed, vec![vec!["A".to_string(), "B".to_string()]]);
    }

    #[test]
    fn parse_validation_response_none_when_no_bracket() {
        assert!(parse_validation_response("not json at all", 1).is_none());
    }

    #[test]
    fn build_validation_prompt_numbers_groups_and_truncates_members() {
        let big_group: Vec<String> = (0..20).map(|i| format!("kw{i}")).collect();
        let prompt = build_validation_prompt(&[vec!["A".to_string(), "B".to_string()], big_group]);
        assert!(prompt.contains("1. [\"A\",\"B\"]"));
        assert!(prompt.contains("kw14"));
        assert!(!prompt.contains("kw15"));
    }
}
