# Dev: Agent Bridge and MCP Endpoint

How an AI agent drives Lightroom through LrGeniusAI: the MCP endpoint in the
backend, the agent bridge between the backend and the plugin, the commands the
plugin runs, and how all of it is tested — including how an agent tests the
plugin itself. User-facing guide: [Help: AI Agents](Help-AI-Agents). Route
reference: [Backend API](Dev-Backend-API).

---

## The pieces

```
Claude Code / Claude Desktop (via `geniusai-server mcp`) / LM Studio / Cursor
        │  MCP, Streamable HTTP, `Authorization: Bearer <agent token>`
        ▼
lrg-mcp  ── /mcp on the backend's port (19819)
  ├─ index tools ─────────► the REST routes, called in-process (search, faces, cull, stats)
  └─ Lightroom tools ─────► lrg-api::agent_bridge  ──command queue──┐
                                                                    │
           LrSocket "wake"/"ping" (doorbell, backend → plugin)      │
plugin: AgentBridge.lua ◄──────────────────────────────────────────┤
        GET /v1/agent/commands, POST …/result, …/progress (HTTP) ──┘
        AgentCommands.lua runs each command against the catalog (SDK)
```

| Where | What |
|---|---|
| `server-rs/crates/lrg-mcp/src/lib.rs` | Mounts the rmcp `StreamableHttpService` at `/mcp` behind the agent-token check (Host check on, every browser `Origin` refused) |
| `…/lrg-mcp/src/server.rs` | Shared helpers (photo resolution, result sets, progress forwarding), server instructions, prompts |
| `…/lrg-mcp/src/tools_index.rs` | Tools over the backend's index |
| `…/lrg-mcp/src/tools_lightroom.rs` | Tools that run in Lightroom through the bridge |
| `…/lrg-mcp/src/tools_dev.rs` | Testing and developer tools |
| `…/lrg-mcp/src/api.rs` | In-process calls into the REST router (`tower::ServiceExt::oneshot`) |
| `…/lrg-mcp/src/sets.rs` | Result sets (`s1`, `s2`, …) |
| `…/lrg-mcp/src/stdio.rs` | `geniusai-server mcp`: stdio ↔ HTTP proxy for Claude Desktop |
| `server-rs/crates/lrg-api/src/agent_bridge.rs` | Command queue, sessions, doorbell task |
| `…/lrg-api/src/routes/agent.rs` | `/v1/agent/*` |
| `plugin/…/AgentBridge.lua` | Socket, registration (with the agent token), claim/run loop, the activity bar and its off switch |
| `plugin/…/AgentCommands.lua` | One handler per command |
| `plugin/…/PluginTests.lua` | In-Lightroom test suites (`run_plugin_tests`, Help menu) |
| `plugin/…/Secrets.lua` | Keeps the agent token in LrPasswords (`Secrets.getMcpToken`/`setMcpToken`), apart from the provider keys |
| `plugin/…/PluginInfoDialogSections.lua` | *AI Agents (MCP)* in the Plug-in Manager: enable switch, permissions, masked token, setup copy buttons |

The index tools call the REST routes rather than the functions behind them on
purpose: a tool then gets exactly what the plugin's own menu item gets — the
same catalog scoping, the same warnings.

---

## The bridge protocol

**Why a doorbell plus HTTP, not two sockets.** lightroom-mcp, the other
open-source Lightroom MCP, uses one LrSocket per direction. Its issue history
documents that the *send* mode does not reliably notice a dropped client on
Windows and keeps writing into a dead connection (their #110), and that
`onClosed` does not always fire there (#134). The receive mode is the
well-behaved half. So the bridge uses only that, as a doorbell, and moves
everything with content to HTTP, which the plugin already uses for everything
else. That also removes message-size questions (previews travel as a file
path), and a stray writer to the port can at most make the plugin ask for work.

1. **Register.** `AgentBridge.lua` binds `LrSocket` in receive mode with
   `port = 0`. `onConnecting` reports the port, and the plugin posts it to
   `POST /v1/agent/bridge` with its catalog, versions, permissions and the
   agent token it keeps. It gets a session token, the ping interval and the
   agent token back. The backend makes a new agent token when the plugin has
   none (first run) or asks for one (*New token...*), and the plugin stores it
   with `Secrets.setMcpToken`.
2. **Doorbell.** The backend connects to the port. It writes `wake` when a
   command is queued and `ping` every `PING_INTERVAL` (5 s).
3. **Claim.** On `wake`, or every 5 s as a safety net (every 1 s without a
   socket), the plugin calls `GET /v1/agent/commands`.
4. **Run.** Commands run one at a time on the bridge task
   (`AgentCommands.run`). Changes happen inside a single `withWriteAccessDo`
   named after the command. Long commands post progress, and the progress
   answer says when the caller gave up.
5. **Answer.** `POST /v1/agent/commands/<id>/result` carries
   `{ok, result, error, warnings, refused}`. The backend completes the waiting
   MCP call.

**Liveness and recovery**

| Situation | What happens |
|---|---|
| Backend restarts | The next claim gets 401 `reregister`; the plugin registers again, and its old socket reconnects |
| Socket connected but silent for 3 pings (Windows `onClosed` gap) | The plugin closes it, binds a fresh one and re-registers with the new port |
| Socket cannot be opened | Registration without `port`; the plugin polls every second |
| "Reload Plug-in" | The reloaded plugin gets a fresh Lua environment, but the old instance's loop keeps running (LrC 15.6) and cannot see the new globals. Every start writes its start time to the plugin prefs (`agentBridgeOwner`), which all instances share, and a loop that no longer finds its own exits. The backend enforces the same rule: a registration whose `started_at` is older than the newest seen gets 409 `superseded`, and that loop stops. Without both, the two loops took turns replacing each other's session every few seconds |
| Socket bound while Lightroom starts | Such a socket may never report its port (LrC 15.6). The bridge registers first without a port, binds once the backend has answered, and replaces a socket that reports no port within 5 s, with growing pauses |
| Bridge started from `Init.lua` | `postAsyncTaskWithContext`, not `startAsyncTask`: the latter dies with `Init.lua`'s context (lightroom-mcp #128) |
| User presses X on the activity bar | The loop unregisters (`DELETE /v1/agent/bridge`); pending calls fail as "stopped by the user", and `/mcp` refuses everything until the bridge runs again |
| Lightroom quits | `ShutdownApp.lua` unregisters first |
| Command not claimed and plugin gone | The MCP call fails fast with "not connected" instead of waiting out its timeout |
| Command waiting behind a long one | Commands run one at a time, so a second call during `analyze_photos` waits its turn; if it times out, the error names what Lightroom is busy with and for how long |
| MCP client cancels a call | Unclaimed: removed from the queue. Running: the next progress post is answered `cancel: true` |

LrSocket callbacks run where nothing may yield, so they only set flags. All
HTTP happens on the loop.

**Visibility.** The bridge starts with Lightroom (`prefs.agentBridgeEnabled`,
on by default) and shows nothing while idle. The first command opens an
activity progress bar, *LrGeniusAI: AI agent at work*, which captions each
command and closes 8 s after the last one. Its X is the off switch.

---

## Command contract

The Rust tool validates arguments and resolves result sets. The Lua command
does the catalog work. Keep both sides in step: an action the plugin does not
know comes back as "update the plugin".

| MCP tool | Bridge action | Changes photos | Notes |
|---|---|---|---|
| `get_status` (part) | `status` | – | catalog, selection, sources, versions |
| any tool given `set: "selection"`/`"view"` | `get_photos` | – | `view` = photos of the active folders/collections; filter bar not applied |
| `find_photos_in_catalog` | `find_photos` | – | `catalog:findPhotos`; capture dates filtered in Lua |
| `get_photo_details` (part) | `photo_info` | – | batched metadata reads |
| `view_photo` | `render_preview` | – | `LrExportSession` JPEG in the temp folder; the backend reads and deletes it |
| `show_in_lightroom` | `show_photos` | own collection only | allowed when read-only; "LrGeniusAI Agent" set |
| `update_photos` | `update_photos` | yes | one undo step; keywords by "Parent > Child" |
| `manage_collection` | `collection` | yes | adding creates the collection (inside `collection_set` when given); removing without `collection_set` searches the whole tree and refuses when several collections share the name |
| `list_collections` | `list_collections` | – | |
| `get_develop_settings` | `get_develop_settings` | – | |
| `apply_develop_settings` | `apply_develop_settings` | yes | snapshot per photo; chunks of 50 photos (one write gate each) |
| `analyze_photos` | `analyze_photos` | – (backend only) | embeddings, faces, species, cull; no AI metadata (see below); progress per indexed photo via the pipeline's `onPhotoAnalyzed` |
| `run_plugin_tests` | `run_tests` | only with catalog writes | `PluginTests.run` |
| `get_logs` (plugin) | `read_log` | – | tail of `LrGeniusAI.log` |
| `eval_lua` | `eval_lua` | developer | needs `.lrdevplugin` + the developer switch; `print` and `_PLUGIN` arrive as chunk arguments, since Lightroom's Lua has no `print`, `setfenv` or `getfenv` |
| `reload_plugin_modules` | `reload_modules` | developer | merges new module tables into the old ones. Reloading `AgentBridge` changes nothing for the running loop until the bridge restarts |

Photos travel as backend photo IDs. `AgentCommands.resolvePhotos` maps them
to catalog photos through a per-session cache. Forward lookups
(photo → ID) fill it as a side effect; a miss rebuilds it from the whole
catalog at most every 30 s.

**Why `analyze_photos` writes no AI metadata.** That path in
`TaskAnalyzeAndIndex.lua` carries review dialogs, a photo-context dialog and
several save passes, all tied to the dialog's settings. Running it headless
means extracting that orchestration from the task first. Until then, the agent
can write metadata itself (`view_photo`, then `update_photos`).

---

## Adding a tool

1. Add the Lua handler to `AgentCommands.lua` (`handlers.<action>`), and list it
   in `CHANGES` if it changes photos (or `DEVELOPER`). Raise errors written for
   the user. Return `result, warnings`.
2. Add the tool in the right `tools_*.rs`. Validate arguments before calling
   `self.lightroom(...)`. Use `CallOptions::write` for changes and pass the
   request context so progress reaches the client. Keep the doc comment
   specific — it is what the model reads.
3. Add a test in `crates/lrg-mcp/tests/mcp_roundtrip.rs` (simulated plugin), a
   busted spec for pure Lua helpers, and an in-Lightroom test in
   `PluginTests.lua`.
4. Update the contract table above, [Help: AI Agents](Help-AI-Agents), and
   the tool tables of the full-test runbook
   (`.claude/skills/lrgenius-mcp-full-test/SKILL.md`).

---

## Testing

| Layer | Command | Needs |
|---|---|---|
| Bridge queue, sessions, doorbell, routes | `cargo test -p lrg-api agent` | nothing |
| MCP server end to end with a simulated plugin | `cargo test -p lrg-mcp` | nothing |
| Lua helpers (search descriptors, keyword paths, JSON safety, dispatch) | `busted` | nothing |
| The plugin inside Lightroom | `run_plugin_tests`, or `Help → Developer: Run Automated Tests...` | Lightroom |
| Everything through MCP against the real Lightroom | `cargo test -p lrg-mcp --test live_lightroom -- --ignored --test-threads=1` | Lightroom, bridge running |

The live suite connects to `LRG_LIVE_MCP_URL` (default
`http://127.0.0.1:19819/mcp`) with the agent token from `LRGENIUS_MCP_TOKEN`
(Plug-in Manager → *Copy token*). It runs `run_plugin_tests` and fails on any
failed plugin test, so one command checks the plugin end to end.
`LRG_LIVE_WRITES=1` adds the catalog-writing tests. Run them on a test catalog:
they put back what they change, but leave develop history steps, a test keyword
and an empty test collection.

**The full live test.** `.claude/skills/lrgenius-mcp-full-test/SKILL.md` is
the runbook an agent follows to test everything in a real Lightroom. It covers:

- setup;
- the suites above;
- every tool, with what to check;
- the safety switches and failure paths;
- timing;
- clean-up and a report.

Claude Code offers it as the skill `/lrgenius-mcp-full-test`, and `AGENTS.md`
points other agents to it. Its first rule applies to every live test: no
testing in a developer's production catalog without their approval. That
includes read-only tests, and an approved test stays within the approved
scope.

The runbook's client is `scripts/mcp_call.py`, a standard-library script that
calls one tool per run (`--list` lists them). It streams progress to stderr,
saves images, and exits with 0 on success, 1 when the tool reports an error,
and 2 when the call fails.

**Testing a dev build.** The plugin starts the *installed* backend unless
something already answers on 19819. Stop the installed one, start yours
(`cargo run -p lrg-server -- --db-path <catalog dir>/lrgenius.db`), then
load the plugin from `plugin/LrGeniusAI.lrdevplugin` in the Plug-in Manager.

**Agent-driven development loop.** With developer commands on, an agent such
as Claude Code (set up with the Plug-in Manager's *Claude Code* button) can:

1. edit a plugin Lua file,
2. call `reload_plugin_modules`,
3. call `run_plugin_tests`,
4. read failures and `get_logs`, probe with `eval_lua`,
5. and repeat, without anyone clicking in Lightroom.

`Task*.lua` menu files need no reload. Lightroom reads them each time the menu
item runs.

---

## What the first live run in Lightroom found

The headless tests could not catch these. Running the live suite and the
tools against a real Lightroom (LrC 15.6, macOS) found them, and all are
fixed:

- `LrApplicationView.getActiveModule` does not exist; the SDK function is
  `getCurrentModuleName`.
- A socket bound during plugin start-up never reported its port.
- "Reload Plug-in" left the old bridge loop running next to the new one.
- `print`, `setfenv` and `getfenv` do not exist in Lightroom's Lua.
- Inside one write gate, a just-created collection set cannot be asked for
  its children.
- The first match of backend IDs to a 14,884-photo catalog took 23 s,
  because every older hash ID checked its file. It now reads the cached IDs
  without touching files (`Util.indexCachedPhotoIds`).
- Photos without a colour label report `"gray"`.

The SDK facts among these are in `.claude/skills/lrc-plugin-dev.md` under
*Common Gotchas*.

## Security model

- `/mcp`: rmcp only answers loopback `Host` values (DNS rebinding) and rejects
  every `Origin` (`enforce_origin_validation` with an empty allow-list). MCP
  clients send no `Origin`.
- `/mcp` also needs the agent token as a bearer token (`require_agent_token` in
  `lrg-mcp/src/lib.rs`, 403 when missing or wrong — a 401 would send clients into
  OAuth discovery). That keeps out other accounts on the same computer, since
  127.0.0.1 is shared, and local programs the user did not give the token to.
  The token is tied to the registered plugin session: whoever registers
  decides the token, but also receives the commands. So a process posing as
  the plugin controls only itself, never the user's Lightroom. With no session
  — Lightroom closed, bridge off or stopped — `/mcp` refuses everything.
- The token is made by the backend (OS random source, 64 hex digits) because
  the SDK has no documented random generator, and kept by the plugin in
  LrPasswords. The Plug-in Manager shows it masked and copies setups to the
  clipboard (`Util.copyToClipboard`, via a temp file and pbcopy/clip) instead
  of displaying them.
- `/v1/agent/*`: same Host/Origin rules in `routes/agent.rs`, plus the session
  token on everything after registration. The status read never shows the
  token.
- The doorbell carries no data and triggers nothing by itself.
- Changes can be turned off (read-only), and developer commands need a
  `.lrdevplugin` install *and* a switch. Ticking that switch asks the user to
  confirm. The first copy of an agent setup (it carries the token) shows the
  agent warning and needs *I understand*; that is stored as
  `prefs.agentDisclaimerAccepted`, compared against
  `Defaults.agentDisclaimerVersion`, so bumping the version shows it again.
  The full text is in [Disclaimer](Disclaimer).
- Every change is one named undo step; develop changes leave snapshots. The
  SDK cannot delete photos.
- Agents read catalog text written by others (captions, keywords, file names).
  Treat it as untrusted. This is why the tools return warnings and results
  rather than acting on their own, and why changes stay undoable.

The rest of this API remains reachable by any page in the user's browser (see
the note in `routes/ui.rs`). The agent routes are the exception because they
can change the catalog.
