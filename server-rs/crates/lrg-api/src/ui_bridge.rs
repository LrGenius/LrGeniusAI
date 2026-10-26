//! The browser ↔ Lightroom bridge behind `/v1/ui/*`.
//!
//! A `/v1/ui/` page runs in the browser and can do everything that is purely
//! backend data on its own — listing persons, renaming them, re-clustering,
//! searching. What it cannot do is touch the Lightroom catalog: only the
//! plugin can create a collection or change the Library selection. So actions
//! that end in Lightroom are queued here. The page POSTs one, the plugin
//! drains the queue while its task is running, and performs it.
//!
//! Both ends also stamp when they were last heard from. That is what lets
//! either side tell the user the other one is gone instead of appearing to
//! hang: a page nobody polls is a task that was closed in Lightroom, and a
//! polling plugin with no page is a browser window that never opened.
//!
//! Everything is kept **per page** (`"people"`, `"search"`): Lightroom runs
//! plug-in tasks side by side, and with one shared queue the People task
//! would drain — and reject as unknown — what the search page queued for its
//! own task. A caller that names no page means `"people"`, which is what
//! every client did before there was a second page.
//!
//! The search page also needs to know what Lightroom's current view and
//! selection are, which only the plugin can read; its task posts them as the
//! page's *context* ([`UiBridge::set_context`]) before opening the page, and
//! again when the page asks for a refresh.

use std::collections::{HashMap, VecDeque};
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

/// Upper bound on queued actions per page. Reaching it means nothing is
/// draining — the plugin's task was closed while the page stayed open — so
/// the oldest entries are the stale ones and get dropped. The page learns
/// about this from `plugin_connected` in the POST response, not from the cap.
const MAX_QUEUED: usize = 64;

/// The page a request that names none is about.
pub const DEFAULT_PAGE: &str = "people";

/// The pages there are. A request naming another one is refused at the door
/// (`routes::ui`), rather than opening a queue nothing will ever drain.
pub const PAGES: &[&str] = &["people", "search"];

#[derive(Default)]
struct PageState {
    queue: VecDeque<Value>,
    plugin_seen: Option<Instant>,
    page_seen: Option<Instant>,
    context: Option<Value>,
    /// Bumped on every `set_context`, so a page that asked for a refresh can
    /// tell the new context from the old one.
    context_revision: u64,
}

/// Shared, process-global, one entry per page.
#[derive(Default)]
pub struct UiBridge {
    pages: Mutex<HashMap<String, PageState>>,
}

fn fresh(stamp: Option<Instant>, window: Duration) -> bool {
    stamp.is_some_and(|t| t.elapsed() <= window)
}

impl UiBridge {
    pub fn new() -> Self {
        Self::default()
    }

    fn with_page<T>(&self, page: &str, f: impl FnOnce(&mut PageState) -> T) -> T {
        let mut pages = self.pages.lock().unwrap();
        f(pages.entry(page.to_string()).or_default())
    }

    /// Queues one action for the plugin task serving `page`. Returns how
    /// many are now pending there.
    pub fn enqueue(&self, page: &str, action: Value) -> usize {
        self.with_page(page, |state| {
            state.page_seen = Some(Instant::now());
            while state.queue.len() >= MAX_QUEUED {
                let dropped = state.queue.pop_front();
                log::warn!(
                    "UI bridge queue for {page} full ({MAX_QUEUED}); dropping the oldest action \
                     {:?}. Nothing is draining it — is the task still open in Lightroom?",
                    dropped
                        .as_ref()
                        .and_then(|a| a.get("action"))
                        .and_then(Value::as_str)
                        .unwrap_or("?")
                );
            }
            state.queue.push_back(action);
            state.queue.len()
        })
    }

    /// Hands every pending action of `page` to the plugin and clears its queue.
    pub fn drain(&self, page: &str) -> Vec<Value> {
        self.with_page(page, |state| {
            state.plugin_seen = Some(Instant::now());
            state.queue.drain(..).collect()
        })
    }

    /// Records that the page is alive without touching the queue.
    pub fn mark_page_seen(&self, page: &str) {
        self.with_page(page, |state| state.page_seen = Some(Instant::now()));
    }

    /// True while the plugin task of `page` has polled recently enough to act
    /// on an action.
    pub fn plugin_connected(&self, page: &str) -> bool {
        self.with_page(page, |state| fresh(state.plugin_seen, PLUGIN_ALIVE))
    }

    /// True while `page` has loaded or heartbeated recently.
    pub fn page_open(&self, page: &str) -> bool {
        self.with_page(page, |state| fresh(state.page_seen, PAGE_ALIVE))
    }

    /// Stores what the plugin knows and the page cannot (the search page's
    /// view and selection). Returns the new revision. Setting it also counts
    /// as the plugin being there.
    pub fn set_context(&self, page: &str, context: Value) -> u64 {
        self.with_page(page, |state| {
            state.context = Some(context);
            state.context_revision += 1;
            state.plugin_seen = Some(Instant::now());
            state.context_revision
        })
    }

    /// The last context the plugin stored for `page`, with its revision.
    pub fn context(&self, page: &str) -> (Option<Value>, u64) {
        self.with_page(page, |state| {
            (state.context.clone(), state.context_revision)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn drain_returns_actions_in_order_and_empties_the_queue() {
        let bridge = UiBridge::new();
        bridge.enqueue("people", json!({"action": "a"}));
        bridge.enqueue("people", json!({"action": "b"}));

        let drained = bridge.drain("people");
        assert_eq!(drained.len(), 2);
        assert_eq!(drained[0]["action"], "a");
        assert_eq!(drained[1]["action"], "b");
        assert!(bridge.drain("people").is_empty());
    }

    #[test]
    fn queue_is_capped_by_dropping_the_oldest() {
        let bridge = UiBridge::new();
        for i in 0..MAX_QUEUED + 3 {
            bridge.enqueue("people", json!({ "action": i }));
        }

        let drained = bridge.drain("people");
        assert_eq!(drained.len(), MAX_QUEUED);
        // The three oldest fell off the front, so the survivors start at 3.
        assert_eq!(drained[0]["action"], 3);
    }

    #[test]
    fn liveness_starts_false_and_follows_each_side_independently() {
        let bridge = UiBridge::new();
        assert!(!bridge.plugin_connected("people"));
        assert!(!bridge.page_open("people"));

        bridge.mark_page_seen("people");
        assert!(bridge.page_open("people"));
        assert!(!bridge.plugin_connected("people"));

        bridge.drain("people");
        assert!(bridge.plugin_connected("people"));
    }

    #[test]
    fn enqueueing_counts_as_the_page_being_alive() {
        let bridge = UiBridge::new();
        bridge.enqueue("people", json!({"action": "show_in_library"}));
        assert!(bridge.page_open("people"));
    }

    /// The reason for per-page state: a People task polling must neither take
    /// the search page's actions nor count as the search page's task.
    #[test]
    fn pages_have_their_own_queues_and_liveness() {
        let bridge = UiBridge::new();
        bridge.enqueue("search", json!({"action": "show_search_results"}));
        assert!(bridge.drain("people").is_empty());
        assert!(bridge.plugin_connected("people"));
        assert!(!bridge.plugin_connected("search"));
        assert!(!bridge.page_open("people"));
        assert!(bridge.page_open("search"));
        assert_eq!(bridge.drain("search").len(), 1);
    }

    #[test]
    fn context_is_kept_per_page_with_a_rising_revision() {
        let bridge = UiBridge::new();
        assert_eq!(bridge.context("search"), (None, 0));
        assert_eq!(bridge.set_context("search", json!({"n": 1})), 1);
        assert_eq!(bridge.set_context("search", json!({"n": 2})), 2);
        assert_eq!(bridge.context("search"), (Some(json!({"n": 2})), 2));
        assert_eq!(bridge.context("people"), (None, 0));
        // Posting a context is the plugin being there.
        assert!(bridge.plugin_connected("search"));
    }
}
