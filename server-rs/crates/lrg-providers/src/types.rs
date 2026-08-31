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
    /// Whether the people named on the photo may be sent to the model.
    ///
    /// Its own switch rather than a share of `submit_keywords`, because the
    /// two ask different questions. Keywords are about how much the model has
    /// to work with; names are about whether the people in a private photo are
    /// named to a cloud provider at all — and a household that wants "beach,
    /// sunset" sent may well not want "Ivo, Myriam" sent with it. Wanting the
    /// names without the keyword clutter is just as reasonable, and neither
    /// was expressible while one checkbox governed both.
    pub submit_face_tags: bool,

    pub existing_keywords: Option<Vec<String>>,
    /// Names Lightroom's face recognition put on the photo, kept apart from
    /// `existing_keywords` so the prompt can say they are people.
    ///
    /// Lightroom flattens a named face into the same keyword list as
    /// everything else, and a model reading "Ivo" between "beach" and "sunset"
    /// takes it for scenery — issue #315's "the rocky shore of Ivo Beach".
    /// Gated by `submit_face_tags`.
    pub existing_face_tags: Option<Vec<String>>,
    /// What the local BioCLIP 2 classifier made of the organism in the frame,
    /// when species identification ran for this photo.
    ///
    /// A vision LLM guesses at animals and plants from the whole scene and
    /// gets them confidently wrong — issue #326's two rabbits captioned as
    /// sheep. The classifier is a domain model over a taxonomy, so where it
    /// speaks it outranks the LLM, and the prompt says so.
    pub detected_species: Option<DetectedSpecies>,
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

/// One taxonomic identification, as the prompt needs it.
///
/// A flattened view of `lrg_ml::bioclip::TaxonomyPrediction` rather than the
/// type itself: `lrg-providers` does not depend on `lrg-ml`, and the prompt
/// only ever wants the winning candidate's names and how deep the classifier
/// was willing to go.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DetectedSpecies {
    /// `"species"` / `"genus"` / ... / `"kingdom"`. Empty or `"none"` means
    /// the classifier declined to name anything.
    pub rank: String,
    /// Kingdom-down-to-rank path, `>`-joined. Not put in the prompt itself —
    /// it is what tells the line whether to say "animal" or "plant".
    pub taxonomy: String,
    pub scientific_name: String,
    /// GBIF vernacular name; often empty above species rank.
    pub common_name: String,
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

/// Port of `providers/base.py::EditGenerationRequest`. Deliberately no
/// `Default` impl: most bool fields default to `true` in Python (unlike
/// `bool::default()`), so callers must go through [`EditGenerationRequest::new`].
#[derive(Debug, Clone)]
pub struct EditGenerationRequest {
    pub image_data: Vec<u8>,
    pub uuid: String,

    pub provider: String,
    pub model: String,
    pub api_key: Option<String>,

    pub language: String,
    pub temperature: f64,
    pub max_tokens: Option<u32>,

    pub system_prompt: Option<String>,
    pub user_prompt: Option<String>,

    pub submit_keywords: bool,
    pub submit_folder_names: bool,
    /// Whether the people named on the photo may be sent — see
    /// [`MetadataGenerationRequest::submit_face_tags`]. The edit prompt asks
    /// for names for a different reason (skin tones, who the subject is), but
    /// the user's answer to "may these names leave my machine" is the same
    /// one, so it is the same switch.
    pub submit_face_tags: bool,

    pub existing_keywords: Option<Vec<String>>,
    /// Face tags, kept apart from `existing_keywords` — see
    /// [`MetadataGenerationRequest::existing_face_tags`].
    pub existing_face_tags: Option<Vec<String>>,
    pub location_data: Option<LocationTags>,
    pub folder_names: Option<String>,
    pub user_context: Option<String>,
    pub date_time: Option<String>,
    pub edit_intent: Option<String>,
    pub style_strength: f64,
    pub include_masks: bool,
    pub adjust_white_balance: bool,
    pub adjust_basic_tone: bool,
    pub adjust_presence: bool,
    pub adjust_color_mix: bool,
    pub do_color_grading: bool,
    pub use_tone_curve: bool,
    pub use_point_curve: bool,
    pub adjust_detail: bool,
    pub adjust_effects: bool,
    pub adjust_lens_corrections: bool,
    pub allow_auto_crop: bool,
    pub composition_mode: String,
    pub ollama_base_url: Option<String>,
    pub lmstudio_base_url: Option<String>,
    pub training_examples: Vec<serde_json::Value>,
    /// Whether the *original* file is raw. `None` means the caller did not
    /// say, which is treated as raw — that is what every catalog indexed
    /// before this field existed assumed.
    ///
    /// It decides the unit of `temperature`: Lightroom exposes Kelvin for raw
    /// files and a relative -100..100 scale for JPEG/TIFF/PNG, and the two
    /// are not interchangeable. The image bytes cannot answer the question,
    /// since the plugin exports to JPEG before uploading.
    pub is_raw: Option<bool>,
}

impl EditGenerationRequest {
    /// All `adjust_*`/`use_*`/etc. controls default true, `composition_mode`
    /// defaults "subtle", `style_strength` defaults 0.5 — matching the
    /// Python dataclass field defaults exactly.
    pub fn new(image_data: Vec<u8>, uuid: String, provider: String, model: String) -> Self {
        EditGenerationRequest {
            image_data,
            uuid,
            provider,
            model,
            api_key: None,
            language: "English".to_string(),
            temperature: 0.2,
            max_tokens: None,
            system_prompt: None,
            user_prompt: None,
            submit_keywords: false,
            submit_folder_names: false,
            submit_face_tags: false,
            existing_keywords: None,
            existing_face_tags: None,
            location_data: None,
            folder_names: None,
            user_context: None,
            date_time: None,
            edit_intent: None,
            style_strength: 0.5,
            include_masks: true,
            adjust_white_balance: true,
            adjust_basic_tone: true,
            adjust_presence: true,
            adjust_color_mix: true,
            do_color_grading: true,
            use_tone_curve: true,
            use_point_curve: true,
            adjust_detail: true,
            adjust_effects: true,
            adjust_lens_corrections: true,
            allow_auto_crop: true,
            composition_mode: "subtle".to_string(),
            ollama_base_url: None,
            lmstudio_base_url: None,
            training_examples: Vec::new(),
            is_raw: None,
        }
    }
}

/// Port of `providers/base.py::EditGenerationResponse`.
#[derive(Debug, Clone, Default, Serialize)]
pub struct EditGenerationResponse {
    pub uuid: String,
    pub success: bool,
    pub recipe: Option<serde_json::Value>,
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub error: Option<String>,
    pub warning: Option<String>,
    /// Stable codes for the frame-derived limits that actually changed this
    /// recipe, in the style of culling's `reason_codes` — see
    /// `lrg_analysis::edit_guardrails::GuardrailReason`. Empty when the edit
    /// came back exactly as the provider generated it.
    ///
    /// Filled by `lrg_api::edit_budget` rather than by any provider: what a
    /// photograph can absorb is measured from its pixels, not asked of a model.
    pub guardrail_reasons: Vec<String>,
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
