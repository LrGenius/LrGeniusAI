//! The chat's own connection to the MCP tools, in memory.
//!
//! The chat does not call the tools' code directly: it is an MCP client of
//! the same `LrGeniusMcp` server that `/mcp` serves, over an in-memory pipe.
//! So a chat call takes exactly the path an external agent's call takes —
//! the same result sets, warnings, undo steps, and the same refusals from
//! the plugin's read-only switch and from "agent control stopped".

use std::sync::Arc;

use base64::Engine as _;
use rmcp::model::{CallToolRequestParams, CallToolResult, ClientConfig};
use rmcp::service::RunningService;
use rmcp::{ClientHandler, RoleClient, ServiceExt};
use serde_json::{json, Map, Value};

use lrg_mcp::{LrGeniusMcp, Shared};
use lrg_providers::chat::{ChatImage, ToolCall, ToolResult, ToolSpec};

use crate::permissions::ToolClass;

/// Big enough that a tool answer with a photo in it is not split into many
/// small writes.
const PIPE_BYTES: usize = 1 << 20;

#[derive(Clone, Default)]
struct ChatClient;

impl ClientHandler for ChatClient {
    fn get_info(&self) -> ClientConfig {
        ClientConfig::default()
    }
}

/// A tool as the chat offers it: what the model is told, plus what the
/// permissions and the page need.
#[derive(Debug, Clone)]
pub struct OfferedTool {
    pub spec: ToolSpec,
    pub class: ToolClass,
    /// The annotation title ("Search photos"), for the page.
    pub title: String,
}

pub struct Connection {
    client: RunningService<RoleClient, ChatClient>,
    pub tools: Vec<OfferedTool>,
    /// The server's own guidance for agents, which the chat passes on.
    pub instructions: String,
}

impl Connection {
    /// Start a server around `shared` and connect to it.
    ///
    /// # Errors
    /// Reports a server that would not start or list its tools — neither
    /// happens short of a bug, but the chat must say so rather than hang.
    pub async fn open(shared: Arc<Shared>) -> Result<Self, String> {
        let server = LrGeniusMcp::new(shared);
        let developer = LrGeniusMcp::developer_tool_names();
        let (server_side, client_side) = tokio::io::duplex(PIPE_BYTES);
        tokio::spawn(async move {
            match server.serve(server_side).await {
                Ok(running) => {
                    let _ = running.waiting().await;
                }
                Err(e) => log::error!("Chat: the in-process MCP server did not start: {e}"),
            }
        });
        let client = ChatClient
            .serve(client_side)
            .await
            .map_err(|e| format!("The chat could not connect to LrGeniusAI's tools: {e}"))?;
        let instructions = client
            .peer_info()
            .and_then(|info| info.instructions.clone())
            .unwrap_or_default();
        let tools = client
            .list_all_tools()
            .await
            .map_err(|e| format!("The chat could not list LrGeniusAI's tools: {e}"))?
            .into_iter()
            .filter(|tool| !developer.iter().any(|d| d == tool.name.as_ref()))
            .map(|tool| OfferedTool {
                class: ToolClass::of(tool.annotations.as_ref()),
                title: tool
                    .annotations
                    .as_ref()
                    .and_then(|a| a.title.clone())
                    .or_else(|| tool.title.clone())
                    .unwrap_or_else(|| tool.name.to_string()),
                spec: ToolSpec {
                    name: tool.name.to_string(),
                    description: tool.description.as_deref().unwrap_or_default().to_string(),
                    input_schema: with_photo_choice((*tool.input_schema).clone()),
                },
            })
            .collect();
        Ok(Connection {
            client,
            tools,
            instructions,
        })
    }

    #[must_use]
    pub fn tool(&self, name: &str) -> Option<&OfferedTool> {
        self.tools.iter().find(|t| t.spec.name == name)
    }

    /// Run one call. Never fails: a call that could not be made comes back
    /// as an error result, which is what the model has to read.
    pub async fn call(&self, call: &ToolCall) -> ToolResult {
        let arguments = call.arguments.as_object().cloned().unwrap_or_default();
        let params = CallToolRequestParams::new(call.name.clone()).with_arguments(arguments);
        match self.client.call_tool(params).await {
            Ok(result) => tool_result(call, &result),
            Err(e) => ToolResult {
                call_id: call.id.clone(),
                name: call.name.clone(),
                text: format!("The tool could not be called: {e}"),
                images: Vec::new(),
                is_error: true,
            },
        }
    }
}

/// Says in the schema that a tool taking photos needs exactly one of `set`
/// and `photo_ids` (`lrg_mcp::Photos`, flattened into its arguments).
///
/// The MCP schema leaves both optional because it cannot say "one of" where
/// it would have to — at the top of the tool's schema, which Anthropic's and
/// OpenAI's APIs refuse, so external agents would lose the tool. The chat's
/// local models decode against the schema, so for them it matters: Gemma 4
/// E4B left both out and the call failed.
fn with_photo_choice(mut schema: Map<String, Value>) -> Value {
    let free = |name: &str| {
        let listed = schema.get("properties").and_then(|p| p.get(name)).is_some();
        let required = schema
            .get("required")
            .and_then(Value::as_array)
            .is_some_and(|r| r.iter().any(|v| v == name));
        listed && !required
    };
    if free("set") && free("photo_ids") && !schema.contains_key("oneOf") {
        schema.insert(
            "oneOf".into(),
            json!([{"required": ["set"]}, {"required": ["photo_ids"]}]),
        );
    }
    Value::Object(schema)
}

fn tool_result(call: &ToolCall, result: &CallToolResult) -> ToolResult {
    let text = result
        .content
        .iter()
        .filter_map(|c| c.as_text())
        .map(|t| t.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let images = result
        .content
        .iter()
        .filter_map(|c| c.as_image())
        .filter_map(|image| {
            base64::engine::general_purpose::STANDARD
                .decode(&image.data)
                .ok()
                .map(|data| ChatImage {
                    data,
                    mime_type: image.mime_type.clone(),
                })
        })
        .collect();
    ToolResult {
        call_id: call.id.clone(),
        name: call.name.clone(),
        text,
        images,
        is_error: result.is_error.unwrap_or(false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema(value: Value) -> Map<String, Value> {
        value.as_object().unwrap().clone()
    }

    #[test]
    fn a_tool_taking_photos_needs_a_set_or_photo_ids() {
        let photos = schema(json!({
            "type": "object",
            "properties": {"set": {}, "photo_ids": {}, "rating": {}}
        }));
        assert_eq!(
            with_photo_choice(photos)["oneOf"],
            json!([{"required": ["set"]}, {"required": ["photo_ids"]}])
        );
        // Neither field, only one, or one already required: unchanged.
        for unchanged in [
            json!({"type": "object", "properties": {"query": {}}}),
            json!({"type": "object", "properties": {"set": {}}}),
            json!({"type": "object", "properties": {"set": {}, "photo_ids": {}}, "required": ["set"]}),
        ] {
            assert_eq!(with_photo_choice(schema(unchanged.clone())), unchanged);
        }
    }
}
