//! The MCP server end to end, in memory: a real rmcp client lists and calls
//! the tools, and a simulated plugin claims and answers the commands they
//! queue on the agent bridge — the same path a Lightroom plugin takes, minus
//! Lightroom.

use std::sync::Arc;
use std::time::Duration;

use rmcp::model::{CallToolRequestParams, CallToolResult, ClientConfig};
use rmcp::service::RunningService;
use rmcp::{ClientHandler, RoleClient, ServiceExt};
use serde_json::{json, Value};

use lrg_api::agent_bridge::{ClaimedCommand, PluginInfo, PluginOutcome};
use lrg_api::state::AppState;
use lrg_mcp::{api::Api, sets::ResultSets, LrGeniusMcp, Shared};

#[derive(Clone, Default)]
struct TestClient;

impl ClientHandler for TestClient {
    fn get_info(&self) -> ClientConfig {
        ClientConfig::default()
    }
}

fn setup() -> (Arc<AppState>, LrGeniusMcp) {
    let state = Arc::new(AppState::new(None, true));
    let shared = Arc::new(Shared {
        state: state.clone(),
        api: Api::new(lrg_api::build_router(state.clone())),
        sets: ResultSets::new(),
    });
    (state, LrGeniusMcp::new(shared))
}

async fn connect(server: LrGeniusMcp) -> RunningService<RoleClient, TestClient> {
    let (server_side, client_side) = tokio::io::duplex(256 * 1024);
    tokio::spawn(async move {
        if let Ok(running) = server.serve(server_side).await {
            let _ = running.waiting().await;
        }
    });
    TestClient
        .serve(client_side)
        .await
        .expect("client connects")
}

async fn call(
    client: &RunningService<RoleClient, TestClient>,
    tool: &str,
    args: Value,
) -> CallToolResult {
    client
        .call_tool(
            CallToolRequestParams::new(tool.to_string())
                .with_arguments(args.as_object().cloned().unwrap_or_default()),
        )
        .await
        .expect("the call itself succeeds")
}

fn text(result: &CallToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|c| c.as_text())
        .map(|t| t.text.clone())
        .collect::<Vec<_>>()
        .join("\n")
}

fn json_of(result: &CallToolResult) -> Value {
    serde_json::from_str(&text(result)).expect("the tool answered with JSON")
}

/// Plays the plugin: answers each claimed command with `answer` until it has
/// answered `expected` commands.
fn simulate_plugin(
    state: &Arc<AppState>,
    token: String,
    expected: usize,
    answer: impl Fn(&ClaimedCommand) -> PluginOutcome + Send + 'static,
) -> tokio::task::JoinHandle<Vec<ClaimedCommand>> {
    let bridge = state.agent_bridge.clone();
    tokio::spawn(async move {
        let mut seen = Vec::new();
        for _ in 0..1000 {
            for command in bridge.claim(&token).expect("the session is live") {
                bridge.complete(&token, command.id, answer(&command));
                seen.push(command);
            }
            if seen.len() >= expected {
                return seen;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("the plugin saw {} of {expected} commands", seen.len());
    })
}

fn ok(result: Value) -> PluginOutcome {
    PluginOutcome {
        ok: true,
        result,
        ..Default::default()
    }
}

#[tokio::test]
async fn every_tool_and_prompt_is_listed_and_described() {
    let (_, server) = setup();
    let names = server.tool_names();
    let client = connect(server).await;

    let tools = client.list_all_tools().await.unwrap();
    assert_eq!(tools.len(), names.len());
    for expected in [
        "get_status",
        "search_photos",
        "find_photos_in_catalog",
        "cull_photos",
        "update_photos",
        "apply_develop_settings",
        "view_photo",
        "run_plugin_tests",
        "eval_lua",
    ] {
        assert!(
            tools.iter().any(|t| t.name == expected),
            "{expected} is listed"
        );
    }
    for tool in &tools {
        let described = tool.description.as_deref().unwrap_or("");
        assert!(
            described.len() > 30,
            "{} needs a real description",
            tool.name
        );
    }
    // Read-only tools say so, so clients need not ask before running them.
    let search = tools.iter().find(|t| t.name == "search_photos").unwrap();
    assert_eq!(
        search.annotations.as_ref().and_then(|a| a.read_only_hint),
        Some(true)
    );

    let prompts = client.list_all_prompts().await.unwrap();
    assert_eq!(prompts.len(), 4);
}

#[tokio::test]
async fn without_lightroom_the_tools_say_how_to_connect() {
    let (_, server) = setup();
    let client = connect(server).await;

    let collections = call(&client, "list_collections", json!({})).await;
    assert_eq!(collections.is_error, Some(true));
    assert!(text(&collections).contains("Enable the agent bridge"));

    let status = json_of(&call(&client, "get_status", json!({})).await);
    assert_eq!(status["lightroom"]["connected"], false);
    assert!(status["lightroom"]["how_to_connect"]
        .as_str()
        .unwrap()
        .contains("agent bridge"));
}

#[tokio::test]
async fn a_change_reaches_the_plugin_and_its_warnings_come_back() {
    let (state, server) = setup();
    let registration = state.agent_bridge.register(
        None,
        None,
        PluginInfo {
            allow_changes: true,
            catalog_id: "cat-1".into(),
            ..Default::default()
        },
    );
    let plugin = simulate_plugin(&state, registration.token, 2, |command| {
        match command.action.as_str() {
            "get_photos" => ok(json!({"photo_ids": ["p1", "p2"], "count": 2})),
            "update_photos" => PluginOutcome {
                ok: true,
                result: json!({"updated": 2}),
                warnings: vec!["p3 is not in this catalog".into()],
                ..Default::default()
            },
            other => panic!("unexpected command {other}"),
        }
    });

    let client = connect(server).await;
    let result = call(
        &client,
        "update_photos",
        json!({"set": "selection", "rating": 5, "flag": "pick", "add_keywords": ["Agent > Test"]}),
    )
    .await;
    let commands = plugin.await.unwrap();

    assert_ne!(result.is_error, Some(true), "{}", text(&result));
    let body = json_of(&result);
    assert_eq!(body["updated"], 2);
    assert_eq!(body["warnings"], json!(["p3 is not in this catalog"]));

    let update = commands
        .iter()
        .find(|c| c.action == "update_photos")
        .unwrap();
    assert_eq!(update.args["photo_ids"], json!(["p1", "p2"]));
    assert_eq!(update.args["rating"], 5);
    assert_eq!(update.args["flag"], "pick");
    assert_eq!(update.args["add_keywords"], json!(["Agent > Test"]));
}

#[tokio::test]
async fn a_read_only_bridge_refuses_changes_but_still_shows_results() {
    let (state, server) = setup();
    let registration = state
        .agent_bridge
        .register(None, None, PluginInfo::default());
    let plugin = simulate_plugin(&state, registration.token, 1, |command| {
        assert_eq!(command.action, "show_photos");
        ok(json!({"shown": 1}))
    });
    let client = connect(server).await;

    let refused = call(
        &client,
        "update_photos",
        json!({"photo_ids": ["p1"], "rating": 3}),
    )
    .await;
    assert_eq!(refused.is_error, Some(true));
    assert!(text(&refused).contains("read-only"));

    // Filling the agent's own collection is allowed when read-only.
    let shown = call(&client, "show_in_lightroom", json!({"photo_ids": ["p1"]})).await;
    plugin.await.unwrap();
    assert_ne!(shown.is_error, Some(true), "{}", text(&shown));
}

#[tokio::test]
async fn arguments_are_checked_before_anything_reaches_lightroom() {
    let (_, server) = setup();
    let client = connect(server).await;

    let flag = call(
        &client,
        "update_photos",
        json!({"photo_ids": ["p1"], "flag": "maybe"}),
    )
    .await;
    assert_eq!(flag.is_error, Some(true));
    assert!(text(&flag).contains("pick, reject, none"));

    let both = call(
        &client,
        "update_photos",
        json!({"photo_ids": ["p1"], "set": "s1", "rating": 1}),
    )
    .await;
    assert!(text(&both).contains("not both"));

    let metadata = call(
        &client,
        "analyze_photos",
        json!({"photo_ids": ["p1"], "tasks": ["metadata"]}),
    )
    .await;
    assert!(text(&metadata).contains("Analyze & Index"));
}

#[tokio::test]
async fn result_sets_are_made_combined_and_paged() {
    let (_, server) = setup();
    let client = connect(server).await;

    let a = json_of(
        &call(
            &client,
            "make_set",
            json!({"photo_ids": ["p1", "p2", "p3"], "label": "a"}),
        )
        .await,
    );
    let b = json_of(
        &call(
            &client,
            "make_set",
            json!({"photo_ids": ["p2", "p3", "p4"]}),
        )
        .await,
    );
    assert_eq!(a["count"], 3);

    let both = json_of(
        &call(
            &client,
            "combine_sets",
            json!({"a": a["set_id"], "b": b["set_id"], "operation": "intersection"}),
        )
        .await,
    );
    assert_eq!(both["count"], 2);

    let page = json_of(
        &call(
            &client,
            "get_set",
            json!({"set_id": both["set_id"], "offset": 1, "limit": 1}),
        )
        .await,
    );
    assert_eq!(page["photos"][0]["photo_id"], "p3");
    assert!(page.get("next_offset").is_none());

    let missing = call(&client, "get_set", json!({"set_id": "s999"})).await;
    assert_eq!(missing.is_error, Some(true));
}

#[tokio::test]
async fn developer_commands_need_the_developer_switch() {
    let (state, server) = setup();
    state.agent_bridge.register(
        None,
        None,
        PluginInfo {
            allow_changes: true,
            dev_tools: false,
            ..Default::default()
        },
    );
    let client = connect(server).await;
    let eval = call(&client, "eval_lua", json!({"code": "return 1"})).await;
    assert_eq!(eval.is_error, Some(true));
    assert!(text(&eval).contains("Developer commands are off"));
}
