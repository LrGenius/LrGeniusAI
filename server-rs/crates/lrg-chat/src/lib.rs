//! The in-app agent chat: a local model works the user's catalog through the
//! same tools the MCP endpoint offers, and the user talks to it — and approves
//! what it may change — in a page the backend serves.
//!
//! * [`session`] holds one conversation and runs the loop: ask the model,
//!   run the tool it chose, hand back the result, until it replies.
//! * [`mcp`] is the chat's in-memory MCP connection to `lrg_mcp::LrGeniusMcp`,
//!   so a chat call takes exactly the path an external agent's call takes.
//! * [`permissions`] decides what runs without asking.
//! * [`events`] is what the page is told, numbered so it can reconnect.
//! * [`routes`] is the HTTP surface, merged by `lrg-server` beside `/mcp`.
//!
//! The model answers through `lrg_providers::chat::ChatModel`, so a cloud
//! provider can take the local model's place without the loop changing.
//!
//! This crate sits above `lrg-mcp` and `lrg-api` for the same reason
//! `lrg-mcp` sits above `lrg-api`: it uses both, and `lrg-api` cannot depend
//! on either.

pub mod events;
pub mod mcp;
pub mod permissions;
pub mod prompt;
mod routes;
pub mod session;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use lrg_api::state::AppState;

use crate::permissions::PermissionStore;
use crate::session::ChatSession;

/// Chats open at once. Each holds a tool connection and a history, and one
/// user has little use for more.
const MAX_SESSIONS: usize = 8;

/// A chat nobody has looked at for this long is closed. A page that is open
/// keeps its chat alive by listening for events.
const IDLE_LIMIT: Duration = Duration::from_secs(2 * 60 * 60);

const SWEEP_EVERY: Duration = Duration::from_secs(60);

pub struct ChatHub {
    state: Arc<AppState>,
    /// The backend's own router, which the chat's tools call in-process.
    api: axum::Router,
    sessions: Mutex<HashMap<String, Arc<ChatSession>>>,
    permissions: Arc<PermissionStore>,
    shutdown: CancellationToken,
}

impl ChatHub {
    fn session(&self, id: &str) -> Option<Arc<ChatSession>> {
        self.sessions.lock().unwrap().get(id).cloned()
    }

    fn remove(&self, id: &str) {
        self.sessions.lock().unwrap().remove(id);
    }

    /// Keep a new chat, making room by closing the longest-idle one that is
    /// neither working nor watched.
    fn insert(&self, session: Arc<ChatSession>) -> Result<(), String> {
        let mut sessions = self.sessions.lock().unwrap();
        if sessions.len() >= MAX_SESSIONS {
            let evict = sessions
                .values()
                .filter(|s| !s.is_busy() && s.events.listeners() == 0)
                .max_by_key(|s| s.idle_for())
                .map(|s| s.id.clone());
            match evict {
                Some(id) => {
                    sessions.remove(&id);
                }
                None => {
                    return Err(format!(
                        "{MAX_SESSIONS} chats are open already. Close one of their browser tabs \
                         and try again."
                    ))
                }
            }
        }
        sessions.insert(session.id.clone(), session);
        Ok(())
    }

    fn sweep(&self) {
        self.sessions.lock().unwrap().retain(|id, session| {
            let keep = session.is_busy()
                || session.events.listeners() > 0
                || session.idle_for() < IDLE_LIMIT;
            if !keep {
                log::info!("Chat {id} closed after being idle");
            }
            keep
        });
    }
}

/// The chat's routes. `api` is the backend's own router
/// (`lrg_api::build_router`), which the tools call in-process; `shutdown`
/// ends every open event stream when cancelled.
pub fn router(
    state: Arc<AppState>,
    api: axum::Router,
    shutdown: CancellationToken,
) -> axum::Router {
    let hub = Arc::new(ChatHub {
        state,
        api,
        sessions: Mutex::default(),
        permissions: Arc::new(PermissionStore::new()),
        shutdown,
    });
    tokio::spawn(sweep_loop(Arc::downgrade(&hub)));
    routes::router(hub)
}

async fn sweep_loop(hub: Weak<ChatHub>) {
    let mut tick = tokio::time::interval(SWEEP_EVERY);
    loop {
        tick.tick().await;
        let Some(hub) = hub.upgrade() else {
            return;
        };
        hub.sweep();
    }
}
