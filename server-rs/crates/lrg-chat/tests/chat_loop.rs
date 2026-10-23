//! The chat loop end to end, in memory: a scripted model drives the real MCP
//! tools through a session, and the tests answer its approval requests the
//! way the page would.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};

use lrg_api::state::AppState;
use lrg_chat::events::ChatEvent;
use lrg_chat::permissions::{AskReason, Mode, PermissionStore, Permissions, ToolRule};
use lrg_chat::session::{Approval, ChatSession, ModelSource};
use lrg_providers::chat::{
    ChatDelta, ChatDeltas, ChatModel, ChatPart, ChatRequest, ChatTurn, ToolCall,
};

/// Plays back fixed turns and keeps every request it was sent.
struct Scripted {
    turns: Mutex<VecDeque<ChatTurn>>,
    seen: Mutex<Vec<ChatRequest>>,
}

#[async_trait]
impl ChatModel for Scripted {
    fn name(&self) -> String {
        "scripted".into()
    }
    fn context_window(&self) -> Option<u32> {
        Some(32_768)
    }
    fn supports_images(&self) -> bool {
        false
    }
    async fn complete(
        &self,
        request: &ChatRequest,
        deltas: Option<ChatDeltas>,
    ) -> Result<ChatTurn, String> {
        self.seen.lock().unwrap().push(request.clone());
        let turn = self
            .turns
            .lock()
            .unwrap()
            .pop_front()
            .ok_or("the script ran out")?;
        if let (Some(deltas), false) = (deltas, turn.text.is_empty()) {
            let _ = deltas.send(ChatDelta::Text(turn.text.clone()));
        }
        Ok(turn)
    }
}

struct Fixed(Arc<Scripted>);

#[async_trait]
impl ModelSource for Fixed {
    async fn model(&self) -> Result<Arc<dyn ChatModel>, String> {
        Ok(self.0.clone())
    }
    fn label(&self) -> String {
        "scripted".into()
    }
}

fn call(id: &str, name: &str, arguments: Value) -> ChatTurn {
    ChatTurn {
        reasoning: Some(format!("Use {name}.")),
        tool_calls: vec![ToolCall {
            id: id.into(),
            name: name.into(),
            arguments,
        }],
        ..ChatTurn::default()
    }
}

fn reply(text: &str) -> ChatTurn {
    ChatTurn {
        text: text.into(),
        ..ChatTurn::default()
    }
}

async fn open(turns: Vec<ChatTurn>, permissions: Permissions) -> (Arc<ChatSession>, Arc<Scripted>) {
    let state = Arc::new(AppState::new(None, true));
    let api = lrg_api::build_router(state.clone());
    let model = Arc::new(Scripted {
        turns: Mutex::new(turns.into()),
        seen: Mutex::default(),
    });
    let store = Arc::new(PermissionStore::new());
    // No index is bound in these tests, so the settings live in memory.
    store.set(None, permissions).unwrap();
    let session = ChatSession::open(state, api, Box::new(Fixed(model.clone())), store)
        .await
        .expect("the session opens");
    (session, model)
}

enum Answer {
    Approve(Approval),
    Stop,
}

/// Sends `text` and collects events until the chat is idle again, answering
/// each approval request with `answer`.
async fn run(
    session: &Arc<ChatSession>,
    text: &str,
    mut answer: impl FnMut(&str) -> Answer,
) -> Vec<ChatEvent> {
    let (_, mut live) = session.events.subscribe(u64::MAX);
    session.send(text.into()).unwrap();
    let mut events = Vec::new();
    loop {
        let event = tokio::time::timeout(Duration::from_secs(30), live.recv())
            .await
            .expect("the chat does not stall")
            .expect("the log stays open")
            .event;
        if let ChatEvent::ApprovalNeeded { call_id, .. } = &event {
            match answer(call_id) {
                Answer::Approve(approval) => session.approve(call_id, approval).unwrap(),
                Answer::Stop => session.stop(),
            }
        }
        let done = event == ChatEvent::Busy { busy: false };
        events.push(event);
        if done {
            return events;
        }
    }
}

fn never_asked(_: &str) -> Answer {
    panic!("nothing in this test should need approval")
}

fn approvals(events: &[ChatEvent]) -> Vec<AskReason> {
    events
        .iter()
        .filter_map(|e| match e {
            ChatEvent::ApprovalNeeded { reason, .. } => Some(reason.clone()),
            _ => None,
        })
        .collect()
}

fn skipped(events: &[ChatEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            ChatEvent::ToolSkipped { reason, .. } => Some(reason.clone()),
            _ => None,
        })
        .collect()
}

/// The text of the tool results in the last message the model was sent.
fn last_results(model: &Scripted) -> Vec<String> {
    let seen = model.seen.lock().unwrap();
    let request = seen.last().expect("the model was asked");
    request
        .messages
        .last()
        .unwrap()
        .parts
        .iter()
        .filter_map(|p| match p {
            ChatPart::ToolResult(r) => Some(r.text.clone()),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn looking_runs_without_asking_and_the_model_reads_the_result() {
    let (session, model) = open(
        vec![call("c1", "get_status", json!({})), reply("All good.")],
        Permissions::default(),
    )
    .await;
    let events = run(&session, "How is my catalog?", never_asked).await;

    assert!(events.contains(&ChatEvent::ToolRunning {
        call_id: "c1".into()
    }));
    assert!(events
        .iter()
        .any(|e| matches!(e, ChatEvent::ToolDone { call_id, .. } if call_id == "c1")));
    assert!(events.contains(&ChatEvent::AssistantTurn {
        reasoning: None,
        text: "All good.".into()
    }));
    let results = last_results(&model);
    assert_eq!(results.len(), 1);
    assert!(!results[0].is_empty(), "the status reached the model");
}

#[tokio::test]
async fn a_change_waits_for_approval_and_a_denial_reaches_the_model() {
    let (session, model) = open(
        vec![
            call("c1", "update_photos", json!({"set": "s9", "rating": 5})),
            reply("Understood."),
        ],
        Permissions::default(),
    )
    .await;
    let events = run(&session, "Rate them five stars.", |_| {
        Answer::Approve(Approval::Deny {
            note: Some("only the best ones".into()),
        })
    })
    .await;

    assert_eq!(approvals(&events), vec![AskReason::Mode]);
    assert_eq!(skipped(&events), vec!["not allowed".to_string()]);
    assert!(!events.contains(&ChatEvent::ToolRunning {
        call_id: "c1".into()
    }));
    let results = last_results(&model);
    assert!(
        results[0].contains("did not allow") && results[0].contains("only the best ones"),
        "{results:?}"
    );
}

#[tokio::test]
async fn allowing_a_tool_for_the_chat_stops_the_questions() {
    let (session, _) = open(
        vec![
            call("c1", "update_photos", json!({"set": "s9", "rating": 5})),
            call("c2", "update_photos", json!({"set": "s9", "rating": 4})),
            reply("Done."),
        ],
        Permissions::default(),
    )
    .await;
    let events = run(&session, "Rate them.", |_| Answer::Approve(Approval::Chat)).await;

    assert_eq!(approvals(&events).len(), 1, "asked once, then allowed");
    // Both ran; without Lightroom they fail, which the page is told.
    for id in ["c1", "c2"] {
        assert!(events.contains(&ChatEvent::ToolRunning { call_id: id.into() }));
        assert!(events
            .iter()
            .any(|e| matches!(e, ChatEvent::ToolDone { call_id, ok: false, .. } if call_id == id)));
    }
}

#[tokio::test]
async fn the_same_call_twice_is_not_run_again() {
    let (session, model) = open(
        vec![
            call("c1", "get_status", json!({})),
            call("c2", "get_status", json!({})),
            reply("Done."),
        ],
        Permissions::default(),
    )
    .await;
    let events = run(&session, "Status?", never_asked).await;

    assert_eq!(skipped(&events), vec!["repeated call".to_string()]);
    assert!(last_results(&model)[0].contains("already made exactly this call"));
}

#[tokio::test]
async fn stopping_denies_the_call_that_waits_and_ends_the_turn() {
    let (session, model) = open(
        vec![
            call("c1", "update_photos", json!({"set": "s9", "rating": 5})),
            reply("never reached"),
        ],
        Permissions::default(),
    )
    .await;
    let events = run(&session, "Rate them.", |_| Answer::Stop).await;

    assert_eq!(skipped(&events), vec!["stopped".to_string()]);
    assert!(events.contains(&ChatEvent::Stopped), "the page is told");
    assert_eq!(
        model.seen.lock().unwrap().len(),
        1,
        "no turn after the stop"
    );
    assert!(!session.is_busy());

    // The next message reaches the model with the stop marked before it, so
    // it does not take up the stopped request again.
    run(&session, "Something else.", never_asked).await;
    let seen = model.seen.lock().unwrap();
    let texts: Vec<&str> = seen[1]
        .messages
        .iter()
        .flat_map(|m| &m.parts)
        .filter_map(|p| match p {
            ChatPart::Text(text) => Some(text.as_str()),
            _ => None,
        })
        .collect();
    let stop = texts.iter().position(|t| t.contains("stopped this"));
    let next = texts.iter().position(|t| *t == "Something else.");
    assert!(stop.is_some() && stop < next, "{texts:?}");
}

#[tokio::test]
async fn a_blocked_tool_is_not_offered() {
    let mut permissions = Permissions::default();
    permissions
        .tools
        .insert("search_photos".into(), ToolRule::Block);
    let (session, model) = open(vec![reply("Hi.")], permissions).await;
    run(&session, "Hello", never_asked).await;

    let seen = model.seen.lock().unwrap();
    let offered: Vec<&str> = seen[0].tools.iter().map(|t| t.name.as_str()).collect();
    assert!(!offered.contains(&"search_photos"), "{offered:?}");
    assert!(offered.contains(&"get_status"));
    assert!(
        !offered.contains(&"eval_lua"),
        "developer tools are never offered"
    );
}

#[tokio::test]
async fn a_change_to_many_photos_asks_even_when_everything_is_allowed() {
    let ids: Vec<String> = (0..201).map(|i| format!("p{i}")).collect();
    let (session, _) = open(
        vec![
            call(
                "c1",
                "update_photos",
                json!({"photo_ids": ids, "rating": 5}),
            ),
            reply("ok"),
        ],
        Permissions {
            mode: Mode::AutoAll,
            ..Permissions::default()
        },
    )
    .await;
    let events = run(&session, "Rate all.", |_| {
        Answer::Approve(Approval::Deny { note: None })
    })
    .await;

    assert_eq!(
        approvals(&events),
        vec![AskReason::Bulk {
            photos: 201,
            limit: 200
        }]
    );
    assert!(events.iter().any(|e| matches!(
        e,
        ChatEvent::ToolCall {
            photos: Some(201),
            ..
        }
    )));
}

#[tokio::test]
async fn a_second_message_waits_for_the_first() {
    let (session, _) = open(
        vec![call(
            "c1",
            "update_photos",
            json!({"set": "s9", "rating": 5}),
        )],
        Permissions::default(),
    )
    .await;
    let (_, mut live) = session.events.subscribe(u64::MAX);
    session.send("Rate them.".into()).unwrap();
    // Wait until the call is parked on its approval.
    loop {
        if let ChatEvent::ApprovalNeeded { .. } = live.recv().await.unwrap().event {
            break;
        }
    }
    let refused = session.send("And another thing".into()).unwrap_err();
    assert!(refused.contains("Still working"), "{refused}");
    session.stop();
}
