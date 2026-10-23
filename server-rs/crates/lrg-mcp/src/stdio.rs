//! `geniusai-server mcp`: a stdio ↔ Streamable HTTP proxy, for MCP clients
//! that only start local servers as a command — Claude Desktop's config file
//! has no URL form. It forwards each JSON-RPC message from stdin to the
//! running backend's `/mcp`, and writes every message that comes back to
//! stdout, one per line.
//!
//! It keeps nothing but the session. When the backend restarts underneath it
//! (the session is unknown: HTTP 404), it replays the client's `initialize`
//! to open a fresh session and retries, so the client does not notice.
//!
//! Nothing here may print to stdout except protocol messages; diagnostics go
//! to stderr, which MCP clients keep as the server's log.

use std::sync::Arc;

use futures_util::StreamExt;
use reqwest::header::{ACCEPT, CONTENT_TYPE};
use reqwest::StatusCode;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{Mutex, RwLock};

const SESSION_HEADER: &str = "mcp-session-id";
const VERSION_HEADER: &str = "mcp-protocol-version";

const NOT_RUNNING: &str = "The LrGeniusAI backend is not running. Open Lightroom Classic with the \
                           LrGeniusAI plugin enabled (it starts the backend), then try again.";

/// Where answers to one forwarded message go.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Sink {
    Client,
    /// The replayed `initialize` after a backend restart: the client already
    /// has an answer to its own.
    Discard,
}

enum Outcome {
    Done,
    /// The backend did not know the session this request carried.
    SessionGone(Option<String>),
}

struct Proxy {
    url: String,
    /// The agent token, sent as a bearer token on every request.
    token: Option<String>,
    http: reqwest::Client,
    session: RwLock<Option<String>>,
    protocol_version: RwLock<Option<String>>,
    initialize: RwLock<Option<Value>>,
    reinit: Mutex<()>,
    out: Mutex<tokio::io::Stdout>,
}

/// Runs the proxy until stdin closes. `token` is the agent token from the
/// Plug-in Manager.
pub async fn run(url: String, token: Option<String>) -> std::io::Result<()> {
    let proxy = Arc::new(Proxy {
        url,
        token: token
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty()),
        http: reqwest::Client::builder()
            // Tool calls can run for minutes (analyzing photos); only the
            // connection itself should fail fast.
            .connect_timeout(std::time::Duration::from_secs(5))
            .build()
            .map_err(std::io::Error::other)?,
        session: RwLock::new(None),
        protocol_version: RwLock::new(None),
        initialize: RwLock::new(None),
        reinit: Mutex::new(()),
        out: Mutex::new(tokio::io::stdout()),
    });

    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let mut tasks = tokio::task::JoinSet::new();
    while let Some(line) = lines.next_line().await? {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let message: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("lrgenius mcp: ignoring a line that is not JSON: {e}");
                continue;
            }
        };
        if message["method"] == "initialize" {
            // Finishes before anything else is sent: the session it opens
            // goes on every later request.
            *proxy.initialize.write().await = Some(message.clone());
            proxy.forward(message).await;
        } else {
            let proxy = proxy.clone();
            tasks.spawn(async move { proxy.forward(message).await });
        }
        while tasks.try_join_next().is_some() {}
    }
    // The client closed stdin: it wants no more answers.
    tasks.abort_all();
    proxy.close_session().await;
    Ok(())
}

impl Proxy {
    async fn forward(&self, message: Value) {
        let id = message.get("id").cloned();
        let is_request = message.get("method").is_some() && id.is_some();
        let outcome = match self.post(&message, Sink::Client).await {
            Ok(Outcome::SessionGone(used)) => match self.reinitialize(used).await {
                Ok(()) => self.post(&message, Sink::Client).await,
                Err(e) => Err(e),
            },
            other => other,
        };
        match outcome {
            Ok(_) => {}
            Err(message) => {
                eprintln!("lrgenius mcp: {message}");
                if is_request {
                    self.emit_value(json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": {"code": -32603, "message": message},
                    }))
                    .await;
                }
            }
        }
    }

    /// Opens a new session by replaying the client's `initialize`, unless
    /// another request already did since `failed_session` was rejected.
    async fn reinitialize(&self, failed_session: Option<String>) -> Result<(), String> {
        let _guard = self.reinit.lock().await;
        if *self.session.read().await != failed_session {
            return Ok(());
        }
        let Some(init) = self.initialize.read().await.clone() else {
            return Err("The backend restarted before the session was initialized.".into());
        };
        eprintln!("lrgenius mcp: the backend restarted; opening a new session");
        *self.session.write().await = None;
        self.post(&init, Sink::Discard).await?;
        self.post(
            &json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
            Sink::Discard,
        )
        .await?;
        Ok(())
    }

    async fn post(&self, message: &Value, sink: Sink) -> Result<Outcome, String> {
        let session = self.session.read().await.clone();
        let mut request = self
            .http
            .post(&self.url)
            .header(CONTENT_TYPE, "application/json")
            .header(ACCEPT, "application/json, text/event-stream")
            .body(message.to_string());
        if let Some(s) = &session {
            request = request.header(SESSION_HEADER, s);
        }
        if let Some(token) = &self.token {
            request = request.bearer_auth(token);
        }
        if let Some(v) = self.protocol_version.read().await.clone() {
            request = request.header(VERSION_HEADER, v);
        }
        let response = request.send().await.map_err(|e| {
            if e.is_connect() {
                NOT_RUNNING.to_string()
            } else {
                format!("Could not reach the LrGeniusAI backend: {e}")
            }
        })?;

        let status = response.status();
        if status == StatusCode::NOT_FOUND && session.is_some() && message["method"] != "initialize"
        {
            return Ok(Outcome::SessionGone(session));
        }
        if let Some(s) = response
            .headers()
            .get(SESSION_HEADER)
            .and_then(|v| v.to_str().ok())
        {
            *self.session.write().await = Some(s.to_string());
        }
        if status == StatusCode::ACCEPTED {
            return Ok(Outcome::Done);
        }
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            // The backend explains refusals (a missing agent token) in a
            // JSON-RPC error; pass that sentence on, not the raw body.
            let explained = serde_json::from_str::<Value>(&body)
                .ok()
                .and_then(|v| v["error"]["message"].as_str().map(str::to_string));
            return Err(explained.unwrap_or_else(|| {
                format!(
                    "The LrGeniusAI backend answered HTTP {status}: {}",
                    body.trim()
                )
            }));
        }

        let event_stream = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|ct| ct.starts_with("text/event-stream"));
        if !event_stream {
            let body = response.text().await.map_err(|e| e.to_string())?;
            if !body.trim().is_empty() {
                self.emit(&body, sink).await;
            }
            return Ok(Outcome::Done);
        }

        // Server-sent events: `data:` lines, an empty line ends an event.
        let mut stream = response.bytes_stream();
        let mut pending: Vec<u8> = Vec::new();
        let mut data = String::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| format!("The event stream broke off: {e}"))?;
            pending.extend_from_slice(&chunk);
            while let Some(pos) = pending.iter().position(|b| *b == b'\n') {
                let raw: Vec<u8> = pending.drain(..=pos).collect();
                let line = String::from_utf8_lossy(&raw);
                let line = line.trim_end_matches(['\r', '\n']);
                if line.is_empty() {
                    if !data.is_empty() {
                        self.emit(&data, sink).await;
                        data.clear();
                    }
                } else if let Some(rest) = line.strip_prefix("data:") {
                    if !data.is_empty() {
                        data.push('\n');
                    }
                    data.push_str(rest.strip_prefix(' ').unwrap_or(rest));
                }
                // `event:`, `id:`, `retry:` and `:` comments need no action.
            }
        }
        if !data.is_empty() {
            self.emit(&data, sink).await;
        }
        Ok(Outcome::Done)
    }

    async fn emit(&self, text: &str, sink: Sink) {
        let value: Value = match serde_json::from_str(text) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("lrgenius mcp: the backend sent something that is not JSON: {e}");
                return;
            }
        };
        // Remember the negotiated version: later requests must carry it.
        if let Some(version) = value["result"]["protocolVersion"].as_str() {
            *self.protocol_version.write().await = Some(version.to_string());
        }
        if sink == Sink::Client {
            self.emit_value(value).await;
        }
    }

    async fn emit_value(&self, value: Value) {
        let mut line = value.to_string();
        line.push('\n');
        let mut out = self.out.lock().await;
        if out.write_all(line.as_bytes()).await.is_ok() {
            let _ = out.flush().await;
        }
    }

    async fn close_session(&self) {
        let Some(session) = self.session.read().await.clone() else {
            return;
        };
        let mut request = self.http.delete(&self.url).header(SESSION_HEADER, session);
        if let Some(token) = &self.token {
            request = request.bearer_auth(token);
        }
        let _ = request.send().await;
    }
}
