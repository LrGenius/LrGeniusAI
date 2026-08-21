//! The browser ↔ Lightroom bridge behind `/v1/ui/*`.
//!
//! A `/v1/ui/` page runs in the browser and can do everything that is purely
//! backend data on its own — listing persons, renaming them, re-clustering.
//! What it cannot do is touch the Lightroom catalog: only the plugin can
//! create a collection or change the Library selection. So actions that end
//! in Lightroom are queued here. The page POSTs one, the plugin drains the
//! queue while its task is running, and performs it.
//!
//! Both ends also stamp when they were last heard from. That is what lets
//! either side tell the user the other one is gone instead of appearing to
//! hang: a page nobody polls is a task that was closed in Lightroom, and a
//! polling plugin with no page is a browser window that never opened.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::Value;

/// How long after its last poll the plugin still counts as listening. It
/// polls about twice a second, so this absorbs a few missed beats (a slow
/// catalog write, a `LrTasks` scheduling gap) without reporting it gone.
const PLUGIN_ALIVE: Duration = Duration::from_secs(5);

/// Same, for the page. Its heartbeat is slower, and a browser throttles
/// timers in a hidden tab down to about one a minute, so the window has to be
/// wide enough that a page sitting in the background does not read as gone.
const PAGE_ALIVE: Duration = Duration::from_secs(90);

/// Upper bound on queued actions. Reaching it means nothing is draining —
/// the plugin's task was closed while the page stayed open — so the oldest
/// entries are the stale ones and get dropped. The page learns about this
/// from `plugin_connected` in the POST response, not from the cap.
const MAX_QUEUED: usize = 64;

#[derive(Default)]
struct Inner {
    queue: VecDeque<Value>,
    plugin_seen: Option<Instant>,
    page_seen: Option<Instant>,
}

/// Shared, process-global; one queue serves whichever `/v1/ui/` page is open,
/// since Lightroom runs one modal task at a time anyway.
#[derive(Default)]
pub struct UiBridge {
    inner: Mutex<Inner>,
}

fn fresh(stamp: Option<Instant>, window: Duration) -> bool {
    stamp.is_some_and(|t| t.elapsed() <= window)
}

impl UiBridge {
    pub fn new() -> Self {
        Self::default()
    }

    /// Queues one action for the plugin. Returns how many are now pending.
    pub fn enqueue(&self, action: Value) -> usize {
        let mut inner = self.inner.lock().unwrap();
        inner.page_seen = Some(Instant::now());
        while inner.queue.len() >= MAX_QUEUED {
            let dropped = inner.queue.pop_front();
            log::warn!(
                "UI bridge queue full ({MAX_QUEUED}); dropping the oldest action {:?}. \
                 Nothing is draining it — is the task still open in Lightroom?",
                dropped
                    .as_ref()
                    .and_then(|a| a.get("action"))
                    .and_then(Value::as_str)
                    .unwrap_or("?")
            );
        }
        inner.queue.push_back(action);
        inner.queue.len()
    }

    /// Hands every pending action to the plugin and clears the queue.
    pub fn drain(&self) -> Vec<Value> {
        let mut inner = self.inner.lock().unwrap();
        inner.plugin_seen = Some(Instant::now());
        inner.queue.drain(..).collect()
    }

    /// Records that the page is alive without touching the queue.
    pub fn mark_page_seen(&self) {
        self.inner.lock().unwrap().page_seen = Some(Instant::now());
    }

    /// True while the plugin has polled recently enough to act on an action.
    pub fn plugin_connected(&self) -> bool {
        fresh(self.inner.lock().unwrap().plugin_seen, PLUGIN_ALIVE)
    }

    /// True while a page has loaded or heartbeated recently.
    pub fn page_open(&self) -> bool {
        fresh(self.inner.lock().unwrap().page_seen, PAGE_ALIVE)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn drain_returns_actions_in_order_and_empties_the_queue() {
        let bridge = UiBridge::new();
        bridge.enqueue(json!({"action": "a"}));
        bridge.enqueue(json!({"action": "b"}));

        let drained = bridge.drain();
        assert_eq!(drained.len(), 2);
        assert_eq!(drained[0]["action"], "a");
        assert_eq!(drained[1]["action"], "b");
        assert!(bridge.drain().is_empty());
    }

    #[test]
    fn queue_is_capped_by_dropping_the_oldest() {
        let bridge = UiBridge::new();
        for i in 0..MAX_QUEUED + 3 {
            bridge.enqueue(json!({ "action": i }));
        }

        let drained = bridge.drain();
        assert_eq!(drained.len(), MAX_QUEUED);
        // The three oldest fell off the front, so the survivors start at 3.
        assert_eq!(drained[0]["action"], 3);
    }

    #[test]
    fn liveness_starts_false_and_follows_each_side_independently() {
        let bridge = UiBridge::new();
        assert!(!bridge.plugin_connected());
        assert!(!bridge.page_open());

        bridge.mark_page_seen();
        assert!(bridge.page_open());
        assert!(!bridge.plugin_connected());

        bridge.drain();
        assert!(bridge.plugin_connected());
    }

    #[test]
    fn enqueueing_counts_as_the_page_being_alive() {
        let bridge = UiBridge::new();
        bridge.enqueue(json!({"action": "show_in_library"}));
        assert!(bridge.page_open());
    }
}
