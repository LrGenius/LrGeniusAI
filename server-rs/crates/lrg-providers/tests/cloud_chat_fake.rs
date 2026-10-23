//! The chat's cloud adapters against a fake server on a local port: what each
//! sends for a conversation with tool calls, results and photos, and how it
//! reads the answer.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::Router;
use serde_json::{json, Value};

use lrg_providers::chat::{
    ChatImage, ChatMessage, ChatModel, ChatPart, ChatRequest, ChatRole, NativeTurn, ToolCall,
    ToolResult, ToolSpec,
};
use lrg_providers::cloud_chat::{build_chat_model, AnthropicChat, GeminiChat, OpenAiChat};
use lrg_providers::provider::ProviderSelection;

/// One request as the fake saw it.
#[derive(Debug, Clone)]
struct Seen {
    path: String,
    headers: HeaderMap,
    body: Value,
}

/// A scripted answer: status, `retry-after`, body.
type Answer = (u16, Option<&'static str>, Value);

#[derive(Clone, Default)]
struct Fake {
    seen: Arc<Mutex<Vec<Seen>>>,
    answers: Arc<Mutex<VecDeque<Answer>>>,
}

impl Fake {
    fn answering(answers: Vec<Answer>) -> Self {
        Fake {
            answers: Arc::new(Mutex::new(answers.into())),
            ..Fake::default()
        }
    }

    fn last(&self) -> Seen {
        self.seen
            .lock()
            .unwrap()
            .last()
            .cloned()
            .expect("a request")
    }
}

async fn answer(State(fake): State<Fake>, uri: Uri, headers: HeaderMap, body: Bytes) -> Response {
    fake.seen.lock().unwrap().push(Seen {
        path: uri.path().to_string(),
        headers,
        body: serde_json::from_slice(&body).unwrap_or(Value::Null),
    });
    let (status, retry_after, body) = fake
        .answers
        .lock()
        .unwrap()
        .pop_front()
        .expect("the test scripted enough answers");
    let mut response = (StatusCode::from_u16(status).unwrap(), body.to_string()).into_response();
    response
        .headers_mut()
        .insert("content-type", "application/json".parse().unwrap());
    if let Some(wait) = retry_after {
        response
            .headers_mut()
            .insert("retry-after", wait.parse().unwrap());
    }
    response
}

/// Start the fake; returns its base URL.
async fn serve(fake: Fake) -> String {
    let app = Router::new().fallback(answer).with_state(fake);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://127.0.0.1:{port}")
}

fn photo() -> ChatImage {
    ChatImage {
        data: vec![0xff, 0xd8, 0xff, 0xe0],
        mime_type: "image/jpeg".into(),
    }
}

fn tool() -> ToolSpec {
    ToolSpec {
        name: "update_photos".into(),
        description: "Changes photos.".into(),
        input_schema: json!({
            "$defs": {"Flag": {"type": "string", "enum": ["pick", "reject"]}},
            "type": "object",
            "properties": {
                "set": {"type": ["string", "null"]},
                "photo_ids": {"type": ["array", "null"], "items": {"type": "string"}},
                "flag": {"$ref": "#/$defs/Flag"}
            },
            "oneOf": [{"required": ["set"]}, {"required": ["photo_ids"]}]
        }),
    }
}

/// A user asks, the model calls a tool, the tool answers with a photo, and
/// the conversation goes on. `native` is the model's turn as its API wrote it.
fn conversation(native: Option<NativeTurn>) -> ChatRequest {
    let mut assistant = vec![
        ChatPart::Reasoning("Find them first.".into()),
        ChatPart::ToolCall(ToolCall {
            id: "toolu_1".into(),
            name: "view_photo".into(),
            arguments: json!({"photo_id": "p1"}),
        }),
    ];
    assistant.extend(native.map(ChatPart::Native));
    ChatRequest {
        system: "You work in Lightroom.".into(),
        messages: vec![
            ChatMessage::user_text("Show me p1."),
            ChatMessage {
                role: ChatRole::Assistant,
                parts: assistant,
            },
            ChatMessage {
                role: ChatRole::User,
                parts: vec![ChatPart::ToolResult(ToolResult {
                    call_id: "toolu_1".into(),
                    name: "view_photo".into(),
                    text: "{\"photo_id\":\"p1\"}".into(),
                    images: vec![photo()],
                    is_error: false,
                })],
            },
        ],
        tools: vec![tool()],
        max_tokens: 1536,
        temperature: 0.0,
    }
}

// ---------------------------------------------------------------------------

#[tokio::test]
async fn openai_sends_tool_messages_and_reads_tool_calls() {
    let fake = Fake::answering(vec![(
        200,
        None,
        json!({
            "choices": [{"finish_reason": "tool_calls", "message": {
                "content": null,
                "reasoning_content": "Rate it.",
                "tool_calls": [{"id": "call_9", "type": "function", "function": {
                    "name": "update_photos",
                    "arguments": "{\"photo_ids\": [\"p1\"], \"flag\": \"pick\"}"
                }}]
            }}],
            "usage": {"prompt_tokens": 1200, "completion_tokens": 30}
        }),
    )]);
    let base = serve(fake.clone()).await;
    let model = OpenAiChat::new(
        &format!("{base}/v1/chat/completions"),
        Some("sk-test".into()),
        "gpt-test",
        "OpenAI",
        true,
    );
    let turn = model.complete(&conversation(None), None).await.unwrap();

    let seen = fake.last();
    assert_eq!(seen.path, "/v1/chat/completions");
    assert_eq!(seen.headers["authorization"], "Bearer sk-test");
    let messages = seen.body["messages"].as_array().unwrap();
    assert_eq!(messages[0]["role"], "system");
    assert_eq!(messages[2]["role"], "assistant");
    assert_eq!(messages[2]["content"], Value::Null);
    assert_eq!(
        messages[2]["tool_calls"][0]["function"]["arguments"],
        "{\"photo_id\":\"p1\"}"
    );
    assert_eq!(messages[3]["role"], "tool");
    assert_eq!(messages[3]["tool_call_id"], "toolu_1");
    // The photo cannot ride in the tool message; it follows as the user's.
    assert_eq!(messages[4]["role"], "user");
    assert_eq!(messages[4]["content"][1]["type"], "image_url");
    let parameters = &seen.body["tools"][0]["function"]["parameters"];
    assert!(parameters.get("oneOf").is_none(), "{parameters}");
    assert_eq!(
        parameters["properties"]["flag"]["enum"],
        json!(["pick", "reject"])
    );
    assert_eq!(seen.body["max_completion_tokens"], json!(1536 + 8192));
    assert!(seen.body.get("temperature").is_none());

    assert_eq!(turn.reasoning.as_deref(), Some("Rate it."));
    assert_eq!(turn.tool_calls[0].id, "call_9");
    assert_eq!(turn.tool_calls[0].arguments["flag"], "pick");
    assert_eq!(turn.input_tokens, 1200);
}

#[tokio::test]
async fn a_server_gets_max_tokens_and_the_temperature_but_no_key() {
    let fake = Fake::answering(vec![(
        200,
        None,
        json!({"choices": [{"finish_reason": "stop", "message": {"content": "Done."}}]}),
    )]);
    let base = serve(fake.clone()).await;
    let model = OpenAiChat::new(
        &format!("{base}/v1/chat/completions"),
        None,
        "m",
        "LM Studio",
        false,
    );
    let turn = model.complete(&conversation(None), None).await.unwrap();

    let seen = fake.last();
    assert!(seen.headers.get("authorization").is_none());
    assert_eq!(seen.body["max_tokens"], json!(1536 + 2048));
    assert_eq!(seen.body["temperature"], json!(0.0));
    assert_eq!(turn.text, "Done.");
}

#[tokio::test]
async fn anthropic_sends_its_own_turn_back_and_caches_the_conversation() {
    let thinking =
        json!({"type": "thinking", "thinking": "Find them first.", "signature": "sig=="});
    let native = NativeTurn {
        api: "anthropic",
        content: vec![
            thinking.clone(),
            json!({"type": "tool_use", "id": "toolu_1", "name": "view_photo", "input": {"photo_id": "p1"}}),
        ],
    };
    let fake = Fake::answering(vec![(
        200,
        None,
        json!({
            "content": [
                {"type": "thinking", "thinking": "Flag it.", "signature": "sig2"},
                {"type": "text", "text": "Flagging it."},
                {"type": "tool_use", "id": "toolu_2", "name": "update_photos",
                 "input": {"photo_ids": ["p1"], "flag": "pick"}}
            ],
            "stop_reason": "tool_use",
            "usage": {"input_tokens": 20, "cache_read_input_tokens": 3000,
                      "cache_creation_input_tokens": 100, "output_tokens": 50}
        }),
    )]);
    let base = serve(fake.clone()).await;
    let model = AnthropicChat::new(&format!("{base}/v1"), "sk-ant".into(), "claude-test");
    let turn = model
        .complete(&conversation(Some(native)), None)
        .await
        .unwrap();

    let seen = fake.last();
    assert_eq!(seen.path, "/v1/messages");
    assert_eq!(seen.headers["x-api-key"], "sk-ant");
    assert_eq!(seen.headers["anthropic-version"], "2023-06-01");
    assert_eq!(seen.body["system"][0]["cache_control"]["type"], "ephemeral");
    let messages = seen.body["messages"].as_array().unwrap();
    // The thinking block goes back exactly as it came.
    assert_eq!(messages[1]["content"][0], thinking);
    let result = &messages[2]["content"][0];
    assert_eq!(result["type"], "tool_result");
    assert_eq!(result["tool_use_id"], "toolu_1");
    assert_eq!(result["content"][1]["type"], "image");
    assert_eq!(result["cache_control"]["type"], "ephemeral");
    let schema = &seen.body["tools"][0]["input_schema"];
    assert!(
        schema.get("oneOf").is_none() && schema.get("$defs").is_none(),
        "{schema}"
    );

    assert_eq!(turn.reasoning.as_deref(), Some("Flag it."));
    assert_eq!(turn.text, "Flagging it.");
    assert_eq!(turn.tool_calls[0].id, "toolu_2");
    assert_eq!(turn.input_tokens, 3120);
    let native = turn.native.expect("Claude's own turn is kept");
    assert_eq!(native.content[0]["signature"], "sig2");
}

#[tokio::test]
async fn gemini_answers_calls_by_id_and_keeps_thought_signatures() {
    let call = json!({"functionCall": {"id": "fc_1", "name": "view_photo", "args": {"photo_id": "p1"}},
                      "thoughtSignature": "sig=="});
    let native = NativeTurn {
        api: "gemini",
        content: vec![call.clone()],
    };
    let mut request = conversation(Some(native));
    // Gemini named its call; the tool result answers that id.
    if let ChatPart::ToolResult(result) = &mut request.messages[2].parts[0] {
        result.call_id = "fc_1".into();
    }
    let fake = Fake::answering(vec![(
        200,
        None,
        json!({
            "candidates": [{"finishReason": "STOP", "content": {"role": "model", "parts": [
                {"text": "Thinking about flags.", "thought": true},
                {"functionCall": {"id": "fc_2", "name": "update_photos",
                                  "args": {"photo_ids": ["p1"], "flag": "pick"}},
                 "thoughtSignature": "sig2"}
            ]}}],
            "usageMetadata": {"promptTokenCount": 900, "candidatesTokenCount": 20,
                              "thoughtsTokenCount": 40}
        }),
    )]);
    let base = serve(fake.clone()).await;
    let model = GeminiChat::new(
        &format!("{base}/v1beta"),
        "g-key".into(),
        "models/gemini-test",
    );
    let turn = model.complete(&request, None).await.unwrap();

    let seen = fake.last();
    assert_eq!(seen.path, "/v1beta/models/gemini-test:generateContent");
    assert_eq!(seen.headers["x-goog-api-key"], "g-key");
    let contents = seen.body["contents"].as_array().unwrap();
    assert_eq!(contents[1]["role"], "model");
    assert_eq!(
        contents[1]["parts"][0], call,
        "the model's turn goes back as it came"
    );
    let response = &contents[2]["parts"][0]["functionResponse"];
    assert_eq!(response["id"], "fc_1");
    assert_eq!(response["name"], "view_photo");
    assert_eq!(response["response"]["result"], "{\"photo_id\":\"p1\"}");
    assert!(contents[2]["parts"][1].get("inlineData").is_some());
    let declaration = &seen.body["tools"][0]["functionDeclarations"][0];
    assert!(declaration["parametersJsonSchema"].get("oneOf").is_none());

    assert_eq!(turn.reasoning.as_deref(), Some("Thinking about flags."));
    assert_eq!(turn.tool_calls[0].id, "fc_2");
    assert_eq!(turn.output_tokens, 60);
    assert_eq!(turn.native.unwrap().content[1]["thoughtSignature"], "sig2");
}

#[tokio::test]
async fn only_the_newest_photos_stay_attached() {
    let fake = Fake::answering(vec![(
        200,
        None,
        json!({"content": [{"type": "text", "text": "ok"}], "stop_reason": "end_turn"}),
    )]);
    let base = serve(fake.clone()).await;
    let model = AnthropicChat::new(&format!("{base}/v1"), "k".into(), "m");
    let mut request = ChatRequest {
        system: String::new(),
        max_tokens: 100,
        ..ChatRequest::default()
    };
    for i in 0..6 {
        request.messages.push(ChatMessage {
            role: ChatRole::User,
            parts: vec![
                ChatPart::Text(format!("photo {i}")),
                ChatPart::Image(photo()),
            ],
        });
        request.messages.push(ChatMessage {
            role: ChatRole::Assistant,
            parts: vec![ChatPart::Text("seen".into())],
        });
    }
    request
        .messages
        .push(ChatMessage::user_text("Which was best?"));
    model.complete(&request, None).await.unwrap();

    let body = fake.last().body.to_string();
    assert_eq!(body.matches("\"type\":\"image\"").count(), 4, "{body}");
    assert_eq!(body.matches("no longer attached").count(), 2);
}

#[tokio::test]
async fn a_rate_limit_is_waited_out_and_a_bad_key_is_named() {
    let fake = Fake::answering(vec![
        (429, Some("0"), json!({"error": {"message": "slow down"}})),
        (
            200,
            None,
            json!({"content": [{"type": "text", "text": "ok"}], "stop_reason": "end_turn"}),
        ),
        (
            401,
            None,
            json!({"type": "error", "error": {"type": "authentication_error", "message": "invalid x-api-key"}}),
        ),
    ]);
    let base = serve(fake.clone()).await;
    let model = AnthropicChat::new(&format!("{base}/v1"), "k".into(), "m");
    let request = ChatRequest {
        messages: vec![ChatMessage::user_text("Hi")],
        max_tokens: 100,
        ..ChatRequest::default()
    };
    assert_eq!(model.complete(&request, None).await.unwrap().text, "ok");
    let error = model.complete(&request, None).await.unwrap_err();
    assert!(error.contains("rejected the API key"), "{error}");
    assert!(error.contains("invalid x-api-key"), "{error}");
}

#[test]
fn the_chat_model_needs_what_its_provider_needs() {
    let selection = |name: &str, key: Option<&str>| ProviderSelection {
        name: name.into(),
        api_key: key.map(str::to_string),
        ..ProviderSelection::default()
    };
    for name in ["anthropic", "gemini", "chatgpt"] {
        let error = build_chat_model(&selection(name, None), "m").err().unwrap();
        assert!(error.contains("API key is not set"), "{name}: {error}");
        assert!(
            build_chat_model(&selection(name, Some("k")), "m").is_ok(),
            "{name}"
        );
    }
    let error = build_chat_model(&selection("openai_compatible", None), "m")
        .err()
        .unwrap();
    assert!(error.contains("No AI server address"), "{error}");
    for name in ["lmstudio", "ollama"] {
        let model = build_chat_model(&selection(name, None), "llama").unwrap();
        assert!(model.name().contains("llama"), "{name}");
    }
    assert!(build_chat_model(&selection("mlx", None), "m").is_err());
    assert!(build_chat_model(&selection("anthropic", Some("k")), " ").is_err());
}
