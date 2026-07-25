//! LLM provider REST clients for metadata generation. Edit-recipe
//! generation (Gemini, LM Studio, Vertex AI) lands alongside the
//! `utils/edit_recipe.py` port in M8.

pub mod image_encode;
pub mod normalize;
pub mod ollama;
pub mod openai;
pub mod prompts;
pub mod schema;
pub mod schema_strict;
pub mod types;
