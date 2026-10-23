//! A local engine as a [`ChatModel`]: tool calling by constrained decoding.
//!
//! A local model cannot use its native tool-call format here. That format
//! lives in the model's Jinja chat template, which llama.cpp's C API cannot
//! render (it falls back to built-in templates that know only system, user
//! and assistant). What both local engines *can* do is constrain decoding to
//! a JSON Schema, so every answer is constrained to one of
//!
//! ```json
//! {"analysis": "...", "call": {"search_photos": {"query": "dog on a beach"}}}
//! {"analysis": "...", "reply": "Here are your photos."}
//! ```
//!
//! with one schema branch per tool. A small model can then neither invent a
//! tool nor send arguments its schema rejects.
//!
//! The key names are chosen for their alphabetical order. Both grammar
//! engines emit properties in the order the schema lists them, and
//! `serde_json` lists them alphabetically unless its `preserve_order` feature
//! is on — which only llguidance turns on, so only in builds with llama.cpp.
//! `analysis` < `call` < `reply` keeps the reasoning first and, because the
//! tool's name is the key under `call`, the tool is chosen before its
//! arguments are written, whichever way the build sorts.

use std::collections::HashSet;

use async_trait::async_trait;
use serde_json::{json, Map, Value};

use crate::chat::{
    ChatDelta, ChatDeltas, ChatMessage, ChatModel, ChatPart, ChatRequest, ChatRole, ChatTurn,
    ToolCall, ToolSpec,
};
use crate::image_encode::image_to_rgb;
use crate::local::{
    LocalChatMessage, LocalChatRequest, LocalChatRole, LocalImage, SharedLocalEngine,
};

/// Photos kept as pixels, counted from the end of the conversation. Each one
/// costs a few hundred tokens or more and is evaluated again every turn;
/// older ones become a line of text.
const MAX_IMAGES: usize = 2;

/// Longest argument description in the tool list, in characters.
const MAX_ARG_DESCRIPTION: usize = 200;

/// Longest string the grammar lets a model write, in characters: a tool
/// argument, the reasoning, the reply. Small models fall into repeating a
/// phrase inside a string until the token budget runs out — Gemma 4 12B
/// filled a search text with `rating_max_5_rating_min_1_…` for 1,500
/// tokens — and a cap ends that early instead of carrying it in the history.
/// Captions are the longest real argument and fit easily.
const MAX_ARGUMENT_CHARS: u64 = 1000;
const MAX_ANALYSIS_CHARS: u64 = 600;
const MAX_REPLY_CHARS: u64 = 6000;

pub struct LocalChatModel {
    engine: SharedLocalEngine,
}

impl LocalChatModel {
    #[must_use]
    pub fn new(engine: SharedLocalEngine) -> Self {
        LocalChatModel { engine }
    }
}

#[async_trait]
impl ChatModel for LocalChatModel {
    fn name(&self) -> String {
        self.engine.model_name()
    }

    fn context_window(&self) -> Option<u32> {
        self.engine.context_window()
    }

    fn supports_images(&self) -> bool {
        self.engine.supports_vision()
    }

    async fn complete(
        &self,
        request: &ChatRequest,
        deltas: Option<ChatDeltas>,
    ) -> Result<ChatTurn, String> {
        let mut messages = vec![LocalChatMessage {
            role: LocalChatRole::System,
            text: system_prompt(&request.system, &request.tools),
            images: Vec::new(),
        }];
        messages.extend(render_messages(
            &request.messages,
            self.engine.supports_vision(),
        )?);
        let local = LocalChatRequest {
            messages,
            schema: Some(turn_schema(&request.tools)),
            max_tokens: request.max_tokens,
            temperature: request.temperature,
        };

        // The engine streams raw JSON; the page wants the text inside it.
        let (raw_tx, mut raw_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        let forward = async move {
            let mut stream = TurnStream::default();
            while let Some(raw) = raw_rx.recv().await {
                if let Some(deltas) = &deltas {
                    for delta in stream.feed(&raw) {
                        let _ = deltas.send(delta);
                    }
                }
            }
        };
        // The engine drops its sender when it is done, which ends `forward`.
        let (response, ()) = tokio::join!(self.engine.chat(local, Some(raw_tx)), forward);
        let response = response?;

        let ran_out = response.completion_tokens >= request.max_tokens;
        let mut turn = parse_turn(&response.text, ran_out, request)?;
        turn.input_tokens = response.prompt_tokens;
        turn.output_tokens = response.completion_tokens;
        Ok(turn)
    }
}

// ---------------------------------------------------------------------------
// Prompt
// ---------------------------------------------------------------------------

fn system_prompt(system: &str, tools: &[ToolSpec]) -> String {
    let mut prompt = String::from(system.trim());
    if !tools.is_empty() {
        prompt.push_str("\n\n# Tools\n");
        prompt.push_str(&describe_tools(tools));
    }
    prompt.push_str("\n\n# How to answer\n\nEvery answer is one JSON object");
    if tools.is_empty() {
        prompt.push_str(":\n{\"analysis\": \"...\", \"reply\": \"...\"}\n\n");
    } else {
        prompt.push_str(
            ", either\n\
             {\"analysis\": \"...\", \"call\": {\"<tool name>\": {<arguments>}}} to use a tool, or\n\
             {\"analysis\": \"...\", \"reply\": \"...\"} to answer the user.\n\n",
        );
    }
    prompt.push_str(
        "\"analysis\" is one short sentence: what you know so far and what is still missing.",
    );
    // The failure this guards against, seen with Gemma 4 12B: an analysis
    // that says "I will check the status", then a reply instead of the call.
    if !tools.is_empty() {
        prompt.push_str(
            "\n\nA reply ends your turn: the user reads it, and nothing else happens until they \
             write again. So never reply with what you are about to do — do it, with a call. Use \
             one tool per answer; its result comes back in the next message. Leave out arguments \
             you do not need. Reply once the task is done, or when the user has to decide \
             something.",
        );
    }
    prompt
}

/// The tools as a compact list: each tool's own description, then one line
/// per argument. The grammar enforces the exact shape, so this only has to
/// say what the arguments mean — and only once: many tools share `set` and
/// `photo_ids`, and repeating their descriptions cost about 600 tokens.
fn describe_tools(tools: &[ToolSpec]) -> String {
    let mut out = String::new();
    let mut described: HashSet<(String, String)> = HashSet::new();
    for tool in tools {
        out.push_str(&format!(
            "\n## {}\n{}\n",
            tool.name,
            collapse(&tool.description)
        ));
        let schema = &tool.input_schema;
        let defs = schema.get("$defs").or_else(|| schema.get("definitions"));
        let required: Vec<&str> = schema
            .get("required")
            .and_then(Value::as_array)
            .map(|r| r.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
            continue;
        };
        for (name, property) in properties {
            let target = resolve(property, defs);
            out.push_str(&format!("- {name} ({}", type_label(target, defs)));
            if required.contains(&name.as_str()) {
                out.push_str(", required");
            }
            out.push(')');
            // The description written next to a reference says what the
            // argument is for; the referenced type's own one is the fallback.
            let description = property
                .get("description")
                .or_else(|| target.get("description"))
                .and_then(Value::as_str);
            if let Some(description) = description {
                let description = truncate(&collapse(description), MAX_ARG_DESCRIPTION);
                if described.insert((name.clone(), description.clone())) {
                    out.push_str(": ");
                    out.push_str(&description);
                }
            }
            out.push('\n');
        }
        if let Some((branches, exclusive)) = required_choice(schema) {
            let options: Vec<String> = branches.iter().map(|b| b.join(" + ")).collect();
            out.push_str(&format!(
                "- required: {} of {}\n",
                if exclusive {
                    "exactly one"
                } else {
                    "at least one"
                },
                options.join(", ")
            ));
        }
    }
    out
}

/// The branches of a "give one of these arguments" choice — a `oneOf` or
/// `anyOf` whose branches hold nothing but `required` — and whether it is
/// exclusive (`oneOf`).
fn required_choice(schema: &Value) -> Option<(Vec<Vec<String>>, bool)> {
    let (branches, exclusive) = match (schema.get("oneOf"), schema.get("anyOf")) {
        (Some(branches), _) => (branches, true),
        (None, Some(branches)) => (branches, false),
        (None, None) => return None,
    };
    let branches = branches
        .as_array()?
        .iter()
        .map(|branch| {
            let branch = branch.as_object()?;
            if branch.len() != 1 {
                return None;
            }
            branch
                .get("required")?
                .as_array()?
                .iter()
                .map(|name| name.as_str().map(str::to_string))
                .collect::<Option<Vec<_>>>()
        })
        .collect::<Option<Vec<_>>>()?;
    (!branches.is_empty()).then_some((branches, exclusive))
}

/// A `$ref` resolved against the tool's own definitions.
fn resolve<'a>(schema: &'a Value, defs: Option<&'a Value>) -> &'a Value {
    schema
        .get("$ref")
        .and_then(Value::as_str)
        .and_then(|r| defs?.get(r.rsplit('/').next()?))
        .unwrap_or(schema)
}

fn type_label(schema: &Value, defs: Option<&Value>) -> String {
    let schema = resolve(schema, defs);
    let consts: Vec<String> = schema
        .get("enum")
        .and_then(Value::as_array)
        .map(|values| values.iter().map(Value::to_string).collect())
        .or_else(|| {
            let options = schema
                .get("oneOf")
                .or_else(|| schema.get("anyOf"))?
                .as_array()?;
            options
                .iter()
                .map(|o| o.get("const").map(Value::to_string))
                .collect()
        })
        .unwrap_or_default();
    if !consts.is_empty() {
        return consts.join(" | ");
    }
    let types: Vec<&str> = match schema.get("type") {
        Some(Value::String(t)) => vec![t.as_str()],
        Some(Value::Array(ts)) => ts
            .iter()
            .filter_map(Value::as_str)
            .filter(|t| *t != "null")
            .collect(),
        _ => Vec::new(),
    };
    match types.first().copied() {
        Some("array") => match schema.get("items") {
            Some(items) => format!("list of {}", type_label(items, defs)),
            None => "list".to_string(),
        },
        Some("object") => match schema.get("properties").and_then(Value::as_object) {
            Some(properties) if !properties.is_empty() => format!(
                "object with {}",
                properties.keys().cloned().collect::<Vec<_>>().join(", ")
            ),
            _ => "object".to_string(),
        },
        Some(t) => t.to_string(),
        None => "any".to_string(),
    }
}

fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let cut: String = text.chars().take(max).collect();
    let cut = cut.rsplit_once(' ').map_or(cut.as_str(), |(head, _)| head);
    format!("{cut}…")
}

// ---------------------------------------------------------------------------
// Conversation
// ---------------------------------------------------------------------------

/// The conversation in engine form. Tool results become user turns, the
/// model's own turns are rendered back as the JSON it produced, and only the
/// last [`MAX_IMAGES`] photos stay pixels.
fn render_messages(
    messages: &[ChatMessage],
    vision: bool,
) -> Result<Vec<LocalChatMessage>, String> {
    let total_images: usize = messages
        .iter()
        .flat_map(|m| &m.parts)
        .map(|part| match part {
            ChatPart::Image(_) => 1,
            ChatPart::ToolResult(result) => result.images.len(),
            _ => 0,
        })
        .sum();
    let mut seen = 0usize;
    let mut take_image = |data: &[u8]| -> Result<Option<LocalImage>, String> {
        seen += 1;
        if !vision || seen + MAX_IMAGES <= total_images {
            return Ok(None);
        }
        let (width, height, rgb) = image_to_rgb(data).map_err(|e| e.to_string())?;
        Ok(Some(LocalImage { width, height, rgb }))
    };
    let placeholder = if vision {
        "[a photo shown earlier]"
    } else {
        "[a photo this model cannot see]"
    };

    let mut out = Vec::with_capacity(messages.len());
    for message in messages {
        let mut texts: Vec<String> = Vec::new();
        let mut images = Vec::new();
        match message.role {
            ChatRole::User => {
                for part in &message.parts {
                    match part {
                        ChatPart::Text(text) => texts.push(text.clone()),
                        ChatPart::Image(image) => match take_image(&image.data)? {
                            Some(pixels) => images.push(pixels),
                            None => texts.push(placeholder.to_string()),
                        },
                        ChatPart::ToolResult(result) => {
                            let header = if result.is_error {
                                format!("{} failed:", result.name)
                            } else {
                                format!("Result of {}:", result.name)
                            };
                            texts.push(format!("{header}\n{}", compact_json(&result.text)));
                            for image in &result.images {
                                match take_image(&image.data)? {
                                    Some(pixels) => images.push(pixels),
                                    None => texts.push(placeholder.to_string()),
                                }
                            }
                        }
                        ChatPart::Reasoning(_) | ChatPart::ToolCall(_) | ChatPart::Native(_) => {}
                    }
                }
            }
            ChatRole::Assistant => texts.push(assistant_json(message)),
        }
        out.push(LocalChatMessage {
            role: match message.role {
                ChatRole::User => LocalChatRole::User,
                ChatRole::Assistant => LocalChatRole::Assistant,
            },
            text: texts.join("\n\n"),
            images,
        });
    }
    Ok(out)
}

/// One of the model's turns, as the JSON it would have produced. A turn from
/// a model that called several tools at once keeps only the first: this
/// format has room for one.
fn assistant_json(message: &ChatMessage) -> String {
    let mut analysis = String::new();
    let mut reply = String::new();
    let mut call = None;
    for part in &message.parts {
        match part {
            ChatPart::Reasoning(text) => analysis.push_str(text),
            ChatPart::Text(text) => reply.push_str(text),
            ChatPart::ToolCall(c) if call.is_none() => call = Some(c),
            _ => {}
        }
    }
    let mut object = Map::new();
    object.insert("analysis".into(), Value::String(analysis));
    match call {
        Some(call) => {
            object.insert("call".into(), json!({ call.name.clone(): call.arguments }));
        }
        None => {
            object.insert("reply".into(), Value::String(reply));
        }
    }
    Value::Object(object).to_string()
}

/// Tool results arrive as pretty-printed JSON, and the indentation alone can
/// cost a third of the tokens. Anything that is not JSON passes unchanged.
fn compact_json(text: &str) -> String {
    serde_json::from_str::<Value>(text).map_or_else(|_| text.to_string(), |v| v.to_string())
}

// ---------------------------------------------------------------------------
// Schema
// ---------------------------------------------------------------------------

/// The schema every answer is decoded against.
fn turn_schema(tools: &[ToolSpec]) -> Value {
    let analysis = json!({"type": "string", "maxLength": MAX_ANALYSIS_CHARS});
    let reply = json!({
        "type": "object",
        "properties": {
            "analysis": analysis,
            "reply": {"type": "string", "maxLength": MAX_REPLY_CHARS},
        },
        "required": ["analysis", "reply"],
        "additionalProperties": false,
    });
    if tools.is_empty() {
        return reply;
    }
    let calls: Vec<Value> = tools
        .iter()
        .map(|tool| {
            json!({
                "type": "object",
                "properties": { tool.name.clone(): grammar_schema(&tool.input_schema) },
                "required": [tool.name],
                "additionalProperties": false,
            })
        })
        .collect();
    let call = json!({
        "type": "object",
        "properties": {
            "analysis": analysis,
            "call": {"anyOf": calls},
        },
        "required": ["analysis", "call"],
        "additionalProperties": false,
    });
    json!({ "anyOf": [call, reply] })
}

/// A tool's input schema as a grammar can take it.
///
/// One grammar holds every tool, but each tool's schema brings its own
/// `$defs`, so references are inlined. `oneOf` cannot be enforced while
/// decoding and is read as `anyOf` — except a choice of required arguments
/// ([`required_choice`]), which becomes one object per branch
/// ([`split_choice`]). Objects are closed, or a model could add
/// keys the tool ignores. Everything else — descriptions, defaults, `format`
/// (`schemars` writes `int64`, `double`, which grammar engines reject or
/// ignore unevenly), numeric bounds — is dropped: the prompt carries the
/// descriptions, and the tool still validates its arguments. Strings get a
/// length cap ([`MAX_ARGUMENT_CHARS`]); both engines enforce `maxLength`.
fn grammar_schema(schema: &Value) -> Value {
    let defs = schema.get("$defs").or_else(|| schema.get("definitions"));
    clean_schema(schema, defs, 0)
}

fn clean_schema(schema: &Value, defs: Option<&Value>, depth: usize) -> Value {
    let Some(object) = schema.as_object() else {
        return schema.clone();
    };
    // Deep enough for any tool here; stops a recursive definition.
    if depth > 12 {
        return json!({});
    }
    if let Some(reference) = object.get("$ref").and_then(Value::as_str) {
        return reference
            .rsplit('/')
            .next()
            .and_then(|name| defs?.get(name))
            .map_or_else(|| json!({}), |target| clean_schema(target, defs, depth + 1));
    }

    let choice = required_choice(schema);
    let mut out = Map::new();
    for (key, value) in object {
        match key.as_str() {
            "type" | "enum" | "const" | "required" => {
                out.insert(key.clone(), value.clone());
            }
            "anyOf" | "oneOf" if choice.is_some() => {}
            "properties" => {
                let properties = value
                    .as_object()
                    .map(|p| {
                        p.iter()
                            .map(|(name, s)| (name.clone(), clean_schema(s, defs, depth + 1)))
                            .collect::<Map<_, _>>()
                    })
                    .unwrap_or_default();
                out.insert(key.clone(), Value::Object(properties));
            }
            "items" | "additionalProperties" => {
                out.insert(key.clone(), clean_schema(value, defs, depth + 1));
            }
            "anyOf" | "oneOf" => {
                let options = value
                    .as_array()
                    .map(|o| o.iter().map(|s| clean_schema(s, defs, depth + 1)).collect())
                    .unwrap_or_default();
                out.insert("anyOf".into(), Value::Array(options));
            }
            _ => {}
        }
    }

    let has_type = |out: &Map<String, Value>, name: &str| match out.get("type") {
        Some(Value::String(t)) => t == name,
        Some(Value::Array(ts)) => ts.iter().any(|t| t == name),
        _ => false,
    };
    if has_type(&out, "string") && !out.contains_key("enum") && !out.contains_key("const") {
        out.insert("maxLength".into(), json!(MAX_ARGUMENT_CHARS));
    }
    let is_object =
        has_type(&out, "object") || (!out.contains_key("type") && out.contains_key("properties"));
    if is_object && !out.contains_key("additionalProperties") {
        out.entry("properties").or_insert_with(|| json!({}));
        out.insert("additionalProperties".into(), Value::Bool(false));
    }
    match choice {
        Some((branches, exclusive)) if is_object => split_choice(out, &branches, exclusive),
        _ => Value::Object(out),
    }
}

/// An object with a choice of required arguments, as one object per branch:
/// both grammar engines enforce `required`, but not a choice made beside
/// `properties`. An exclusive choice leaves the other branches' arguments out
/// of each object, so a model cannot give both. A choice naming an argument
/// the object does not have is dropped, as it could never be met.
fn split_choice(base: Map<String, Value>, branches: &[Vec<String>], exclusive: bool) -> Value {
    let listed = |name: &String| {
        base.get("properties")
            .and_then(Value::as_object)
            .is_some_and(|p| p.contains_key(name))
    };
    if !branches.iter().flatten().all(listed) {
        return Value::Object(base);
    }
    let variants: Vec<Value> = branches
        .iter()
        .map(|branch| {
            let mut variant = base.clone();
            let mut required = variant
                .get("required")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            for name in branch {
                if !required.iter().any(|r| r == name) {
                    required.push(json!(name));
                }
            }
            variant.insert("required".into(), Value::Array(required));
            if exclusive {
                if let Some(Value::Object(properties)) = variant.get_mut("properties") {
                    for other in branches.iter().flatten().filter(|n| !branch.contains(n)) {
                        properties.remove(other);
                    }
                }
            }
            Value::Object(variant)
        })
        .collect();
    json!({ "anyOf": variants })
}

// ---------------------------------------------------------------------------
// Answer
// ---------------------------------------------------------------------------

fn parse_turn(text: &str, ran_out: bool, request: &ChatRequest) -> Result<ChatTurn, String> {
    let value: Value = serde_json::from_str(text.trim()).map_err(|e| {
        if ran_out {
            format!(
                "The model ran out of room for its answer ({} tokens) before it finished. \
                 Ask for less at once, or start a new chat.",
                request.max_tokens
            )
        } else {
            format!(
                "The model's answer was not valid JSON ({e}): {}",
                text.chars().take(200).collect::<String>()
            )
        }
    })?;

    let reasoning = value
        .get("analysis")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|a| !a.is_empty())
        .map(str::to_string);

    if let Some(reply) = value.get("reply").and_then(Value::as_str) {
        return Ok(ChatTurn {
            reasoning,
            text: reply.to_string(),
            ..ChatTurn::default()
        });
    }

    let Some((name, arguments)) = value
        .get("call")
        .and_then(Value::as_object)
        .and_then(|call| call.iter().next())
    else {
        return Err(format!(
            "The model's answer had neither a reply nor a tool call: {}",
            text.chars().take(200).collect::<String>()
        ));
    };
    if !request.tools.iter().any(|t| &t.name == name) {
        return Err(format!(
            "The model called \"{name}\", which is not offered."
        ));
    }
    // Optional arguments default to null, so a null says nothing the tool
    // does not already assume — and small models like to fill in every one.
    let arguments = match arguments {
        Value::Object(map) => Value::Object(
            map.iter()
                .filter(|(_, v)| !v.is_null())
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        ),
        other => other.clone(),
    };
    let calls_so_far = request
        .messages
        .iter()
        .flat_map(|m| &m.parts)
        .filter(|p| matches!(p, ChatPart::ToolCall(_)))
        .count();
    Ok(ChatTurn {
        reasoning,
        tool_calls: vec![ToolCall {
            id: format!("call_{}", calls_so_far + 1),
            name: name.clone(),
            arguments,
        }],
        ..ChatTurn::default()
    })
}

/// Pulls the text of the top-level `analysis` and `reply` strings out of an
/// answer while its JSON is still being generated. Strings anywhere else —
/// inside a tool's arguments, say — are skipped.
#[derive(Default)]
struct TurnStream {
    depth: usize,
    in_string: bool,
    /// The string being read is a key of the top-level object.
    reading_key: bool,
    /// The next string at the top level is a key, not a value.
    expect_key: bool,
    key: String,
    field: Option<Field>,
    escape: Escape,
    /// A UTF-16 high surrogate waiting for its partner.
    high_surrogate: Option<u32>,
}

#[derive(Clone, Copy)]
enum Field {
    Analysis,
    Reply,
}

#[derive(Default)]
enum Escape {
    #[default]
    None,
    Backslash,
    Unicode(String),
}

impl TurnStream {
    fn feed(&mut self, raw: &str) -> Vec<ChatDelta> {
        let mut out: Vec<ChatDelta> = Vec::new();
        for c in raw.chars() {
            if let Some(decoded) = self.step(c) {
                let field = self.field.expect("text is only decoded inside a field");
                match (out.last_mut(), field) {
                    (Some(ChatDelta::Reasoning(text)), Field::Analysis)
                    | (Some(ChatDelta::Text(text)), Field::Reply) => text.push(decoded),
                    (_, Field::Analysis) => out.push(ChatDelta::Reasoning(decoded.into())),
                    (_, Field::Reply) => out.push(ChatDelta::Text(decoded.into())),
                }
            }
        }
        out
    }

    /// Advances by one character and returns the character of a streamed
    /// field it decoded, if any.
    fn step(&mut self, c: char) -> Option<char> {
        if !self.in_string {
            match c {
                '{' | '[' => {
                    self.depth += 1;
                    self.expect_key = self.depth == 1 && c == '{';
                }
                '}' | ']' => self.depth = self.depth.saturating_sub(1),
                ',' if self.depth == 1 => self.expect_key = true,
                ':' if self.depth == 1 => self.expect_key = false,
                '"' => {
                    self.in_string = true;
                    self.reading_key = self.depth == 1 && self.expect_key;
                    if self.reading_key {
                        self.key.clear();
                    }
                    self.field = match (self.depth, self.reading_key, self.key.as_str()) {
                        (1, false, "analysis") => Some(Field::Analysis),
                        (1, false, "reply") => Some(Field::Reply),
                        _ => None,
                    };
                }
                _ => {}
            }
            return None;
        }

        let decoded = match std::mem::take(&mut self.escape) {
            Escape::None => match c {
                '\\' => {
                    self.escape = Escape::Backslash;
                    return None;
                }
                '"' => {
                    self.in_string = false;
                    self.reading_key = false;
                    self.field = None;
                    return None;
                }
                c => c,
            },
            Escape::Backslash => match c {
                'n' => '\n',
                't' => '\t',
                'r' => '\r',
                'b' => '\u{8}',
                'f' => '\u{c}',
                'u' => {
                    self.escape = Escape::Unicode(String::new());
                    return None;
                }
                other => other,
            },
            Escape::Unicode(mut hex) => {
                hex.push(c);
                if hex.len() < 4 {
                    self.escape = Escape::Unicode(hex);
                    return None;
                }
                let unit = u32::from_str_radix(&hex, 16).unwrap_or(0xFFFD);
                if (0xD800..0xDC00).contains(&unit) {
                    self.high_surrogate = Some(unit);
                    return None;
                }
                let code = match self.high_surrogate.take() {
                    Some(high) if (0xDC00..0xE000).contains(&unit) => {
                        0x10000 + ((high - 0xD800) << 10) + (unit - 0xDC00)
                    }
                    _ => unit,
                };
                char::from_u32(code).unwrap_or(char::REPLACEMENT_CHARACTER)
            }
        };
        if self.reading_key {
            self.key.push(decoded);
            return None;
        }
        self.field.map(|_| decoded)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::chat::{ChatImage, ToolResult};
    use crate::local::{LocalEngine, LocalRequest, LocalResponse, TextDeltas};

    fn tool(name: &str, schema: Value) -> ToolSpec {
        ToolSpec {
            name: name.into(),
            description: format!("Does {name}."),
            input_schema: schema,
        }
    }

    fn search_tool() -> ToolSpec {
        tool(
            "search_photos",
            json!({
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "$defs": {
                    "Sort": {"oneOf": [
                        {"const": "relevance", "type": "string", "description": "Best first."},
                        {"const": "aesthetic", "type": "string"}
                    ]},
                    "Photos": {
                        "description": "Which photos.",
                        "type": "object",
                        "properties": {"set": {"type": ["string", "null"], "default": null}}
                    }
                },
                "type": "object",
                "properties": {
                    "query": {"type": ["string", "null"], "default": null,
                              "description": "What to look for,\n  in plain words."},
                    "limit": {"type": ["integer", "null"], "format": "int64", "minimum": 0},
                    "sort": {"$ref": "#/$defs/Sort", "description": "Order."},
                    "within": {"$ref": "#/$defs/Photos"}
                },
                "required": ["query"]
            }),
        )
    }

    #[test]
    fn grammar_schema_inlines_references_and_keeps_only_grammar_keywords() {
        let cleaned = grammar_schema(&search_tool().input_schema);
        assert_eq!(
            cleaned,
            json!({
                "type": "object",
                "properties": {
                    "query": {"type": ["string", "null"], "maxLength": 1000},
                    "limit": {"type": ["integer", "null"]},
                    "sort": {"anyOf": [
                        {"const": "relevance", "type": "string"},
                        {"const": "aesthetic", "type": "string"}
                    ]},
                    "within": {
                        "type": "object",
                        "properties": {"set": {"type": ["string", "null"], "maxLength": 1000}},
                        "additionalProperties": false
                    }
                },
                "required": ["query"],
                "additionalProperties": false
            })
        );
    }

    fn update_tool() -> ToolSpec {
        tool(
            "update_photos",
            json!({
                "type": "object",
                "properties": {
                    "set": {"type": ["string", "null"]},
                    "photo_ids": {"type": ["array", "null"], "items": {"type": "string"}},
                    "rating": {"type": ["integer", "null"]}
                },
                "oneOf": [{"required": ["set"]}, {"required": ["photo_ids"]}]
            }),
        )
    }

    #[test]
    fn a_choice_of_required_arguments_becomes_one_object_per_branch() {
        let cleaned = grammar_schema(&update_tool().input_schema);
        let variants = cleaned["anyOf"].as_array().expect("split into branches");
        assert_eq!(variants.len(), 2, "{cleaned}");
        assert_eq!(variants[0]["required"], json!(["set"]));
        assert_eq!(variants[1]["required"], json!(["photo_ids"]));
        // Exclusive: neither branch can give the other's argument.
        assert!(variants[0]["properties"].get("photo_ids").is_none());
        assert!(variants[1]["properties"].get("set").is_none());
        for variant in variants {
            assert_eq!(variant["additionalProperties"], json!(false));
            assert!(variant["properties"].get("rating").is_some());
        }

        // Not exclusive: both stay possible.
        let mut schema = update_tool().input_schema;
        let branches = schema.as_object_mut().unwrap().remove("oneOf").unwrap();
        schema["anyOf"] = branches;
        let cleaned = grammar_schema(&schema);
        assert!(cleaned["anyOf"][0]["properties"].get("photo_ids").is_some());

        // A choice the object cannot meet is not enforced.
        let cleaned = grammar_schema(&json!({
            "type": "object",
            "properties": {"set": {"type": "string"}},
            "oneOf": [{"required": ["set"]}, {"required": ["photo_ids"]}]
        }));
        assert!(cleaned.get("anyOf").is_none(), "{cleaned}");
        assert_eq!(cleaned["additionalProperties"], json!(false));
    }

    #[test]
    fn the_tool_list_states_a_choice_of_arguments() {
        let listed = describe_tools(&[update_tool()]);
        assert!(
            listed.contains("- required: exactly one of set, photo_ids\n"),
            "{listed}"
        );
    }

    #[test]
    fn a_tool_without_arguments_takes_an_empty_object() {
        let cleaned = grammar_schema(&json!({"type": "object", "$schema": "x"}));
        assert_eq!(
            cleaned,
            json!({"type": "object", "properties": {}, "additionalProperties": false})
        );
    }

    #[test]
    fn the_turn_schema_names_every_tool_under_call() {
        let schema = turn_schema(&[search_tool(), tool("get_status", json!({"type": "object"}))]);
        let call = &schema["anyOf"][0]["properties"]["call"]["anyOf"];
        assert_eq!(call[0]["required"], json!(["search_photos"]));
        assert_eq!(call[1]["required"], json!(["get_status"]));
        assert_eq!(schema["anyOf"][1]["required"], json!(["analysis", "reply"]));
    }

    #[test]
    fn without_tools_the_model_can_only_reply() {
        let schema = turn_schema(&[]);
        assert_eq!(schema["required"], json!(["analysis", "reply"]));
    }

    #[test]
    fn a_shared_argument_is_described_once() {
        let listed = describe_tools(&[search_tool(), search_tool()]);
        assert_eq!(listed.matches("in plain words").count(), 1, "{listed}");
        assert_eq!(listed.matches("- query (string, required)").count(), 2);
    }

    #[test]
    fn the_tool_list_says_what_each_argument_is() {
        let listed = describe_tools(&[search_tool()]);
        assert!(
            listed.contains("## search_photos\nDoes search_photos."),
            "{listed}"
        );
        assert!(
            listed.contains("- query (string, required): What to look for, in plain words."),
            "{listed}"
        );
        assert!(listed.contains("- limit (integer)\n"), "{listed}");
        assert!(
            listed.contains("- sort (\"relevance\" | \"aesthetic\"): Order."),
            "{listed}"
        );
        assert!(
            listed.contains("- within (object with set): Which photos."),
            "{listed}"
        );
    }

    fn request(tools: Vec<ToolSpec>) -> ChatRequest {
        ChatRequest {
            tools,
            max_tokens: 100,
            ..ChatRequest::default()
        }
    }

    #[test]
    fn a_call_drops_null_arguments_and_gets_an_id() {
        let turn = parse_turn(
            r#"{"analysis": "Search first.", "call": {"search_photos": {"query": "dog", "limit": null}}}"#,
            false,
            &request(vec![search_tool()]),
        )
        .unwrap();
        assert_eq!(turn.reasoning.as_deref(), Some("Search first."));
        assert_eq!(
            turn.tool_calls,
            vec![ToolCall {
                id: "call_1".into(),
                name: "search_photos".into(),
                arguments: json!({"query": "dog"}),
            }]
        );
    }

    #[test]
    fn a_reply_is_text() {
        let turn = parse_turn(
            r#"{"analysis": "", "reply": "Done."}"#,
            false,
            &request(Vec::new()),
        )
        .unwrap();
        assert_eq!(turn.text, "Done.");
        assert!(turn.reasoning.is_none());
        assert!(turn.tool_calls.is_empty());
    }

    #[test]
    fn a_cut_off_answer_says_it_ran_out() {
        let err = parse_turn(r#"{"analysis": "Let me"#, true, &request(Vec::new())).unwrap_err();
        assert!(err.contains("ran out of room"), "{err}");
    }

    #[test]
    fn the_history_is_rendered_back_as_the_json_the_model_writes() {
        let call = ToolCall {
            id: "call_1".into(),
            name: "search_photos".into(),
            arguments: json!({"query": "dog"}),
        };
        let messages = vec![
            ChatMessage::user_text("Find dogs."),
            ChatMessage {
                role: ChatRole::Assistant,
                parts: vec![
                    ChatPart::Reasoning("Search.".into()),
                    ChatPart::ToolCall(call),
                ],
            },
            ChatMessage {
                role: ChatRole::User,
                parts: vec![ChatPart::ToolResult(ToolResult {
                    call_id: "call_1".into(),
                    name: "search_photos".into(),
                    text: "{\n  \"set\": \"s1\",\n  \"count\": 3\n}".into(),
                    images: Vec::new(),
                    is_error: false,
                })],
            },
        ];
        let rendered = render_messages(&messages, true).unwrap();
        assert_eq!(rendered[0].text, "Find dogs.");
        assert_eq!(
            rendered[1].text,
            r#"{"analysis":"Search.","call":{"search_photos":{"query":"dog"}}}"#
        );
        assert_eq!(rendered[1].role, LocalChatRole::Assistant);
        // Key order depends on serde_json's features; the indentation must
        // be gone either way.
        assert!(
            rendered[2].text.starts_with("Result of search_photos:\n{"),
            "{}",
            rendered[2].text
        );
        assert!(!rendered[2].text.contains("  "), "{}", rendered[2].text);
    }

    fn png() -> Vec<u8> {
        let src = image::RgbImage::from_pixel(4, 4, image::Rgb([200, 10, 10]));
        let mut buf = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(src)
            .write_to(&mut buf, image::ImageFormat::Png)
            .unwrap();
        buf.into_inner()
    }

    fn photo_message() -> ChatMessage {
        ChatMessage {
            role: ChatRole::User,
            parts: vec![ChatPart::Image(ChatImage {
                data: png(),
                mime_type: "image/png".into(),
            })],
        }
    }

    #[test]
    fn only_the_last_photos_stay_pixels() {
        let messages = vec![photo_message(), photo_message(), photo_message()];
        let rendered = render_messages(&messages, true).unwrap();
        assert!(rendered[0].images.is_empty());
        assert_eq!(rendered[0].text, "[a photo shown earlier]");
        assert_eq!(rendered[1].images.len(), 1);
        assert_eq!(rendered[2].images.len(), 1);
    }

    #[test]
    fn a_text_model_is_told_it_cannot_see() {
        let rendered = render_messages(&[photo_message()], false).unwrap();
        assert!(rendered[0].images.is_empty());
        assert_eq!(rendered[0].text, "[a photo this model cannot see]");
    }

    fn stream_all(chunks: &[&str]) -> (String, String) {
        let mut stream = TurnStream::default();
        let (mut reasoning, mut text) = (String::new(), String::new());
        for chunk in chunks {
            for delta in stream.feed(chunk) {
                match delta {
                    ChatDelta::Reasoning(r) => reasoning.push_str(&r),
                    ChatDelta::Text(t) => text.push_str(&t),
                }
            }
        }
        (reasoning, text)
    }

    #[test]
    fn the_stream_yields_analysis_and_reply_whatever_the_chunking() {
        let answer = r#"{"analysis": "Say \"hi\".", "reply": "Hallo\nWelt ä😀"}"#;
        let expected = ("Say \"hi\".".to_string(), "Hallo\nWelt ä😀".to_string());
        assert_eq!(stream_all(&[answer]), expected);
        let chars: Vec<String> = answer.chars().map(String::from).collect();
        let refs: Vec<&str> = chars.iter().map(String::as_str).collect();
        assert_eq!(stream_all(&refs), expected);
    }

    #[test]
    fn the_stream_skips_strings_inside_a_call() {
        let answer =
            r#"{"analysis": "Look.", "call": {"search_photos": {"query": "reply", "reply": "x"}}}"#;
        assert_eq!(stream_all(&[answer]), ("Look.".to_string(), String::new()));
    }

    /// Answers every chat with `answer`, streamed a character at a time, and
    /// keeps the request it got.
    struct FakeEngine {
        answer: String,
        seen: Mutex<Option<LocalChatRequest>>,
    }

    #[async_trait]
    impl LocalEngine for FakeEngine {
        async fn generate(&self, _: Vec<LocalRequest>) -> Vec<Result<LocalResponse, String>> {
            Vec::new()
        }
        fn supports_vision(&self) -> bool {
            true
        }
        fn model_name(&self) -> String {
            "fake".into()
        }
        async fn chat(
            &self,
            request: LocalChatRequest,
            deltas: Option<TextDeltas>,
        ) -> Result<LocalResponse, String> {
            *self.seen.lock().unwrap() = Some(request);
            if let Some(deltas) = deltas {
                for c in self.answer.chars() {
                    let _ = deltas.send(c.to_string());
                }
            }
            Ok(LocalResponse {
                text: self.answer.clone(),
                prompt_tokens: 42,
                completion_tokens: 7,
                prefix_reused: false,
            })
        }
    }

    #[tokio::test]
    async fn a_turn_streams_its_reply_and_constrains_the_engine() {
        let engine = Arc::new(FakeEngine {
            answer: r#"{"analysis": "Easy.", "reply": "Hello there."}"#.into(),
            seen: Mutex::new(None),
        });
        let model = LocalChatModel::new(engine.clone());
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut request = request(vec![search_tool()]);
        request.system = "You help with photos.".into();
        request.messages = vec![ChatMessage::user_text("Hi")];

        let turn = model.complete(&request, Some(tx)).await.unwrap();
        assert_eq!(turn.text, "Hello there.");
        assert_eq!((turn.input_tokens, turn.output_tokens), (42, 7));

        let mut streamed = String::new();
        while let Ok(delta) = rx.try_recv() {
            if let ChatDelta::Text(t) = delta {
                streamed.push_str(&t);
            }
        }
        assert_eq!(streamed, "Hello there.");

        let seen = engine.seen.lock().unwrap().take().unwrap();
        assert_eq!(seen.messages[0].role, LocalChatRole::System);
        assert!(seen.messages[0].text.starts_with("You help with photos."));
        assert!(seen.messages[0].text.contains("## search_photos"));
        assert_eq!(seen.messages[1].text, "Hi");
        assert!(seen.schema.unwrap()["anyOf"].is_array());
    }
}
