//! SigLIP2 text preprocessing: open_clip's `canonicalize` clean function
//! plus the Gemma tokenizer (`context_length` 64, `add_bos_token=False`,
//! `add_eos_token=True`, right-padded with `<pad>`=0). Ported from
//! `open_clip.tokenizer._clean_canonicalize`/`basic_clean` and the
//! `_HFTokenizerWrapper.__call__` default (non-"clips") path.
//!
//! Known gap: Python's `basic_clean` runs `ftfy.fix_text` before
//! canonicalization to repair mojibake/encoding artifacts. This port skips
//! ftfy (no equivalent crate) — a no-op for well-formed UTF-8 text, which
//! covers the overwhelming majority of search queries and keyword strings.
//! Revisit if golden text-embedding tests ever show drift traceable to
//! malformed input.

use tokenizers::Tokenizer;

pub const CONTEXT_LENGTH: usize = 64;

const PUNCTUATION: &str = "!\"#$%&'()*+,-./:;<=>?@[\\]^_`{|}~";

fn html_unescape(s: &str) -> String {
    // Matches Python's html.unescape for the handful of entities that show
    // up in real captions/keywords; full HTML5 entity tables are overkill
    // for this input class.
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
}

/// Port of `basic_clean` (minus ftfy, see module docs).
fn basic_clean(text: &str) -> String {
    html_unescape(&html_unescape(text)).trim().to_string()
}

/// Port of `canonicalize_text` / `_clean_canonicalize`.
fn canonicalize(text: &str) -> String {
    let text = text.replace('_', " ");
    let stripped: String = text.chars().filter(|c| !PUNCTUATION.contains(*c)).collect();
    let lower = stripped.to_lowercase();
    lower.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn clean_text(text: &str) -> String {
    canonicalize(&basic_clean(text))
}

#[derive(Debug, thiserror::Error)]
pub enum TokenizeError {
    #[error("failed to load tokenizer: {0}")]
    Load(String),
    #[error("tokenization failed: {0}")]
    Encode(String),
}

pub struct SiglipTokenizer {
    inner: Tokenizer,
}

impl SiglipTokenizer {
    pub fn from_file(path: &std::path::Path) -> Result<Self, TokenizeError> {
        let mut inner =
            Tokenizer::from_file(path).map_err(|e| TokenizeError::Load(e.to_string()))?;
        // Truncate to leave room for the post-processor's <eos> token,
        // matching HF's `truncation=True, max_length=64` behavior; the
        // tokenizer.json post-processor already appends <eos> (id 1) with
        // no <bos> (add_bos_token=False, add_eos_token=True upstream).
        inner
            .with_truncation(Some(tokenizers::TruncationParams {
                max_length: CONTEXT_LENGTH,
                ..Default::default()
            }))
            .map_err(|e| TokenizeError::Load(e.to_string()))?;
        inner.with_padding(Some(tokenizers::PaddingParams {
            strategy: tokenizers::PaddingStrategy::Fixed(CONTEXT_LENGTH),
            pad_id: 0,
            pad_token: "<pad>".to_string(),
            ..Default::default()
        }));
        Ok(SiglipTokenizer { inner })
    }

    /// Tokenize a batch of texts into `[batch, CONTEXT_LENGTH]` i64 ids
    /// (row-major), ready for the ONNX text tower's `input_ids` input.
    pub fn encode_batch(&self, texts: &[String]) -> Result<Vec<i64>, TokenizeError> {
        let cleaned: Vec<String> = texts.iter().map(|t| clean_text(t)).collect();
        let encodings = self
            .inner
            .encode_batch(cleaned, true)
            .map_err(|e| TokenizeError::Encode(e.to_string()))?;
        let mut out = Vec::with_capacity(texts.len() * CONTEXT_LENGTH);
        for enc in &encodings {
            out.extend(enc.get_ids().iter().map(|&id| id as i64));
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonicalize_matches_python_semantics() {
        assert_eq!(clean_text("A Photo_of a CAT!"), "a photo of a cat");
        assert_eq!(clean_text("  multiple   spaces  "), "multiple spaces");
        assert_eq!(clean_text("caf\u{e9} & bar"), "café bar");
    }

    #[test]
    fn html_entities_are_unescaped_before_canonicalization() {
        assert_eq!(clean_text("Tom &amp; Jerry"), "tom jerry");
    }
}
