//! The Lightroom ↔ backend command bridge behind `/v1/agent/*`, and what the
//! MCP endpoint (`lrg-mcp`) drives Lightroom through.
//!
//! The backend cannot touch the Lightroom catalog; only the plugin can. So a
//! command for Lightroom — rate these photos, show that collection — is queued
//! here, and the plugin carries it out and reports back. Two channels do that:
//!
//! * **A doorbell.** The plugin listens on an `LrSocket` in receive mode, on a
//!   port the OS picks, and tells us that port when it registers. We connect to
//!   it and write one line, `wake`, whenever a command is queued, plus a `ping`
//!   every few seconds so the plugin can tell a dead backend from a quiet one.
//!   The line carries nothing else: the plugin answers it by asking for the
//!   queued commands over HTTP. Anything else that manages to write to that
//!   port can at most make the plugin ask for work that is not there.
//!
//! * **HTTP for everything with content.** Commands are claimed with
//!   `GET /v1/agent/commands` and answered with
//!   `POST /v1/agent/commands/{id}/result`, both carrying the session token the
//!   plugin was given when it registered.
//!
//! Why not a socket each way, as lightroom-mcp does: `LrSocket`'s *send* mode
//! does not reliably notice that its client went away on Windows and keeps
//! "sending" into a dead connection (lightroom-mcp issues #110 and #134). The
//! receive side is the well-behaved half, so it is the only half used, and only
//! as an accelerator: without a socket the plugin asks every second instead,
//! which is slower but loses nothing.
//!
//! Commands run one at a time in the plugin. It claims them in order and works
//! through them on a single task, because parallel `LrTasks` have caused
//! trouble in this plugin before, mostly on Windows.

use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{mpsc, oneshot};

/// How long after the plugin last talked to us it still counts as there when
/// the doorbell socket is not connected. The plugin falls back to asking every
/// second without a socket, so this absorbs a few missed beats.
const PLUGIN_ALIVE: Duration = Duration::from_secs(10);

/// How often the doorbell rings with nothing queued. The plugin treats a
/// connected socket that has been silent for a few of these as dead and binds
/// a fresh one — Windows `LrSocket` does not always report the close.
pub const PING_INTERVAL: Duration = Duration::from_secs(5);

/// Connecting to the plugin's port right after it registers can race the
/// listener coming up; a handful of short retries covers that.
const CONNECT_ATTEMPTS: u32 = 5;
const CONNECT_RETRY: Duration = Duration::from_millis(300);

/// Upper bound on commands waiting to be claimed. Each one has a caller
/// awaiting it, so reaching this means many agents at once or a plugin that
/// stopped claiming without saying so; refusing is better than growing.
const MAX_QUEUED: usize = 256;

/// What the plugin reports about itself when it registers. Shown back by
/// `GET /v1/agent/bridge`, and read by the MCP tools to scope backend queries
/// to the open catalog.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct PluginInfo {
    #[serde(default)]
    pub plugin_version: String,
    #[serde(default)]
    pub lightroom_version: String,
    #[serde(default)]
    pub catalog_path: String,
    #[serde(default)]
    pub catalog_name: String,
    /// The backend's catalog scope for this catalog — the same value the
    /// plugin sends as `catalog_id` on its own requests.
    #[serde(default)]
    pub catalog_id: String,
    #[serde(default)]
    pub db_path: String,
    /// Off unless the plugin says otherwise: an old or confused plugin that
    /// does not send it gets the read-only behaviour, not the permissive one.
    #[serde(default)]
    pub allow_changes: bool,
    /// The developer commands (`eval_lua`, `reload_modules`). Only a plugin
    /// running from a `.lrdevplugin` folder with the developer switch on
    /// offers them.
    #[serde(default)]
    pub dev_tools: bool,
    #[serde(default)]
    pub platform: String,
}

/// A successful command: what the plugin returned, plus anything that went
/// partly wrong. Warnings travel with the result rather than into a log so the
/// agent can tell the user (see "Errors must surface" in CLAUDE.md).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CommandReply {
    pub result: Value,
    pub warnings: Vec<String>,
}

/// Progress the plugin reports while working through a long command.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Progress {
    #[serde(default)]
    pub done: f64,
    #[serde(default)]
    pub total: Option<f64>,
    #[serde(default)]
    pub message: Option<String>,
}

/// Why a command did not produce a reply. The messages are written for the
/// agent reading them, and through it the user: each says what to do next.
#[derive(Debug, Clone, PartialEq)]
pub enum BridgeError {
    /// No plugin registered, or it went quiet.
    NotConnected,
    /// The user stopped the bridge in Lightroom while this was pending.
    Stopped,
    /// The plugin registered again (it was reloaded) and the old instance's
    /// commands died with it.
    Replaced,
    /// The plugin is read-only by the user's choice.
    ChangesNotAllowed,
    /// The command needs the developer switch in the plugin.
    DevToolsDisabled,
    Busy,
    TimedOut {
        action: String,
        seconds: u64,
        claimed: bool,
        /// What Lightroom was busy with instead, and for how long: commands
        /// run one at a time, so a long one makes the next wait its turn.
        busy_with: Option<(String, u64)>,
    },
    /// The plugin ran the command and it failed.
    Plugin {
        message: String,
        warnings: Vec<String>,
    },
}

impl fmt::Display for BridgeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BridgeError::NotConnected => write!(
                f,
                "Lightroom is not connected. Ask the user to open Lightroom Classic with the \
                 LrGeniusAI plugin; its agent bridge starts with it (Plug-in Manager > LrGeniusAI \
                 > AI Agents > Enable the agent bridge)."
            ),
            BridgeError::Stopped => write!(
                f,
                "The user stopped agent control in Lightroom. Do not retry; ask the user to start \
                 the AI Agent Bridge again if they want you to continue."
            ),
            BridgeError::Replaced => write!(
                f,
                "The LrGeniusAI plugin was reloaded in Lightroom while this request was pending. \
                 Check whether it took effect before trying again."
            ),
            BridgeError::ChangesNotAllowed => write!(
                f,
                "The user has set the agent bridge to read-only in Lightroom, so photos cannot be \
                 changed. Ask the user to allow changes in the LrGeniusAI Plug-in Manager if they \
                 want this."
            ),
            BridgeError::DevToolsDisabled => write!(
                f,
                "Developer commands are off. They need the plugin installed from its \
                 .lrdevplugin folder and \"Allow developer commands\" ticked in the LrGeniusAI \
                 Plug-in Manager."
            ),
            BridgeError::Busy => write!(
                f,
                "Lightroom has too many agent requests waiting. Wait for the current ones to \
                 finish, then try again."
            ),
            BridgeError::TimedOut {
                action,
                seconds,
                claimed: false,
                busy_with: Some((busy, running)),
            } => write!(
                f,
                "Lightroom is still working on \"{busy}\" (for {running} s so far) and runs one \
                 request at a time, so \"{action}\" did not start within {seconds} s. Wait until \
                 it finishes, then try again."
            ),
            BridgeError::TimedOut {
                action,
                seconds,
                claimed: false,
                busy_with: None,
            } => write!(
                f,
                "Lightroom did not pick up \"{action}\" within {seconds} s. It may be showing a \
                 dialog or busy with a long task; ask the user to check Lightroom."
            ),
            BridgeError::TimedOut {
                action,
                seconds,
                claimed: true,
                ..
            } => write!(
                f,
                "Lightroom is still working on \"{action}\" after {seconds} s and may finish \
                 later. Check the photos before retrying, or the change may be applied twice."
            ),
            BridgeError::Plugin { message, .. } => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for BridgeError {}

/// How a caller wants one command run.
pub struct CallOptions {
    pub timeout: Duration,
    /// The command changes the catalog. Refused up front when the plugin is
    /// read-only, rather than queued for the plugin to refuse.
    pub changes: bool,
    /// The command needs the developer switch.
    pub dev: bool,
    /// Where to forward the plugin's progress reports, if anywhere.
    pub progress: Option<mpsc::UnboundedSender<Progress>>,
}

impl CallOptions {
    pub fn read(timeout: Duration) -> Self {
        Self {
            timeout,
            changes: false,
            dev: false,
            progress: None,
        }
    }

    pub fn write(timeout: Duration) -> Self {
        Self {
            changes: true,
            ..Self::read(timeout)
        }
    }
}

/// A command as the plugin receives it from `GET /v1/agent/commands`.
#[derive(Debug, Clone, Serialize)]
pub struct ClaimedCommand {
    pub id: u64,
    pub action: String,
    pub args: Value,
    /// How long the caller waits. The plugin may use it to give up early on
    /// something it can tell will not finish in time.
    pub timeout_seconds: u64,
}

/// What a plugin sends to register.
#[derive(Debug, Clone, Default)]
pub struct RegisterRequest {
    /// The plugin's LrSocket port, when it could open one.
    pub port: Option<u16>,
    /// The session token it already holds, to keep that session.
    pub previous_token: Option<String>,
    pub info: PluginInfo,
    /// The agent token the plugin keeps in its password storage. Absent on
    /// its first registration; the backend then makes one.
    pub mcp_token: Option<String>,
    /// Make a new agent token even though the plugin sent one: the user
    /// asked for it, and every MCP client set up with the old one is locked
    /// out.
    pub rotate_mcp_token: bool,
    /// When this bridge started (any clock the plugin keeps steady, seconds).
    /// The most recently started bridge wins: see [`Superseded`].
    pub started_at: Option<f64>,
}

/// A registration from a bridge older than one already seen.
///
/// "Reload Plug-in" gives the plugin a fresh Lua environment, but a bridge
/// loop the old instance started can keep running (LrC 15.6), and the two
/// then replace each other's session every few seconds. The plugin sends when
/// its bridge started; a bridge older than the newest one seen is refused, and
/// stops itself on that answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Superseded;

/// What `register` hands back to the plugin.
#[derive(Debug, Clone, Serialize)]
pub struct Registration {
    pub token: String,
    pub session: u64,
    /// True when this re-registration kept an existing session (same plugin
    /// instance, new socket port) rather than starting a new one.
    pub resumed: bool,
    /// The token MCP clients must present, for the plugin to keep. Only ever
    /// in this answer to the registering plugin; never in the status.
    pub mcp_token: String,
}

/// Whether a request to `/mcp` may proceed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpAccess {
    Granted,
    /// No plugin has registered since the backend started (or the user
    /// stopped the bridge), so there is no agent token to compare against.
    NoToken,
    /// A token is set, and the request carried a different one or none.
    Denied,
}

/// The plugin's answer to one command, as posted to
/// `POST /v1/agent/commands/{id}/result`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PluginOutcome {
    #[serde(default)]
    pub ok: bool,
    #[serde(default)]
    pub result: Value,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub warnings: Vec<String>,
    /// The plugin refused because it is read-only or lacks developer
    /// commands, so the right error type can reach the agent.
    #[serde(default)]
    pub refused: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SocketState {
    /// Registered without a port, or still connecting.
    None,
    Connecting,
    Connected,
    /// We could not connect, or the connection dropped. The plugin keeps
    /// asking over HTTP in the meantime.
    Down,
}

struct Session {
    id: u64,
    token: String,
    info: PluginInfo,
    port: Option<u16>,
    registered_at: chrono::DateTime<chrono::Utc>,
    last_seen: Instant,
    socket: SocketState,
    /// Dropping this ends the doorbell task, which closes the connection.
    doorbell: Option<mpsc::UnboundedSender<()>>,
    /// Bumped for every doorbell task, so a task for a socket the plugin
    /// already replaced cannot report the new connection as down.
    doorbell_gen: u64,
    commands_done: u64,
    /// What MCP clients must present. Bound to the session on purpose:
    /// whoever registers decides the token, but also receives the commands —
    /// a process posing as the plugin only ever controls itself.
    mcp_token: String,
}

impl Session {
    fn alive(&self) -> bool {
        self.socket == SocketState::Connected || self.last_seen.elapsed() <= PLUGIN_ALIVE
    }

    fn ring(&self) {
        if let Some(doorbell) = &self.doorbell {
            // A closed channel means the doorbell task already gave up; the
            // plugin's own polling picks the command up instead.
            let _ = doorbell.send(());
        }
    }
}

type ReplySender = oneshot::Sender<Result<CommandReply, BridgeError>>;

struct Entry {
    session: u64,
    action: String,
    args: Value,
    timeout: Duration,
    claimed_at: Option<Instant>,
    cancel_requested: bool,
    reply: Option<ReplySender>,
    progress: Option<mpsc::UnboundedSender<Progress>>,
}

#[derive(Default)]
struct Inner {
    session: Option<Session>,
    /// The latest bridge start seen since the backend started, kept after an
    /// unregister so an older bridge cannot slip in once the newer one stops.
    newest_start: Option<f64>,
    next_session: u64,
    next_command: u64,
    queue: VecDeque<u64>,
    entries: HashMap<u64, Entry>,
}

impl Inner {
    fn session_for(&mut self, token: &str) -> Option<&mut Session> {
        self.session
            .as_mut()
            .filter(|s| constant_time_eq(s.token.as_bytes(), token.as_bytes()))
    }

    /// Ends every command that belongs to `session` with `error`.
    fn fail_session(&mut self, session: u64, error: &BridgeError) {
        let ids: Vec<u64> = self
            .entries
            .iter()
            .filter(|(_, e)| e.session == session)
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            if let Some(mut entry) = self.entries.remove(&id) {
                if let Some(reply) = entry.reply.take() {
                    let _ = reply.send(Err(error.clone()));
                }
            }
        }
        self.queue.retain(|id| self.entries.contains_key(id));
    }

    fn remove(&mut self, id: u64) -> Option<Entry> {
        self.queue.retain(|queued| *queued != id);
        self.entries.remove(&id)
    }
}

/// Shared, process-global: one Lightroom plugin at a time talks to a backend.
#[derive(Default)]
pub struct AgentBridge {
    inner: Mutex<Inner>,
}

/// Snapshot for `GET /v1/agent/bridge` and the MCP status tool.
#[derive(Debug, Clone, Serialize)]
pub struct BridgeStatus {
    pub connected: bool,
    pub socket: SocketState,
    pub session: Option<u64>,
    pub plugin: Option<PluginInfo>,
    pub registered_at: Option<String>,
    pub last_seen_seconds: Option<u64>,
    pub queued: usize,
    pub in_progress: usize,
    pub commands_done: u64,
}

impl AgentBridge {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers the plugin. A plugin that passes the token it already holds
    /// keeps its session — it only bound a new socket — and its in-flight
    /// commands survive. Any other registration is a new plugin instance (a
    /// reload, a second Lightroom) and replaces the old session; commands the
    /// old one had not finished end with [`BridgeError::Replaced`].
    pub fn register(
        self: &Arc<Self>,
        port: Option<u16>,
        previous_token: Option<&str>,
        info: PluginInfo,
    ) -> Registration {
        self.register_with(RegisterRequest {
            port,
            previous_token: previous_token.map(str::to_string),
            info,
            ..Default::default()
        })
        .expect("a registration without a start time is never superseded")
    }

    /// [`AgentBridge::register`] with everything a plugin can send,
    /// including the agent token it keeps.
    pub fn register_with(
        self: &Arc<Self>,
        request: RegisterRequest,
    ) -> Result<Registration, Superseded> {
        let RegisterRequest {
            port,
            previous_token,
            info,
            mcp_token,
            rotate_mcp_token,
            started_at,
        } = request;
        let mcp_token = match mcp_token.as_deref().map(str::trim) {
            Some(kept) if !rotate_mcp_token && valid_token(kept) => kept.to_string(),
            _ => mint_token(),
        };

        let mut inner = self.inner.lock().unwrap();

        let resumable = previous_token
            .as_deref()
            .filter(|t| !t.is_empty())
            .is_some_and(|t| inner.session_for(t).is_some());

        if let Some(started) = started_at {
            // A millisecond of slack: the plugin sends what it read from its
            // clock, and the same bridge must never count as older than
            // itself.
            if !resumable
                && inner
                    .newest_start
                    .is_some_and(|newest| started < newest - 0.001)
            {
                log::info!(
                    "Agent bridge: refused a registration from a bridge started before the current one"
                );
                return Err(Superseded);
            }
            inner.newest_start = Some(inner.newest_start.map_or(started, |n| n.max(started)));
        }

        let (session_id, token) = if resumable {
            let session = inner.session.as_mut().unwrap();
            // Dropping the old sender stops the old doorbell task first.
            session.doorbell = None;
            session.info = info;
            session.port = port;
            session.last_seen = Instant::now();
            session.mcp_token = mcp_token.clone();
            (session.id, session.token.clone())
        } else {
            if let Some(old) = inner.session.take() {
                log::info!(
                    "Agent bridge: plugin session {} replaced by a new registration",
                    old.id
                );
                inner.fail_session(old.id, &BridgeError::Replaced);
            }
            inner.next_session += 1;
            let id = inner.next_session;
            let token = mint_token();
            inner.session = Some(Session {
                id,
                token: token.clone(),
                info,
                port,
                registered_at: chrono::Utc::now(),
                last_seen: Instant::now(),
                socket: SocketState::None,
                doorbell: None,
                doorbell_gen: 0,
                commands_done: 0,
                mcp_token: mcp_token.clone(),
            });
            (id, token)
        };

        // Commands may already be waiting (queued before a re-register); ring
        // once so the plugin picks them up as soon as we connect.
        let pending = !inner.queue.is_empty();
        let session = inner.session.as_mut().unwrap();
        session.doorbell_gen += 1;
        if let Some(port) = port {
            let (tx, rx) = mpsc::unbounded_channel();
            session.doorbell = Some(tx);
            session.socket = SocketState::Connecting;
            let doorbell = Doorbell {
                bridge: Arc::downgrade(self),
                session: session_id,
                generation: session.doorbell_gen,
            };
            tokio::spawn(run_doorbell(doorbell, port, rx, pending));
        } else {
            session.socket = SocketState::None;
        }

        log::info!(
            "Agent bridge: plugin registered (session {}, port {}, resumed {resumable})",
            session.id,
            port.map_or_else(|| "none".to_string(), |p| p.to_string())
        );
        Ok(Registration {
            token,
            session: session_id,
            resumed: resumable,
            mcp_token,
        })
    }

    /// Checks the agent token a request to `/mcp` presented.
    pub fn check_mcp_token(&self, presented: Option<&str>) -> McpAccess {
        let inner = self.inner.lock().unwrap();
        let Some(session) = inner.session.as_ref() else {
            return McpAccess::NoToken;
        };
        match presented {
            Some(token) if constant_time_eq(token.as_bytes(), session.mcp_token.as_bytes()) => {
                McpAccess::Granted
            }
            _ => McpAccess::Denied,
        }
    }

    /// The plugin is stopping on purpose. Returns false for an unknown token.
    pub fn unregister(&self, token: &str) -> bool {
        let mut inner = self.inner.lock().unwrap();
        if inner.session_for(token).is_none() {
            return false;
        }
        let session = inner.session.take().unwrap();
        inner.fail_session(session.id, &BridgeError::Stopped);
        log::info!(
            "Agent bridge: plugin session {} stopped in Lightroom",
            session.id
        );
        true
    }

    /// Hands every queued command to the plugin, in order. `None` for an
    /// unknown token, which tells the plugin to register again — the backend
    /// was restarted underneath it.
    pub fn claim(&self, token: &str) -> Option<Vec<ClaimedCommand>> {
        let mut inner = self.inner.lock().unwrap();
        let session_id = {
            let session = inner.session_for(token)?;
            session.last_seen = Instant::now();
            session.id
        };
        let ids: Vec<u64> = inner.queue.drain(..).collect();
        let now = Instant::now();
        let mut claimed = Vec::with_capacity(ids.len());
        for id in ids {
            let Some(entry) = inner.entries.get_mut(&id) else {
                continue;
            };
            if entry.session != session_id {
                continue;
            }
            entry.claimed_at = Some(now);
            claimed.push(ClaimedCommand {
                id,
                action: entry.action.clone(),
                args: entry.args.clone(),
                timeout_seconds: entry.timeout.as_secs(),
            });
        }
        Some(claimed)
    }

    /// Delivers the plugin's answer. Returns false when nobody is waiting any
    /// more (the caller timed out or went away), so the plugin can log that
    /// its work was not reported.
    pub fn complete(&self, token: &str, id: u64, outcome: PluginOutcome) -> Option<bool> {
        let mut inner = self.inner.lock().unwrap();
        let session_id = {
            let session = inner.session_for(token)?;
            session.last_seen = Instant::now();
            session.commands_done += 1;
            session.id
        };
        let Some(mut entry) = inner.entries.remove(&id) else {
            return Some(false);
        };
        if entry.session != session_id {
            return Some(false);
        }
        let result = if outcome.ok {
            Ok(CommandReply {
                result: outcome.result,
                warnings: outcome.warnings,
            })
        } else {
            Err(match outcome.refused.as_deref() {
                Some("changes") => BridgeError::ChangesNotAllowed,
                Some("dev_tools") => BridgeError::DevToolsDisabled,
                _ => BridgeError::Plugin {
                    message: outcome.error.filter(|e| !e.is_empty()).unwrap_or_else(|| {
                        format!("Lightroom could not run \"{}\".", entry.action)
                    }),
                    warnings: outcome.warnings,
                },
            })
        };
        let delivered = entry
            .reply
            .take()
            .is_some_and(|reply| reply.send(result).is_ok());
        Some(delivered)
    }

    /// Records progress on a command and answers whether the caller has
    /// given up on it, so the plugin can stop between photos.
    pub fn progress(&self, token: &str, id: u64, progress: Progress) -> Option<bool> {
        let mut inner = self.inner.lock().unwrap();
        let session_id = {
            let session = inner.session_for(token)?;
            session.last_seen = Instant::now();
            session.id
        };
        let Some(entry) = inner.entries.get_mut(&id) else {
            // Nobody waits for it any more: tell the plugin to stop.
            return Some(true);
        };
        if entry.session != session_id {
            return Some(true);
        }
        if let Some(tx) = &entry.progress {
            let _ = tx.send(progress);
        }
        Some(entry.cancel_requested)
    }

    /// True when a command sent now would be picked up.
    pub fn is_connected(&self) -> bool {
        let inner = self.inner.lock().unwrap();
        inner.session.as_ref().is_some_and(Session::alive)
    }

    /// What the connected plugin said about itself, if one is connected.
    pub fn plugin_info(&self) -> Option<PluginInfo> {
        let inner = self.inner.lock().unwrap();
        inner
            .session
            .as_ref()
            .filter(|s| s.alive())
            .map(|s| s.info.clone())
    }

    pub fn status(&self) -> BridgeStatus {
        let inner = self.inner.lock().unwrap();
        let in_progress = inner
            .entries
            .values()
            .filter(|e| e.claimed_at.is_some())
            .count();
        match &inner.session {
            Some(s) => BridgeStatus {
                connected: s.alive(),
                socket: s.socket,
                session: Some(s.id),
                plugin: Some(s.info.clone()),
                registered_at: Some(s.registered_at.to_rfc3339()),
                last_seen_seconds: Some(s.last_seen.elapsed().as_secs()),
                queued: inner.queue.len(),
                in_progress,
                commands_done: s.commands_done,
            },
            None => BridgeStatus {
                connected: false,
                socket: SocketState::None,
                session: None,
                plugin: None,
                registered_at: None,
                last_seen_seconds: None,
                queued: inner.queue.len(),
                in_progress,
                commands_done: 0,
            },
        }
    }

    /// Runs one command in Lightroom and waits for its answer.
    ///
    /// Fails fast — without queueing — when no plugin is connected, when the
    /// command changes photos and the plugin is read-only, or when it needs
    /// developer commands the plugin does not offer. A queued command that the
    /// plugin stops claiming (Lightroom quit) also fails fast, instead of
    /// waiting out the whole timeout.
    pub async fn call(
        &self,
        action: &str,
        args: Value,
        options: CallOptions,
    ) -> Result<CommandReply, BridgeError> {
        let (tx, mut rx) = oneshot::channel();
        let (id, session_id) = {
            let mut inner = self.inner.lock().unwrap();
            let Some(session) = inner.session.as_ref().filter(|s| s.alive()) else {
                return Err(BridgeError::NotConnected);
            };
            if options.changes && !session.info.allow_changes {
                return Err(BridgeError::ChangesNotAllowed);
            }
            if options.dev && !session.info.dev_tools {
                return Err(BridgeError::DevToolsDisabled);
            }
            if inner.queue.len() >= MAX_QUEUED {
                return Err(BridgeError::Busy);
            }
            let session_id = session.id;
            inner.next_command += 1;
            let id = inner.next_command;
            inner.entries.insert(
                id,
                Entry {
                    session: session_id,
                    action: action.to_string(),
                    args,
                    timeout: options.timeout,
                    claimed_at: None,
                    cancel_requested: false,
                    reply: Some(tx),
                    progress: options.progress,
                },
            );
            inner.queue.push_back(id);
            inner.session.as_ref().unwrap().ring();
            (id, session_id)
        };

        // Ends the entry if this future is dropped before an answer came —
        // the MCP client cancelled the tool call, or gave up waiting.
        let mut guard = PendingGuard {
            bridge: self,
            id,
            armed: true,
        };

        let started = Instant::now();
        let mut watchdog = tokio::time::interval(Duration::from_millis(500));
        watchdog.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                answer = &mut rx => {
                    guard.armed = false;
                    return answer.unwrap_or(Err(BridgeError::NotConnected));
                }
                _ = watchdog.tick() => {
                    let mut inner = self.inner.lock().unwrap();
                    let claimed = inner
                        .entries
                        .get(&id)
                        .is_some_and(|e| e.claimed_at.is_some());
                    if started.elapsed() >= options.timeout {
                        // Why an unclaimed command waited: another one of
                        // this session is still running.
                        let busy_with = if claimed {
                            None
                        } else {
                            inner
                                .entries
                                .iter()
                                .filter(|(other, e)| **other != id && e.session == session_id)
                                .find_map(|(_, e)| {
                                    e.claimed_at
                                        .map(|at| (e.action.clone(), at.elapsed().as_secs()))
                                })
                        };
                        // A claimed command keeps running in Lightroom; with
                        // its entry gone, a late answer finds nobody and the
                        // plugin's next progress report is told to stop.
                        inner.remove(id);
                        guard.armed = false;
                        return Err(BridgeError::TimedOut {
                            action: action.to_string(),
                            seconds: options.timeout.as_secs(),
                            claimed,
                            busy_with,
                        });
                    }
                    let alive = inner
                        .session
                        .as_ref()
                        .is_some_and(|s| s.id == session_id && s.alive());
                    if !alive && !claimed {
                        inner.remove(id);
                        guard.armed = false;
                        return Err(BridgeError::NotConnected);
                    }
                }
            }
        }
    }

    fn abandon(&self, id: u64) {
        let mut inner = self.inner.lock().unwrap();
        let claimed = inner
            .entries
            .get(&id)
            .is_some_and(|e| e.claimed_at.is_some());
        if claimed {
            // Already running in Lightroom: it cannot be pulled back, but the
            // plugin checks this flag between photos when it reports progress.
            if let Some(entry) = inner.entries.get_mut(&id) {
                entry.cancel_requested = true;
                entry.reply = None;
                entry.progress = None;
            }
        } else {
            inner.remove(id);
        }
    }

    fn set_socket(&self, doorbell: &Doorbell, state: SocketState) {
        let mut inner = self.inner.lock().unwrap();
        let session_id = doorbell.session;
        if let Some(session) = inner
            .session
            .as_mut()
            .filter(|s| s.id == session_id && s.doorbell_gen == doorbell.generation)
        {
            if session.socket != state {
                log::info!(
                    "Agent bridge: doorbell socket {state:?} (session {})",
                    session.id
                );
            }
            session.socket = state;
            if state == SocketState::Connected {
                session.last_seen = Instant::now();
            }
        }
    }
}

struct PendingGuard<'a> {
    bridge: &'a AgentBridge,
    id: u64,
    armed: bool,
}

impl Drop for PendingGuard<'_> {
    fn drop(&mut self) {
        if self.armed {
            self.bridge.abandon(self.id);
        }
    }
}

/// Which doorbell task this is, for reporting its socket state back.
struct Doorbell {
    bridge: Weak<AgentBridge>,
    session: u64,
    generation: u64,
}

impl Doorbell {
    fn report(&self, state: SocketState) -> bool {
        match self.bridge.upgrade() {
            Some(bridge) => {
                bridge.set_socket(self, state);
                true
            }
            None => false,
        }
    }
}

/// Connects to the plugin's `LrSocket` and rings it: `wake` per queued
/// command, `ping` on a timer. Ends when the session drops its sender, when
/// the plugin closes the connection, or when a write fails.
async fn run_doorbell(
    doorbell: Doorbell,
    port: u16,
    mut rings: mpsc::UnboundedReceiver<()>,
    ring_first: bool,
) {
    let mut stream = None;
    for attempt in 1..=CONNECT_ATTEMPTS {
        match tokio::net::TcpStream::connect(("127.0.0.1", port)).await {
            Ok(s) => {
                stream = Some(s);
                break;
            }
            Err(e) => {
                log::debug!("Agent bridge: doorbell connect attempt {attempt} to port {port}: {e}");
                tokio::time::sleep(CONNECT_RETRY).await;
            }
        }
    }
    let Some(stream) = stream else {
        log::warn!(
            "Agent bridge: could not connect to the plugin's socket on port {port}; \
             the plugin keeps working by asking over HTTP"
        );
        doorbell.report(SocketState::Down);
        return;
    };
    let _ = stream.set_nodelay(true);
    if !doorbell.report(SocketState::Connected) {
        return;
    }

    let (mut reader, mut writer) = stream.into_split();
    // LrSocket's receive side never writes to us, so the only thing a read
    // can report is the plugin closing the connection.
    let closed = async move {
        let mut buf = [0u8; 64];
        loop {
            match reader.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(_) => continue,
            }
        }
    };
    tokio::pin!(closed);

    let mut ping = tokio::time::interval(PING_INTERVAL);
    ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    if ring_first && writer.write_all(b"wake\n").await.is_err() {
        doorbell.report(SocketState::Down);
        return;
    }
    loop {
        let line: &[u8] = tokio::select! {
            ring = rings.recv() => match ring {
                Some(()) => b"wake\n",
                // The session ended or moved to a new socket.
                None => return,
            },
            _ = ping.tick() => b"ping\n",
            _ = &mut closed => break,
        };
        if writer.write_all(line).await.is_err() {
            break;
        }
    }
    doorbell.report(SocketState::Down);
}

/// A fresh random token: two v4 UUIDs, 244 random bits from the operating
/// system's generator, as 64 hex digits.
fn mint_token() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

/// What the backend accepts as a kept agent token: long enough to be a real
/// secret, and nothing that cannot travel in an HTTP header.
fn valid_token(token: &str) -> bool {
    (32..=256).contains(&token.len())
        && token
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Compares tokens without leaking how much of a guess matched through
/// timing. Overkill for loopback, but free.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tokio::io::AsyncBufReadExt;

    fn info(allow_changes: bool) -> PluginInfo {
        PluginInfo {
            plugin_version: "9.9.9".into(),
            catalog_id: "cat-1".into(),
            allow_changes,
            ..Default::default()
        }
    }

    fn quick() -> CallOptions {
        CallOptions::read(Duration::from_secs(5))
    }

    #[tokio::test]
    async fn call_without_a_plugin_fails_fast() {
        let bridge = Arc::new(AgentBridge::new());
        let err = bridge.call("status", json!({}), quick()).await.unwrap_err();
        assert_eq!(err, BridgeError::NotConnected);
        assert!(err.to_string().contains("Enable the agent bridge"));
    }

    #[tokio::test]
    async fn a_claimed_command_is_answered_with_the_plugin_result() {
        let bridge = Arc::new(AgentBridge::new());
        let reg = bridge.register(None, None, info(true));

        let worker = {
            let bridge = bridge.clone();
            let token = reg.token.clone();
            tokio::spawn(async move {
                loop {
                    let claimed = bridge.claim(&token).unwrap();
                    if let Some(cmd) = claimed.first() {
                        assert_eq!(cmd.action, "status");
                        assert_eq!(cmd.args["x"], 1);
                        let delivered = bridge.complete(
                            &token,
                            cmd.id,
                            PluginOutcome {
                                ok: true,
                                result: json!({"photos": 3}),
                                warnings: vec!["one photo is offline".into()],
                                ..Default::default()
                            },
                        );
                        assert_eq!(delivered, Some(true));
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
        };

        let reply = bridge
            .call("status", json!({"x": 1}), quick())
            .await
            .unwrap();
        worker.await.unwrap();
        assert_eq!(reply.result["photos"], 3);
        assert_eq!(reply.warnings, vec!["one photo is offline".to_string()]);
        assert_eq!(bridge.status().commands_done, 1);
    }

    #[tokio::test]
    async fn plugin_errors_and_refusals_keep_their_meaning() {
        let bridge = Arc::new(AgentBridge::new());
        let reg = bridge.register(None, None, info(true));
        for (outcome, expected) in [
            (
                PluginOutcome {
                    ok: false,
                    error: Some("3 photos are not in this catalog".into()),
                    ..Default::default()
                },
                BridgeError::Plugin {
                    message: "3 photos are not in this catalog".into(),
                    warnings: vec![],
                },
            ),
            (
                PluginOutcome {
                    ok: false,
                    refused: Some("changes".into()),
                    ..Default::default()
                },
                BridgeError::ChangesNotAllowed,
            ),
        ] {
            let bridge2 = bridge.clone();
            let token = reg.token.clone();
            let answer = tokio::spawn(async move {
                loop {
                    if let Some(cmd) = bridge2.claim(&token).unwrap().first() {
                        bridge2.complete(&token, cmd.id, outcome);
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            });
            let err = bridge.call("x", json!({}), quick()).await.unwrap_err();
            answer.await.unwrap();
            assert_eq!(err, expected);
        }
    }

    #[tokio::test]
    async fn read_only_plugin_refuses_changes_before_queueing() {
        let bridge = Arc::new(AgentBridge::new());
        bridge.register(None, None, info(false));
        let err = bridge
            .call(
                "update_photos",
                json!({}),
                CallOptions::write(Duration::from_secs(1)),
            )
            .await
            .unwrap_err();
        assert_eq!(err, BridgeError::ChangesNotAllowed);
        assert_eq!(bridge.status().queued, 0);
    }

    #[tokio::test]
    async fn unregister_fails_pending_commands_as_stopped() {
        let bridge = Arc::new(AgentBridge::new());
        let reg = bridge.register(None, None, info(true));
        let caller = {
            let bridge = bridge.clone();
            tokio::spawn(async move { bridge.call("status", json!({}), quick()).await })
        };
        // Let the call queue up first.
        while bridge.status().queued == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(bridge.unregister(&reg.token));
        assert_eq!(caller.await.unwrap().unwrap_err(), BridgeError::Stopped);
        assert!(!bridge.is_connected());
    }

    #[tokio::test]
    async fn a_new_plugin_instance_replaces_the_old_but_a_resume_keeps_it() {
        let bridge = Arc::new(AgentBridge::new());
        let first = bridge.register(None, None, info(true));

        let resumed = bridge.register(None, Some(&first.token), info(true));
        assert!(resumed.resumed);
        assert_eq!(resumed.token, first.token);
        assert_eq!(resumed.session, first.session);

        let caller = {
            let bridge = bridge.clone();
            tokio::spawn(async move { bridge.call("status", json!({}), quick()).await })
        };
        while bridge.status().queued == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let second = bridge.register(None, Some("not-the-token"), info(true));
        assert!(!second.resumed);
        assert_ne!(second.token, first.token);
        assert_eq!(caller.await.unwrap().unwrap_err(), BridgeError::Replaced);
        // The old token is dead.
        assert!(bridge.claim(&first.token).is_none());
    }

    #[tokio::test]
    async fn an_unclaimed_command_times_out_without_running_later() {
        let bridge = Arc::new(AgentBridge::new());
        let reg = bridge.register(None, None, info(true));
        // Keep the plugin "alive" without claiming anything.
        let err = bridge
            .call(
                "status",
                json!({}),
                CallOptions::read(Duration::from_secs(1)),
            )
            .await
            .unwrap_err();
        assert!(matches!(err, BridgeError::TimedOut { claimed: false, .. }));
        assert!(bridge.claim(&reg.token).unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_command_waiting_behind_a_long_one_says_what_lightroom_is_busy_with() {
        let bridge = Arc::new(AgentBridge::new());
        let reg = bridge.register(None, None, info(true));
        let long = {
            let bridge = bridge.clone();
            tokio::spawn(async move {
                bridge
                    .call(
                        "analyze_photos",
                        json!({}),
                        CallOptions::read(Duration::from_secs(30)),
                    )
                    .await
            })
        };
        // The plugin claims the long command and works on it.
        while bridge.claim(&reg.token).unwrap().is_empty() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let err = bridge
            .call(
                "status",
                json!({}),
                CallOptions::read(Duration::from_secs(1)),
            )
            .await
            .unwrap_err();
        match &err {
            BridgeError::TimedOut {
                busy_with: Some((busy, _)),
                claimed: false,
                ..
            } => assert_eq!(busy, "analyze_photos"),
            other => panic!("unexpected {other:?}"),
        }
        assert!(err
            .to_string()
            .contains("still working on \"analyze_photos\""));
        long.abort();
    }

    #[tokio::test]
    async fn a_dropped_call_marks_a_running_command_for_cancellation() {
        let bridge = Arc::new(AgentBridge::new());
        let reg = bridge.register(None, None, info(true));
        let caller = {
            let bridge = bridge.clone();
            tokio::spawn(async move { bridge.call("apply", json!({}), quick()).await })
        };
        let cmd = loop {
            if let Some(cmd) = bridge.claim(&reg.token).unwrap().into_iter().next() {
                break cmd;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        };
        let progress = Progress {
            done: 1.0,
            total: Some(4.0),
            message: None,
        };
        assert_eq!(
            bridge.progress(&reg.token, cmd.id, progress.clone()),
            Some(false)
        );
        caller.abort();
        let _ = caller.await;
        assert_eq!(bridge.progress(&reg.token, cmd.id, progress), Some(true));
        // The late result finds nobody waiting.
        assert_eq!(
            bridge.complete(&reg.token, cmd.id, PluginOutcome::default()),
            Some(false)
        );
    }

    #[tokio::test]
    async fn progress_reports_reach_the_caller() {
        let bridge = Arc::new(AgentBridge::new());
        let reg = bridge.register(None, None, info(true));
        let (tx, mut rx) = mpsc::unbounded_channel();
        let caller = {
            let bridge = bridge.clone();
            tokio::spawn(async move {
                let mut options = quick();
                options.progress = Some(tx);
                bridge.call("apply", json!({}), options).await
            })
        };
        let cmd = loop {
            if let Some(cmd) = bridge.claim(&reg.token).unwrap().into_iter().next() {
                break cmd;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        };
        bridge.progress(
            &reg.token,
            cmd.id,
            Progress {
                done: 2.0,
                total: Some(5.0),
                message: Some("Rating photos".into()),
            },
        );
        let seen = rx.recv().await.unwrap();
        assert_eq!(seen.done, 2.0);
        assert_eq!(seen.message.as_deref(), Some("Rating photos"));
        bridge.complete(
            &reg.token,
            cmd.id,
            PluginOutcome {
                ok: true,
                ..Default::default()
            },
        );
        assert!(caller.await.unwrap().is_ok());
    }

    /// Plays the plugin's `LrSocket`: a listener the bridge connects to,
    /// reading the lines it rings.
    #[tokio::test]
    async fn the_doorbell_connects_rings_on_each_command_and_reports_a_close() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let bridge = Arc::new(AgentBridge::new());
        let reg = bridge.register(Some(port), None, info(true));

        let (socket, _) = listener.accept().await.unwrap();
        let mut lines = tokio::io::BufReader::new(socket).lines();

        // Wait until the bridge counts the socket as connected.
        for _ in 0..100 {
            if bridge.status().socket == SocketState::Connected {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(bridge.status().socket, SocketState::Connected);

        let caller = {
            let bridge = bridge.clone();
            tokio::spawn(async move { bridge.call("status", json!({}), quick()).await })
        };
        // The first ping fires immediately; skip pings until the wake.
        loop {
            let line = lines.next_line().await.unwrap().unwrap();
            if line == "wake" {
                break;
            }
            assert_eq!(line, "ping");
        }
        let cmd = bridge.claim(&reg.token).unwrap().remove(0);
        bridge.complete(
            &reg.token,
            cmd.id,
            PluginOutcome {
                ok: true,
                ..Default::default()
            },
        );
        assert!(caller.await.unwrap().is_ok());

        // Lightroom closes the socket: the bridge notices.
        drop(lines);
        for _ in 0..100 {
            if bridge.status().socket == SocketState::Down {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(bridge.status().socket, SocketState::Down);
        // Still "connected" for a while on the strength of the last claim,
        // which is what lets the plugin fall back to polling seamlessly.
        assert!(bridge.is_connected());
    }

    #[tokio::test]
    async fn the_agent_token_is_minted_kept_and_rotated() {
        let bridge = Arc::new(AgentBridge::new());
        assert_eq!(bridge.check_mcp_token(Some("x")), McpAccess::NoToken);

        // First registration: the plugin has none, so the backend makes one.
        let first = bridge
            .register_with(RegisterRequest {
                info: info(true),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(first.mcp_token.len(), 64);
        assert_eq!(
            bridge.check_mcp_token(Some(&first.mcp_token)),
            McpAccess::Granted
        );
        assert_eq!(bridge.check_mcp_token(None), McpAccess::Denied);
        assert_eq!(bridge.check_mcp_token(Some("wrong")), McpAccess::Denied);

        // After a restart the plugin brings the token it kept.
        let kept = bridge
            .register_with(RegisterRequest {
                info: info(true),
                mcp_token: Some(first.mcp_token.clone()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(kept.mcp_token, first.mcp_token);

        // Something too short to be a secret is replaced, not adopted.
        let weak = bridge
            .register_with(RegisterRequest {
                info: info(true),
                mcp_token: Some("1234".into()),
                ..Default::default()
            })
            .unwrap();
        assert_ne!(weak.mcp_token, "1234");

        // The user asked for a new one: the old one stops working.
        let rotated = bridge
            .register_with(RegisterRequest {
                info: info(true),
                previous_token: Some(weak.token.clone()),
                mcp_token: Some(weak.mcp_token.clone()),
                rotate_mcp_token: true,
                ..Default::default()
            })
            .unwrap();
        assert!(rotated.resumed);
        assert_ne!(rotated.mcp_token, weak.mcp_token);
        assert_eq!(
            bridge.check_mcp_token(Some(&weak.mcp_token)),
            McpAccess::Denied
        );

        // Stopping the bridge closes MCP entirely.
        bridge.unregister(&rotated.token);
        assert_eq!(
            bridge.check_mcp_token(Some(&rotated.mcp_token)),
            McpAccess::NoToken
        );
    }

    #[tokio::test]
    async fn the_newest_bridge_wins_and_an_older_one_is_told_to_stop() {
        let bridge = Arc::new(AgentBridge::new());
        let start = |t: f64| RegisterRequest {
            info: info(true),
            started_at: Some(t),
            ..Default::default()
        };
        let old = bridge.register_with(start(100.0)).unwrap();
        // The plugin was reloaded: a bridge started later takes over.
        let new = bridge.register_with(start(200.0)).unwrap();
        assert_ne!(old.session, new.session);

        // The old loop notices it was replaced and registers again: refused.
        assert!(matches!(
            bridge.register_with(start(100.0)),
            Err(Superseded)
        ));
        // Still refused once the newer one stopped.
        bridge.unregister(&new.token);
        assert!(matches!(
            bridge.register_with(start(100.0)),
            Err(Superseded)
        ));
        // The newer bridge started again (the user re-enabled it): fine.
        assert!(bridge.register_with(start(300.0)).is_ok());

        // A plugin build that sends no start time is not judged at all.
        assert!(bridge.register_with(RegisterRequest::default()).is_ok());
    }

    #[test]
    fn tokens_compare_in_full() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"abcd"));
    }
}
