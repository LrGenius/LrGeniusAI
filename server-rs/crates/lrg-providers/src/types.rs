//! Port of `providers/base.py`'s dataclasses: the provider-agnostic
//! request/response contract for metadata generation. Edit-recipe
//! generation (`EditGenerationRequest`/`Response`) is out of scope until
//! the `utils/edit_recipe.py` port lands.

use lrg_imaging::location::LocationTags;
use serde::Serialize;

#[derive(Debug, Clone, Default)]
pub struct MetadataGenerationRequest {
    pub image_data: Vec<u8>,
    pub uuid: String,

    pub provider: String,
    pub model: String,
    pub api_key: Option<String>,

    pub generate_keywords: bool,
    pub generate_caption: bool,
    pub generate_title: bool,
    pub generate_alt_text: bool,

    pub language: String,
    pub temperature: f64,
    pub max_tokens: Option<u32>,

    pub system_prompt: Option<String>,
    pub user_prompt: Option<String>,

    pub submit_keywords: bool,
    pub submit_folder_names: bool,

    pub existing_keywords: Option<Vec<String>>,
    pub location_data: Option<LocationTags>,
    pub folder_names: Option<String>,
    pub user_context: Option<String>,
    pub date_time: Option<String>,

    pub keyword_categories: Option<KeywordCategories>,
    pub bilingual_keywords: bool,
    pub keyword_secondary_language: Option<String>,
    pub generate_aliases: bool,
    pub catalog_keywords: Option<Vec<String>>,

    pub ollama_base_url: Option<String>,
    pub lmstudio_base_url: Option<String>,
}

/// Keyword hierarchy: either a flat list of category names, or a nested
/// tree (category -> subcategories, recursively).
#[derive(Debug, Clone)]
pub enum KeywordCategories {
    Flat(Vec<String>),
    Nested(KeywordTree),
}

/// Preserves insertion order (Python dict iteration order matters for
/// the flattened-categories prompt text and schema `required` ordering).
/// Newtype (not a type alias) because Rust forbids recursive aliases.
#[derive(Debug, Clone, Default)]
pub struct KeywordTree(pub Vec<(String, KeywordTree)>);

impl std::ops::Deref for KeywordTree {
    type Target = Vec<(String, KeywordTree)>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl FromIterator<(String, KeywordTree)> for KeywordTree {
    fn from_iter<T: IntoIterator<Item = (String, KeywordTree)>>(iter: T) -> Self {
        KeywordTree(iter.into_iter().collect())
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct MetadataGenerationResponse {
    pub uuid: String,
    pub success: bool,
    pub keywords: Option<serde_json::Value>,
    pub caption: Option<String>,
    pub title: Option<String>,
    pub alt_text: Option<String>,
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub error: Option<String>,
    pub warning: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("provider error: {0}")]
    Provider(String),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
}
