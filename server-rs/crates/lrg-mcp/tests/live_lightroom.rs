//! Live tests of the Lightroom half of LrGeniusAI, driven over MCP.
//!
//! They talk to a running backend's `/mcp` endpoint exactly as an AI agent
//! would, and through it to the plugin inside a running Lightroom. Ignored by
//! default; run them with Lightroom open (the agent bridge starts with it)
//! and — for the photo tests — a photo selected:
//!
//! ```text
//! cargo test -p lrg-mcp --test live_lightroom -- --ignored --test-threads=1
//! ```
//!
//! `LRGENIUS_MCP_TOKEN` must hold the agent token (Plug-in Manager >
//! LrGeniusAI > AI Agents > Copy token). `LRG_LIVE_MCP_URL` points elsewhere
//! than `http://127.0.0.1:19819/mcp`; `LRG_LIVE_WRITES=1` also runs the
//! plugin's catalog-writing tests (they put every change back, but use a test
//! catalog anyway).

use std::time::Duration;

use reqwest::header::{ACCEPT, CONTENT_TYPE};
use serde_json::{json, Value};

const SESSION_HEADER: &str = "mcp-session-id";

/// A minimal MCP client over Streamable HTTP: enough to initialize and call
/// tools. Requests are answered as JSON or as server-sent events; both are
/// handled.
struct LiveClient {
    url: String,
    token: String,
    http: reqwest::Client,
    session: Option<String>,
    version: String,
    next_id: u64,
}

impl LiveClient {
    async fn connect() -> Self {
        let url = std::env::var("LRG_LIVE_MCP_URL")
            .unwrap_or_else(|_| "http://127.0.0.1:19819/mcp".to_string());
        let token = std::env::var("LRGENIUS_MCP_TOKEN").unwrap_or_else(|_| {
            panic!(
                "set LRGENIUS_MCP_TOKEN to the agent token (Plug-in Manager > LrGeniusAI > AI \
                 Agents > Copy token)"
            )
        });
        let mut client = Self {
            url,
            token,
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(900))
                .build()
                .unwrap(),
            session: None,
            version: "2025-06-18".to_string(),
            next_id: 0,
        };
        let init = client
            .request(
                "initialize",
                json!({
                    "protocolVersion": client.version,
                    "capabilities": {},
                    "clientInfo": {"name": "lrgenius-live-tests", "version": "1"},
                }),
            )
            .await;
        if let Some(v) = init["protocolVersion"].as_str() {
            client.version = v.to_string();
        }
        client
            .send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
            .await;
        client
    }

    async fn send(&mut self, message: Value) -> Vec<Value> {
        let mut request = self
            .http
            .post(&self.url)
            .header(CONTENT_TYPE, "application/json")
            .header(ACCEPT, "application/json, text/event-stream")
            .header("mcp-protocol-version", &self.version)
            .bearer_auth(&self.token)
            .body(message.to_string());
        if let Some(s) = &self.session {
            request = request.header(SESSION_HEADER, s);
        }
        let response = request.send().await.unwrap_or_else(|e| {
            panic!(
                "cannot reach {} — is the LrGeniusAI backend running? ({e})",
                self.url
            )
        });
        if let Some(s) = response
            .headers()
            .get(SESSION_HEADER)
            .and_then(|v| v.to_str().ok())
        {
            self.session = Some(s.to_string());
        }
        let status = response.status();
        let event_stream = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|ct| ct.starts_with("text/event-stream"));
        let body = response.text().await.unwrap();
        assert!(status.is_success(), "HTTP {status}: {body}");
        if !event_stream {
            return if body.trim().is_empty() {
                vec![]
            } else {
                vec![serde_json::from_str(&body).unwrap()]
            };
        }
        let mut messages = Vec::new();
        let mut data = String::new();
        for line in body.lines().chain(std::iter::once("")) {
            if line.is_empty() {
                if !data.is_empty() {
                    messages.push(serde_json::from_str(&data).unwrap());
                    data.clear();
                }
            } else if let Some(rest) = line.strip_prefix("data:") {
                data.push_str(rest.strip_prefix(' ').unwrap_or(rest));
            }
        }
        messages
    }

    async fn request(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let id = self.next_id;
        let messages = self
            .send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
            .await;
        let answer = messages
            .into_iter()
            .find(|m| m["id"] == id)
            .unwrap_or_else(|| panic!("no answer to {method}"));
        assert!(answer.get("error").is_none(), "{method} failed: {answer}");
        answer["result"].clone()
    }

    /// Calls a tool; returns (is_error, text, the whole result).
    async fn tool(&mut self, name: &str, args: Value) -> (bool, String, Value) {
        let result = self
            .request("tools/call", json!({"name": name, "arguments": args}))
            .await;
        let text = result["content"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|c| c["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n");
        (result["isError"].as_bool().unwrap_or(false), text, result)
    }

    /// Calls a tool that must succeed with JSON.
    async fn tool_json(&mut self, name: &str, args: Value) -> Value {
        let (is_error, text, _) = self.tool(name, args).await;
        assert!(!is_error, "{name} failed: {text}");
        serde_json::from_str(&text).unwrap_or_else(|_| panic!("{name} did not answer JSON: {text}"))
    }
}

/// The status, after checking Lightroom is connected.
async fn connected() -> (LiveClient, Value) {
    let mut client = LiveClient::connect().await;
    let status = client.tool_json("get_status", json!({})).await;
    assert_eq!(
        status["lightroom"]["connected"], true,
        "Lightroom is not connected: open Lightroom with the LrGeniusAI plugin; its agent bridge \
         starts with it ({})",
        status["lightroom"]
    );
    (client, status)
}

/// The photo the photo tests use: the active one in Lightroom.
fn selected_photo(status: &Value) -> Option<String> {
    status["lightroom"]["selection"]["active_photo_id"]
        .as_str()
        .or_else(|| status["lightroom"]["selection"]["photo_ids"][0].as_str())
        .map(str::to_string)
}

#[tokio::test]
#[ignore = "needs Lightroom with the agent bridge running"]
async fn lightroom_is_connected_and_describes_itself() {
    let (_, status) = connected().await;
    let lr = &status["lightroom"];
    assert!(lr["catalog"]["path"]
        .as_str()
        .is_some_and(|p| !p.is_empty()));
    assert!(lr["lightroom_version"].is_string());
    println!(
        "Lightroom {} - catalog {} ({} photos) - changes allowed: {} - developer commands: {}",
        lr["lightroom_version"],
        lr["catalog"]["name"],
        lr["catalog"]["photo_count"],
        lr["allow_changes"],
        lr["developer_commands"]
    );
}

#[tokio::test]
#[ignore = "needs Lightroom with the agent bridge running"]
async fn the_plugins_own_tests_pass_inside_lightroom() {
    let (mut client, _) = connected().await;
    let writes = std::env::var("LRG_LIVE_WRITES").is_ok_and(|v| v == "1");
    let result = client
        .tool_json(
            "run_plugin_tests",
            json!({"include_catalog_writes": writes}),
        )
        .await;
    for r in result["results"].as_array().into_iter().flatten() {
        let verdict = if r["ok"] == true {
            "ok  "
        } else if r["skipped"].is_string() {
            "skip"
        } else {
            "FAIL"
        };
        println!(
            "{verdict} {} / {} {}",
            r["suite"].as_str().unwrap_or(""),
            r["name"].as_str().unwrap_or(""),
            r["error"].as_str().or(r["skipped"].as_str()).unwrap_or("")
        );
    }
    assert_eq!(
        result["failed"], 0,
        "plugin tests failed: {}",
        result["failures"]
    );
    assert!(result["passed"].as_u64().unwrap_or(0) > 0, "no test ran");
}

#[tokio::test]
#[ignore = "needs Lightroom with the agent bridge running and a photo selected"]
async fn the_selection_resolves_and_has_details() {
    let (mut client, status) = connected().await;
    if selected_photo(&status).is_none() {
        eprintln!("skipped: select a photo in Lightroom");
        return;
    }
    let set = client
        .tool_json(
            "make_set",
            json!({"set": "selection", "label": "live test selection"}),
        )
        .await;
    assert!(set["count"].as_u64().unwrap() >= 1);
    let details = client
        .tool_json("get_photo_details", json!({"set": set["set_id"]}))
        .await;
    let first = &details["photos"][0];
    assert!(
        first["lightroom"]["file_name"].is_string(),
        "Lightroom details missing: {first}"
    );
}

#[tokio::test]
#[ignore = "needs Lightroom with the agent bridge running and a photo selected"]
async fn view_photo_returns_a_jpeg() {
    let (mut client, status) = connected().await;
    let Some(photo) = selected_photo(&status) else {
        eprintln!("skipped: select a photo in Lightroom");
        return;
    };
    let (is_error, text, result) = client
        .tool("view_photo", json!({"photo_id": photo, "size": 256}))
        .await;
    assert!(!is_error, "view_photo failed: {text}");
    let image = result["content"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["type"] == "image")
        .expect("an image block");
    assert_eq!(image["mimeType"], "image/jpeg");
    assert!(image["data"].as_str().unwrap().len() > 100);
}

#[tokio::test]
#[ignore = "needs Lightroom with the agent bridge running"]
async fn catalog_queries_run_in_lightroom() {
    let (mut client, _) = connected().await;
    let found = client
        .tool_json(
            "find_photos_in_catalog",
            json!({"rating_min": 0, "limit": 3}),
        )
        .await;
    assert!(found["count"].as_u64().unwrap() <= 3);
    let collections = client.tool_json("list_collections", json!({})).await;
    assert!(collections["collections"].is_array());
}

#[tokio::test]
#[ignore = "needs Lightroom with the agent bridge running and developer commands on"]
async fn developer_commands_run_lua_in_the_plugin() {
    let (mut client, status) = connected().await;
    if status["lightroom"]["developer_commands"] != true {
        eprintln!("skipped: developer commands are off in the plugin");
        return;
    }
    let result = client
        .tool_json(
            "eval_lua",
            json!({"code": "print('from Lightroom') return LrApplication.versionString()"}),
        )
        .await;
    assert!(result["value"].is_string(), "{result}");
    assert_eq!(result["output"][0], "from Lightroom");
}
