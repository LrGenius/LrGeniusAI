//! One conversation: its history, its connection to the tools, and the loop
//! that lets the model work until it has an answer for the user.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use lrg_api::agent_bridge::CallOptions;
use lrg_api::llm_engine::EngineOverrides;
use lrg_api::routes::llm::engine_for_request;
use lrg_api::state::AppState;
use lrg_mcp::api::Api;
use lrg_mcp::sets::ResultSets;
use lrg_mcp::Shared;
use lrg_providers::chat::{
    ChatDelta, ChatMessage, ChatModel, ChatPart, ChatRequest, ChatRole, ToolCall, ToolResult,
};
use lrg_providers::local_chat::LocalChatModel;

use crate::events::{ChatEvent, DeltaKind, EventLog};
use crate::mcp::Connection;
use crate::permissions::{AskReason, PermissionStore, Permissions, ToolClass, ToolRule, Verdict};
use crate::prompt::system_prompt;

/// Model turns per user message before the chat gives up and asks. A task
/// that needs more is rare; a model going in circles is not.
const MAX_STEPS: usize = 12;

/// Room for one answer. A tool call is a few dozen tokens; this is for the
/// replies.
const MAX_ANSWER_TOKENS: u32 = 1536;

/// The window assumed for an engine that does not report one (MLX sizes its
/// cache from the prompt). Generous, and still far below what the models in
/// the catalog take.
const ASSUMED_WINDOW: u32 = 32_768;

/// What the system prompt and the tool list cost before the first turn
/// reports the real number: about 3,000 tokens with every tool offered.
const ASSUMED_OVERHEAD: usize = 4_000;

/// Rough, on the safe side for English and JSON alike.
const CHARS_PER_TOKEN: usize = 3;
const TOKENS_PER_IMAGE: usize = 600;

/// What a dropped tool result says instead.
const OMITTED: &str = "[result omitted to make room — run the tool again if you need it]";

/// Added to the history where the user stopped the chat. Without it the
/// request left unanswered is merged with the next message, and the model
/// answers both (seen with Gemma 4 E4B).
const STOPPED: &str =
    "[The user stopped this before it was finished. Do not go on with it unless they ask again.]";

/// Where the model for the next turn comes from. Resolved per turn rather
/// than held: the engine slots unload an idle model and swap it when indexing
/// asks for another, and a session holding its own reference would keep the
/// old one alive — for llama.cpp, it would make the swap fail outright.
#[async_trait]
pub trait ModelSource: Send + Sync {
    async fn model(&self) -> Result<Arc<dyn ChatModel>, String>;

    /// The model as the page names it, before it has loaded.
    fn label(&self) -> String;
}

/// One of the two local engines, chosen the way an indexing request chooses
/// it.
pub struct LocalModels {
    pub state: Arc<AppState>,
    pub provider: String,
    pub model: String,
    pub overrides: EngineOverrides,
}

#[async_trait]
impl ModelSource for LocalModels {
    async fn model(&self) -> Result<Arc<dyn ChatModel>, String> {
        let engine = engine_for_request(&self.state, &self.provider, &self.model, self.overrides)
            .await?
            .ok_or_else(|| format!("\"{}\" is not a local AI model provider.", self.provider))?;
        Ok(Arc::new(LocalChatModel::new(engine)))
    }

    fn label(&self) -> String {
        self.model.clone()
    }
}

/// The user's answer to a call that waits for approval.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum Approval {
    Once,
    /// This tool, for the rest of this chat.
    Chat,
    /// This tool, from now on: sets its rule to allow.
    Always,
    Deny {
        /// Passed on to the model, so it can do what the user wants instead.
        #[serde(default)]
        note: Option<String>,
    },
}

#[derive(Default)]
struct Inner {
    messages: Vec<ChatMessage>,
    allowed_in_chat: HashSet<String>,
    pending: HashMap<String, oneshot::Sender<Approval>>,
    running: Option<CancellationToken>,
    /// What the system prompt and tools cost, learned from the first turn.
    overhead: Option<usize>,
}

pub struct ChatSession {
    pub id: String,
    token: String,
    state: Arc<AppState>,
    shared: Arc<Shared>,
    connection: Connection,
    models: Box<dyn ModelSource>,
    permissions: Arc<PermissionStore>,
    system: String,
    pub events: EventLog,
    inner: Mutex<Inner>,
    last_seen: Mutex<Instant>,
}

impl ChatSession {
    /// A new conversation with its own tool connection and result sets.
    ///
    /// # Errors
    /// Reports a tool server that would not start.
    pub async fn open(
        state: Arc<AppState>,
        api: axum::Router,
        models: Box<dyn ModelSource>,
        permissions: Arc<PermissionStore>,
    ) -> Result<Arc<Self>, String> {
        let shared = Arc::new(Shared {
            state: state.clone(),
            api: Api::new(api),
            sets: ResultSets::new(),
        });
        let connection = Connection::open(shared.clone()).await?;
        let today = chrono::Utc::now().date_naive().to_string();
        let system = system_prompt(&connection.instructions, &today);
        Ok(Arc::new(ChatSession {
            id: uuid::Uuid::new_v4().simple().to_string(),
            token: format!(
                "{}{}",
                uuid::Uuid::new_v4().simple(),
                uuid::Uuid::new_v4().simple()
            ),
            state,
            shared,
            connection,
            models,
            permissions,
            system,
            events: EventLog::default(),
            inner: Mutex::default(),
            last_seen: Mutex::new(Instant::now()),
        }))
    }

    /// The secret the page sends with every request.
    #[must_use]
    pub fn token(&self) -> &str {
        &self.token
    }

    /// Compared in constant time: the token is the only thing between a
    /// page in some other tab and the user's catalog.
    #[must_use]
    pub fn token_matches(&self, presented: &str) -> bool {
        let (a, b) = (self.token.as_bytes(), presented.as_bytes());
        a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
    }

    pub fn touch(&self) {
        *self.last_seen.lock().unwrap() = Instant::now();
    }

    #[must_use]
    pub fn idle_for(&self) -> Duration {
        self.last_seen.lock().unwrap().elapsed()
    }

    #[must_use]
    pub fn is_busy(&self) -> bool {
        self.inner.lock().unwrap().running.is_some()
    }

    fn index_dir(&self) -> Option<std::path::PathBuf> {
        self.state.lance_root()
    }

    #[must_use]
    pub fn permissions(&self) -> Permissions {
        self.permissions.get(self.index_dir().as_deref())
    }

    /// # Errors
    /// Reports settings that could not be written; they still apply until
    /// the backend restarts.
    pub fn set_permissions(&self, permissions: Permissions) -> Result<bool, String> {
        self.permissions
            .set(self.index_dir().as_deref(), permissions)
    }

    /// What the page needs to draw itself.
    #[must_use]
    pub fn info(&self) -> Value {
        let permissions = self.permissions();
        let tools: Vec<Value> = self
            .connection
            .tools
            .iter()
            .map(|t| {
                json!({
                    "name": t.spec.name,
                    "title": t.title,
                    "class": t.class,
                    "rule": permissions.tools.get(&t.spec.name),
                })
            })
            .collect();
        let bridge = self.state.agent_bridge.plugin_info();
        json!({
            "session_id": self.id,
            "model": self.models.label(),
            "busy": self.is_busy(),
            "tools": tools,
            "permissions": permissions,
            "permissions_saved": self.index_dir().is_some(),
            "lightroom": bridge.as_ref().map(|info| json!({
                "catalog_name": info.catalog_name,
                "may_change_photos": info.allow_changes,
            })),
        })
    }

    /// Load the model in the background while the user types their first
    /// message. A model that cannot load is reported now rather than after
    /// the user has written something.
    pub fn warm_up(self: &Arc<Self>) {
        let session = self.clone();
        tokio::spawn(async move {
            if let Err(message) = session.models.model().await {
                session.events.emit(ChatEvent::Error { message });
            }
        });
    }

    /// Start working on a user message.
    ///
    /// # Errors
    /// Refuses while the previous message is still being worked on.
    pub fn send(self: &Arc<Self>, text: String) -> Result<(), String> {
        let cancel = CancellationToken::new();
        {
            let mut inner = self.inner.lock().unwrap();
            if inner.running.is_some() {
                return Err(
                    "Still working on the last message. Wait for it, or stop it first.".into(),
                );
            }
            inner.running = Some(cancel.clone());
            inner.messages.push(ChatMessage::user_text(text.clone()));
        }
        self.events.emit(ChatEvent::UserMessage { text });
        tokio::spawn(self.clone().run(cancel));
        Ok(())
    }

    /// # Errors
    /// Reports a call that is not waiting (any more).
    pub fn approve(&self, call_id: &str, approval: Approval) -> Result<(), String> {
        let sender = self.inner.lock().unwrap().pending.remove(call_id);
        match sender {
            Some(sender) => {
                let _ = sender.send(approval);
                Ok(())
            }
            None => Err(format!("No call \"{call_id}\" is waiting for approval.")),
        }
    }

    /// Stop after the step in progress. A tool call already running in
    /// Lightroom is not interrupted — its result is recorded — but nothing
    /// new starts, and calls waiting for approval are denied.
    pub fn stop(&self) {
        if let Some(cancel) = &self.inner.lock().unwrap().running {
            cancel.cancel();
        }
    }

    async fn run(self: Arc<Self>, cancel: CancellationToken) {
        self.events.emit(ChatEvent::Busy { busy: true });
        if let Err(message) = self.work(&cancel).await {
            self.events.emit(ChatEvent::Error { message });
        }
        if cancel.is_cancelled() {
            let mut inner = self.inner.lock().unwrap();
            // Stopped after the model's last word, nothing was left unfinished.
            if let Some(last) = inner
                .messages
                .last_mut()
                .filter(|m| m.role == ChatRole::User)
            {
                last.parts.push(ChatPart::Text(STOPPED.into()));
            }
            drop(inner);
            self.events.emit(ChatEvent::Stopped);
        }
        {
            let mut inner = self.inner.lock().unwrap();
            inner.running = None;
            inner.pending.clear();
        }
        self.events.emit(ChatEvent::Busy { busy: false });
    }

    async fn work(&self, cancel: &CancellationToken) -> Result<(), String> {
        let mut made: Vec<(String, Value)> = Vec::new();
        for _ in 0..MAX_STEPS {
            if cancel.is_cancelled() {
                return Ok(());
            }
            let model = self.models.model().await?;
            let permissions = self.permissions();
            let tools = self
                .connection
                .tools
                .iter()
                .filter(|t| permissions.offers(&t.spec.name))
                .map(|t| t.spec.clone())
                .collect();
            let (messages, sent_estimate) = self.fitted_history(model.context_window())?;
            let request = ChatRequest {
                system: self.system.clone(),
                messages,
                tools,
                max_tokens: MAX_ANSWER_TOKENS,
                temperature: 0.0,
            };

            let (delta_tx, mut delta_rx) = tokio::sync::mpsc::unbounded_channel::<ChatDelta>();
            let forward = async {
                while let Some(delta) = delta_rx.recv().await {
                    let (kind, text) = match delta {
                        ChatDelta::Reasoning(text) => (DeltaKind::Reasoning, text),
                        ChatDelta::Text(text) => (DeltaKind::Text, text),
                    };
                    self.events.emit(ChatEvent::Delta { kind, text });
                }
            };
            let answer =
                async { tokio::join!(model.complete(&request, Some(delta_tx)), forward).0 };
            let turn = tokio::select! {
                biased;
                () = cancel.cancelled() => return Ok(()),
                turn = answer => turn?,
            };

            {
                let mut inner = self.inner.lock().unwrap();
                if inner.overhead.is_none() {
                    inner.overhead =
                        Some((turn.input_tokens as usize).saturating_sub(sent_estimate));
                }
                inner.messages.push(turn.to_message());
            }
            self.events.emit(ChatEvent::AssistantTurn {
                reasoning: turn.reasoning.clone(),
                text: turn.text.clone(),
            });
            self.events.emit(ChatEvent::Usage {
                prompt_tokens: turn.input_tokens,
                context_window: model.context_window(),
            });
            if turn.tool_calls.is_empty() {
                return Ok(());
            }

            // Every call gets a result, even one that never ran: a history
            // with an unanswered call is one no model API accepts.
            let mut results = Vec::with_capacity(turn.tool_calls.len());
            for call in &turn.tool_calls {
                let result = self.handle_call(call, cancel, &mut made).await;
                results.push(ChatPart::ToolResult(result));
            }
            self.inner.lock().unwrap().messages.push(ChatMessage {
                role: ChatRole::User,
                parts: results,
            });
        }
        self.events.emit(ChatEvent::Warning {
            message: format!(
                "Stopped after {MAX_STEPS} steps without an answer. Tell the assistant how to go \
                 on, or ask for less at once."
            ),
        });
        Ok(())
    }

    async fn handle_call(
        &self,
        call: &ToolCall,
        cancel: &CancellationToken,
        made: &mut Vec<(String, Value)>,
    ) -> ToolResult {
        let Some(tool) = self.connection.tool(&call.name) else {
            return self.skip(call, "unknown tool", "There is no tool by that name.");
        };
        let photos = if tool.class == ToolClass::Destructive {
            self.count_photos(&call.arguments).await
        } else {
            None
        };
        self.events.emit(ChatEvent::ToolCall {
            call_id: call.id.clone(),
            tool: call.name.clone(),
            title: tool.title.clone(),
            class: tool.class,
            arguments: call.arguments.clone(),
            photos,
        });

        // The loop seen with Gemma 4 12B: the same search, again and again.
        let key = (call.name.clone(), call.arguments.clone());
        if made.contains(&key) {
            return self.skip(
                call,
                "repeated call",
                "You already made exactly this call for this message, and its result is above. \
                 Use it, try something different, or reply to the user.",
            );
        }
        made.push(key);

        let allowed_in_chat = self
            .inner
            .lock()
            .unwrap()
            .allowed_in_chat
            .contains(&call.name);
        match self
            .permissions()
            .verdict(&call.name, tool.class, allowed_in_chat, photos)
        {
            Verdict::Run => {}
            Verdict::Blocked => {
                return self.skip(
                    call,
                    "turned off",
                    "The user has turned this tool off for the chat.",
                );
            }
            Verdict::Ask(reason) => {
                if let Some(refused) = self.ask(call, reason, cancel).await {
                    return refused;
                }
            }
        }
        if cancel.is_cancelled() {
            return self.skip(
                call,
                "stopped",
                "The user stopped the chat before this ran.",
            );
        }

        self.events.emit(ChatEvent::ToolRunning {
            call_id: call.id.clone(),
        });
        let result = self.connection.call(call).await;
        let (set_id, count, warnings) = summarize(&result.text);
        self.events.emit(ChatEvent::ToolDone {
            call_id: call.id.clone(),
            ok: !result.is_error,
            text: result.text.clone(),
            set_id,
            count,
            warnings,
            images: result.images.len(),
        });
        result
    }

    /// Wait for the user's answer. `None` means go ahead; otherwise the
    /// result to hand the model instead.
    async fn ask(
        &self,
        call: &ToolCall,
        reason: AskReason,
        cancel: &CancellationToken,
    ) -> Option<ToolResult> {
        let (sender, receiver) = oneshot::channel();
        self.inner
            .lock()
            .unwrap()
            .pending
            .insert(call.id.clone(), sender);
        self.events.emit(ChatEvent::ApprovalNeeded {
            call_id: call.id.clone(),
            reason,
        });
        let approval = tokio::select! {
            approval = receiver => approval.ok(),
            () = cancel.cancelled() => None,
        };
        self.inner.lock().unwrap().pending.remove(&call.id);
        match approval {
            Some(Approval::Once) => None,
            Some(Approval::Chat) => {
                self.inner
                    .lock()
                    .unwrap()
                    .allowed_in_chat
                    .insert(call.name.clone());
                None
            }
            Some(Approval::Always) => {
                let mut permissions = self.permissions();
                permissions.tools.insert(call.name.clone(), ToolRule::Allow);
                match self.set_permissions(permissions) {
                    Ok(true) => {}
                    Ok(false) => self.events.emit(ChatEvent::Warning {
                        message: "This choice applies until LrGeniusAI restarts: the backend \
                                  does not know where its index is yet, so it cannot save it."
                            .into(),
                    }),
                    Err(message) => self.events.emit(ChatEvent::Warning { message }),
                }
                None
            }
            Some(Approval::Deny { note }) => {
                let mut text = "The user did not allow this call.".to_string();
                if let Some(note) = note.filter(|n| !n.trim().is_empty()) {
                    text.push_str(&format!(" They said: {}", note.trim()));
                }
                Some(self.skip(call, "not allowed", &text))
            }
            None => Some(self.skip(
                call,
                "stopped",
                "The user stopped the chat before allowing this.",
            )),
        }
    }

    /// A call that does not run: the page is told why, the model reads `text`.
    fn skip(&self, call: &ToolCall, reason: &str, text: &str) -> ToolResult {
        self.events.emit(ChatEvent::ToolSkipped {
            call_id: call.id.clone(),
            reason: reason.to_string(),
        });
        ToolResult {
            call_id: call.id.clone(),
            name: call.name.clone(),
            text: text.to_string(),
            images: Vec::new(),
            is_error: true,
        }
    }

    /// How many photos a call that changes photos is aimed at, for the bulk
    /// limit. `None` when it cannot be told beforehand.
    async fn count_photos(&self, arguments: &Value) -> Option<usize> {
        if let Some(ids) = arguments.get("photo_ids").and_then(Value::as_array) {
            return Some(ids.len());
        }
        if arguments.get("photo_id").is_some() {
            return Some(1);
        }
        let set = arguments.get("set").and_then(Value::as_str)?;
        if let scope @ ("selection" | "view") = set {
            let reply = self
                .state
                .agent_bridge
                .call(
                    "get_photos",
                    json!({"scope": scope, "limit": lrg_mcp::MAX_PHOTOS_PER_CALL}),
                    CallOptions::read(Duration::from_secs(30)),
                )
                .await
                .ok()?;
            return reply.result["photo_ids"].as_array().map(Vec::len);
        }
        self.shared.sets.get(set).map(|s| s.photo_ids.len())
    }

    /// The history as it fits the model's window, and its estimated size.
    /// The oldest tool results give up their text and photos first — the
    /// model can run a tool again — and the last exchange is never touched.
    fn fitted_history(&self, window: Option<u32>) -> Result<(Vec<ChatMessage>, usize), String> {
        let inner = self.inner.lock().unwrap();
        let window = window.unwrap_or(ASSUMED_WINDOW) as usize;
        let overhead = inner.overhead.unwrap_or(ASSUMED_OVERHEAD);
        let budget = window.saturating_sub(overhead + MAX_ANSWER_TOKENS as usize + 256);
        fit(merge_adjacent(&inner.messages), budget).ok_or_else(|| {
            format!(
                "This conversation no longer fits the model's context window ({window} \
                 tokens). Start a new chat, or raise the context size in the plugin settings."
            )
        })
    }
}

/// Turns of the same speaker in a row, merged into one: a message sent after
/// a failed turn would otherwise follow another user turn, which some model
/// APIs refuse.
fn merge_adjacent(messages: &[ChatMessage]) -> Vec<ChatMessage> {
    let mut out: Vec<ChatMessage> = Vec::with_capacity(messages.len());
    for message in messages {
        match out.last_mut() {
            Some(last) if last.role == message.role => {
                last.parts.extend(message.parts.iter().cloned());
            }
            _ => out.push(message.clone()),
        }
    }
    out
}

fn estimate(messages: &[ChatMessage]) -> usize {
    messages
        .iter()
        .flat_map(|m| &m.parts)
        .map(|part| match part {
            ChatPart::Text(text) | ChatPart::Reasoning(text) => text.len() / CHARS_PER_TOKEN,
            ChatPart::Image(_) => TOKENS_PER_IMAGE,
            ChatPart::ToolCall(call) => call.arguments.to_string().len() / CHARS_PER_TOKEN + 10,
            ChatPart::ToolResult(result) => {
                result.text.len() / CHARS_PER_TOKEN + result.images.len() * TOKENS_PER_IMAGE
            }
        })
        .sum()
}

fn fit(mut messages: Vec<ChatMessage>, budget: usize) -> Option<(Vec<ChatMessage>, usize)> {
    let protected = messages.len().saturating_sub(2);
    loop {
        let size = estimate(&messages);
        if size <= budget {
            return Some((messages, size));
        }
        let shrinkable = messages[..protected]
            .iter_mut()
            .flat_map(|m| &mut m.parts)
            .find(|part| match part {
                ChatPart::ToolResult(result) => result.text != OMITTED || !result.images.is_empty(),
                ChatPart::Image(_) => true,
                _ => false,
            })?;
        match shrinkable {
            ChatPart::ToolResult(result) => {
                result.text = OMITTED.to_string();
                result.images.clear();
            }
            part => *part = ChatPart::Text("[a photo, omitted to make room]".into()),
        }
    }
}

/// The parts of a tool's JSON answer the page shows outside the text.
fn summarize(text: &str) -> (Option<String>, Option<u64>, Vec<String>) {
    let Ok(value) = serde_json::from_str::<Value>(text) else {
        return (None, None, Vec::new());
    };
    let set_id = value
        .get("set_id")
        .and_then(Value::as_str)
        .map(str::to_string);
    let count = value.get("count").and_then(Value::as_u64);
    let warnings = value
        .get("warnings")
        .and_then(Value::as_array)
        .map(|w| {
            w.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    (set_id, count, warnings)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result_message(text: &str) -> ChatMessage {
        ChatMessage {
            role: ChatRole::User,
            parts: vec![ChatPart::ToolResult(ToolResult {
                call_id: "c".into(),
                name: "t".into(),
                text: text.into(),
                images: Vec::new(),
                is_error: false,
            })],
        }
    }

    #[test]
    fn the_oldest_results_go_first_and_the_last_exchange_stays() {
        let big = "x".repeat(3000);
        let messages = vec![
            ChatMessage::user_text("find dogs"),
            result_message(&big),
            result_message(&big),
            ChatMessage::user_text("and cats"),
        ];
        let (fitted, size) = fit(messages, 1200).unwrap();
        let text_of = |m: &ChatMessage| match &m.parts[0] {
            ChatPart::ToolResult(r) => r.text.clone(),
            _ => String::new(),
        };
        assert_eq!(text_of(&fitted[1]), OMITTED);
        assert_eq!(text_of(&fitted[2]), big, "the last exchange is kept whole");
        assert!(size <= 1200);
    }

    #[test]
    fn a_history_that_cannot_fit_says_so() {
        let messages = vec![
            ChatMessage::user_text("x".repeat(9000)),
            ChatMessage::user_text("y"),
        ];
        assert!(fit(messages, 100).is_none());
    }

    #[test]
    fn turns_of_one_speaker_in_a_row_are_merged() {
        let merged = merge_adjacent(&[ChatMessage::user_text("a"), ChatMessage::user_text("b")]);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].parts.len(), 2);
    }

    #[test]
    fn a_set_answer_is_summarized_for_the_page() {
        let (set, count, warnings) =
            summarize(r#"{"set_id": "s2", "count": 14, "warnings": ["3 not indexed"]}"#);
        assert_eq!(set.as_deref(), Some("s2"));
        assert_eq!(count, Some(14));
        assert_eq!(warnings, vec!["3 not indexed".to_string()]);
        assert_eq!(summarize("not json"), (None, None, Vec::new()));
    }
}
