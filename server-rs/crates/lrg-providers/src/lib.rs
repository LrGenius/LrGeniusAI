//! LLM provider REST clients for metadata generation, plus the
//! edit-recipe contract (`edit_recipe`) shared by all edit-capable
//! providers. Gemini, LM Studio, and Vertex AI providers land in M8
//! alongside the `/v1/edit/recipe` endpoint.

pub mod edit_recipe;
pub mod gemini;
pub mod gemini_schema;
pub mod image_encode;
pub mod keyword_taxonomy;
pub mod lmstudio;
pub mod local;
pub mod local_provider;
pub mod normalize;
pub mod ollama;
pub mod openai;
pub mod prompts;
pub mod provider;
pub mod schema;
pub mod schema_strict;
pub mod text_llm;
pub mod types;
pub mod vertexai;
