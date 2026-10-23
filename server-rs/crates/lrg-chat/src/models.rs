//! Which model a chat runs on, and the others the page offers in its place.
//!
//! The plugin hands over every connection the user has set up — the API keys,
//! the AI server's address, the local engine settings — when it opens a chat,
//! so the page can start a new chat on another model without Lightroom. They
//! stay in the backend's memory for as long as the chat is open: never
//! logged, never written anywhere and never handed to the page.

use std::sync::Arc;

use serde_json::{json, Value};

use lrg_api::llm_engine::EngineOverrides;
use lrg_api::routes::llm::engine_overrides_from_json;
use lrg_api::routes::server::{available_models, ProviderConnections};
use lrg_api::state::AppState;
use lrg_providers::cloud_chat::build_chat_model;
use lrg_providers::openai_compatible::ServerUrl;
use lrg_providers::provider::ProviderSelection;

use crate::session::{CloudModel, LocalModels, ModelSource};

/// The providers the chat runs on, in the order the page lists them — this
/// computer, apps on it, the user's own server, the cloud — as the plugin's
/// model picker does (`AiProviders.lua`).
const PROVIDERS: [&str; 8] = [
    "mlx",
    "llamacpp",
    "ollama",
    "lmstudio",
    "openai_compatible",
    "chatgpt",
    "gemini",
    "anthropic",
];

fn is_local(provider: &str) -> bool {
    matches!(provider, "llamacpp" | "mlx")
}

/// What a chat was opened with: its model, and what a new chat on another
/// model takes.
#[derive(Clone)]
pub struct ChatSetup {
    pub provider: String,
    pub model: String,
    pub connections: ProviderConnections,
    pub overrides: EngineOverrides,
}

impl ChatSetup {
    /// From the body that opens a chat: `provider`, `model`, the engine
    /// settings (`llm_n_ctx`, …), and `connections` with every provider's key
    /// and address under the names `/models` takes. `api_key`, `server_url`,
    /// `ollama_base_url` and `lmstudio_base_url` beside them are for the
    /// chosen provider, and win.
    ///
    /// # Errors
    /// Names a missing model.
    pub fn from_json(body: &Value) -> Result<Self, String> {
        let text = |key: &str| {
            body.get(key)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        };
        let provider = match text("provider").unwrap_or_default().to_lowercase().as_str() {
            // The name the model listing uses, so the page finds it there.
            "openai" => "chatgpt".to_string(),
            other => other.to_string(),
        };
        let model = text("model")
            .ok_or("No AI model is chosen for the chat. Pick one in the plugin settings.")?;
        let mut connections = body
            .get("connections")
            .map(ProviderConnections::from_json)
            .unwrap_or_default();
        if let Some(key) = text("api_key") {
            connections.set_key(&provider, key);
        }
        if let Some(url) = text("server_url") {
            connections.server_url = Some(url);
        }
        if let Some(url) = text("ollama_base_url") {
            connections.ollama_base_url = Some(url);
        }
        if let Some(url) = text("lmstudio_base_url") {
            connections.lmstudio_base_url = Some(url);
        }
        Ok(ChatSetup {
            provider,
            model,
            connections,
            overrides: engine_overrides_from_json(body),
        })
    }

    /// The same connections, on another model.
    #[must_use]
    pub fn with_model(&self, provider: &str, model: &str) -> Self {
        ChatSetup {
            provider: provider.trim().to_lowercase(),
            model: model.trim().to_string(),
            ..self.clone()
        }
    }

    /// Where the chat's model comes from.
    ///
    /// # Errors
    /// Names the key, address or model that is missing.
    pub fn source(&self, state: &Arc<AppState>) -> Result<Box<dyn ModelSource>, String> {
        if self.model.is_empty() {
            return Err("No model is chosen for the chat.".into());
        }
        if is_local(&self.provider) {
            return Ok(Box::new(LocalModels {
                state: state.clone(),
                provider: self.provider.clone(),
                model: self.model.clone(),
                overrides: self.overrides,
            }));
        }
        let selection = self.connections.selection(&self.provider);
        Ok(Box::new(CloudModel {
            model: build_chat_model(&selection, &self.model)?,
            recipient: recipient(&selection),
        }))
    }
}

/// Who a cloud chat's conversation goes to, as the page names them; `None`
/// for a server on this computer.
#[must_use]
pub fn recipient(selection: &ProviderSelection) -> Option<String> {
    match selection.name.as_str() {
        "anthropic" => Some("Anthropic".into()),
        "gemini" => Some("Google".into()),
        "chatgpt" | "openai" => Some("OpenAI".into()),
        "openai_compatible" => selection
            .server_url
            .as_deref()
            .and_then(|url| ServerUrl::parse(url).ok())
            .map(|url| format!("your AI server ({})", url.label)),
        // LM Studio and Ollama: the server the plugin reaches on this
        // computer unless the user moved it, and then it is theirs.
        _ => None,
    }
}

/// The name a provider is listed under, as in the plugin's model picker.
fn label(provider: &str, server: Option<&str>) -> String {
    match provider {
        "mlx" => "On this Mac",
        "llamacpp" if cfg!(target_os = "macos") => "On this Mac",
        "llamacpp" => "On this PC",
        "ollama" => "Ollama",
        "lmstudio" => "LM Studio",
        "openai_compatible" => server.unwrap_or("Other AI server"),
        "chatgpt" => "OpenAI",
        "gemini" => "Google Gemini",
        "anthropic" => "Anthropic",
        other => other,
    }
    .to_string()
}

/// What the page's model menu offers: every model that can be reached right
/// now with the chat's connections, grouped by provider, and the one the chat
/// runs on. A provider with nothing to offer — Ollama not running, no key —
/// is left out; `warnings` says why a server the user set up, or a key they
/// entered, did not answer.
pub async fn offered(state: &AppState, setup: &ChatSetup) -> Value {
    let listing = available_models(state, &setup.connections).await;
    let server = listing
        .pointer("/servers/openai_compatible/label")
        .and_then(Value::as_str);
    let providers: Vec<Value> = PROVIDERS
        .iter()
        .filter_map(|&provider| {
            let mut models: Vec<&str> = listing["models"][provider]
                .as_array()?
                .iter()
                .filter_map(Value::as_str)
                .collect();
            if models.is_empty() {
                return None;
            }
            models.sort_by_key(|m| m.to_lowercase());
            Some(json!({
                "provider": provider,
                "label": label(provider, server),
                "sends_to": recipient(&setup.connections.selection(provider)),
                "models": models,
            }))
        })
        .collect();
    // An MLX model saved under its old name is listed under its new one.
    let model = listing["aliases"][&setup.provider][&setup.model]
        .as_str()
        .unwrap_or(&setup.model);
    json!({
        "current": {
            "provider": setup.provider,
            "model": model,
            "label": label(&setup.provider, server),
        },
        "providers": providers,
        "warnings": listing["warnings"],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_chosen_providers_key_lands_beside_the_others() {
        let setup = ChatSetup::from_json(&json!({
            "provider": "OpenAI",
            "model": " gpt-5 ",
            "api_key": "sk-chosen",
            "connections": {"anthropic_apikey": "sk-ant", "openai_apikey": "sk-old"},
            "llm_n_ctx": "8192",
        }))
        .unwrap();
        assert_eq!(setup.provider, "chatgpt");
        assert_eq!(setup.model, "gpt-5");
        assert_eq!(setup.overrides.n_ctx, Some(8192));
        let openai = setup.connections.selection("chatgpt");
        assert_eq!(openai.api_key.as_deref(), Some("sk-chosen"));
        let anthropic = setup.with_model("anthropic", "claude").connections;
        assert_eq!(
            anthropic.selection("anthropic").api_key.as_deref(),
            Some("sk-ant")
        );
        assert_eq!(anthropic.selection("ollama").api_key, None);
    }

    #[test]
    fn a_chat_needs_a_model() {
        let message = ChatSetup::from_json(&json!({"provider": "mlx"}))
            .err()
            .unwrap();
        assert!(message.contains("No AI model"), "{message}");
    }
}
