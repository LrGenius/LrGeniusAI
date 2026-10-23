//! What the chat page is told, as it happens.
//!
//! Every event that changes what the page shows is numbered and kept, so a
//! page that reconnects — a reload, a laptop waking up — asks for everything
//! after the last number it saw and rebuilds the conversation exactly. The
//! text a model streams is the exception: those pieces go out live only,
//! unnumbered, and the finished turn follows as one numbered event, so the
//! log holds one entry per turn rather than one per token.

use std::sync::Mutex;

use serde::Serialize;
use serde_json::Value;
use tokio::sync::broadcast;

use crate::permissions::{AskReason, ToolClass};

/// Room for a burst of streamed pieces while a page is slow to read. A page
/// that falls further behind is told to reconnect, and the log catches it up.
const LIVE_CAPACITY: usize = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeltaKind {
    Reasoning,
    Text,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ChatEvent {
    UserMessage {
        text: String,
    },
    /// A piece of the turn being generated. Live only.
    Delta {
        kind: DeltaKind,
        text: String,
    },
    /// A finished turn: what the model thought and said. Its tool calls
    /// follow as their own events.
    AssistantTurn {
        reasoning: Option<String>,
        text: String,
    },
    ToolCall {
        call_id: String,
        tool: String,
        title: String,
        class: ToolClass,
        arguments: Value,
        /// How many photos the call touches, when known beforehand.
        photos: Option<usize>,
    },
    ApprovalNeeded {
        call_id: String,
        reason: AskReason,
    },
    ToolRunning {
        call_id: String,
    },
    ToolDone {
        call_id: String,
        ok: bool,
        /// The tool's answer, as the model reads it.
        text: String,
        /// The result set it produced, if any, for the page to show.
        set_id: Option<String>,
        count: Option<u64>,
        warnings: Vec<String>,
        images: usize,
    },
    /// Not run: denied, blocked, or a repeat of an earlier call.
    ToolSkipped {
        call_id: String,
        reason: String,
    },
    Warning {
        message: String,
    },
    Error {
        message: String,
    },
    /// The user stopped the chat; sent just before it goes idle. Whatever
    /// the model had streamed of an unfinished turn is not kept.
    Stopped,
    /// The chat started or finished working on a message.
    Busy {
        busy: bool,
    },
    /// How full the model's context is, after a turn.
    Usage {
        prompt_tokens: u32,
        context_window: Option<u32>,
    },
}

impl ChatEvent {
    fn is_live_only(&self) -> bool {
        matches!(self, ChatEvent::Delta { .. })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Sequenced {
    /// `None` for live-only events.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seq: Option<u64>,
    #[serde(flatten)]
    pub event: ChatEvent,
}

#[derive(Default)]
struct Inner {
    last: u64,
    events: Vec<Sequenced>,
}

pub struct EventLog {
    inner: Mutex<Inner>,
    live: broadcast::Sender<Sequenced>,
}

impl Default for EventLog {
    fn default() -> Self {
        EventLog {
            inner: Mutex::default(),
            live: broadcast::channel(LIVE_CAPACITY).0,
        }
    }
}

impl EventLog {
    pub fn emit(&self, event: ChatEvent) {
        // Logged and sent under one lock, so a page subscribing in between
        // can neither miss an event nor see one twice.
        let mut inner = self.inner.lock().unwrap();
        let seq = (!event.is_live_only()).then(|| {
            inner.last += 1;
            inner.last
        });
        let sequenced = Sequenced { seq, event };
        if seq.is_some() {
            inner.events.push(sequenced.clone());
        }
        let _ = self.live.send(sequenced);
    }

    /// How many pages are listening right now.
    #[must_use]
    pub fn listeners(&self) -> usize {
        self.live.receiver_count()
    }

    /// Everything logged after `after`, and the live events from here on.
    pub fn subscribe(&self, after: u64) -> (Vec<Sequenced>, broadcast::Receiver<Sequenced>) {
        let inner = self.inner.lock().unwrap();
        let receiver = self.live.subscribe();
        let past = inner
            .events
            .iter()
            .filter(|e| e.seq.is_some_and(|s| s > after))
            .cloned()
            .collect();
        (past, receiver)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deltas_go_out_live_but_are_not_kept() {
        let log = EventLog::default();
        let (_, mut live) = log.subscribe(0);
        log.emit(ChatEvent::UserMessage { text: "hi".into() });
        log.emit(ChatEvent::Delta {
            kind: DeltaKind::Text,
            text: "Hel".into(),
        });
        log.emit(ChatEvent::AssistantTurn {
            reasoning: None,
            text: "Hello".into(),
        });

        assert_eq!(live.try_recv().unwrap().seq, Some(1));
        assert_eq!(live.try_recv().unwrap().seq, None);
        assert_eq!(live.try_recv().unwrap().seq, Some(2));

        let (past, _) = log.subscribe(1);
        assert_eq!(past.len(), 1);
        assert_eq!(past[0].seq, Some(2));
    }

    #[test]
    fn events_serialize_flat_with_a_type() {
        let event = Sequenced {
            seq: Some(3),
            event: ChatEvent::Busy { busy: true },
        };
        assert_eq!(
            serde_json::to_value(&event).unwrap(),
            serde_json::json!({"seq": 3, "type": "busy", "busy": true})
        );
    }
}
