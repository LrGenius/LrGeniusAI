# "Ask Your Catalog" — Implementation Plan

## Goal

Ship a conversational chat panel inside Lightroom that lets photographers query and act on their catalog in natural language. The chat is powered by a tool-using LLM running server-side. Tools are thin adapters over the existing services (`search`, `persons`, `db`, `chroma`, `style_engine`). Writes (collections, ratings, edits) are never executed by the LLM directly — the LLM **proposes**, the user **confirms**, and the plugin **applies** via the Lightroom SDK.

> **Status:** Not started. This document is the implementation plan. Read it top-to-bottom; phases are ordered so each step lands a working, testable slice.

## Product Direction

- Photographers should be able to ask questions like *"Pull my 30 strongest portraits of Anna from 2024 and put them in a collection"* and get a concrete proposed action.
- The first release is **read-mostly**: every retrieval tool works; the write tools all go through an explicit confirmation step.
- Tools take and return `photo_id` sets so that face groupings, embeddings, and SQL-style metadata filters can be **composed by the LLM** without us writing join logic.

## Guiding Principles

- The agent loop runs on the server. The plugin is a chat UI + action applier — it does not know about embeddings, faces, or providers.
- Reuse existing services. New code is wrappers, an LLM dispatch loop, and a tiny session store.
- Tool results are **summaries**, not raw data. Photo-id lists longer than ~50 are stashed under an opaque `result_ref` the LLM can pass into the next tool.
- Writes are gated. The LLM emits `propose_action`; only `POST /chat/commit` after explicit user confirmation triggers a write, and the commit endpoint validates that the proposal references photo_ids the agent actually retrieved this turn (prompt-injection guard).
- Provider-agnostic. `LLMProviderBase` gains a `chat_with_tools` method; cloud providers use native function calling, local providers (LM Studio, Ollama) use a JSON-mode shim.

## MVP Scope

The MVP must support:

- A new Lightroom plug-in extra "Catalog Chat" that opens a floating chat dialog.
- Session-persistent transcript stored in SQLite, survives plugin reload.
- Streaming-style updates: every tool call and its summary appear in the transcript as the agent works.
- Tool set: `semantic_search`, `similar_to`, `list_persons`, `photos_of_person`, `metadata_query`, `photo_details`, `index_stats`, `propose_action`. (`group_bursts` and `propose_edit` are stretch — see Phase 6.)
- Action kinds at v1: `create_collection`, `set_rating`, `set_flag`, `set_color_label`, `export_csv`. (`apply_edit_recipe` deferred to Phase 6.)
- Works against Gemini and ChatGPT providers at v1. LM Studio/Ollama via the JSON-mode shim (Phase 5).

Out of scope for v1: voice input, multi-turn memory across sessions, agent-initiated edit-recipe authoring, batch undo.

---

## Architecture Snapshot

```
plugin/LrGeniusAI.lrdevplugin/TaskCatalogChat.lua
        │  HTTP (poll-stream)
        ▼
server/src/routes/chat.py                 ← NEW Blueprint
        │ delegates to
        ▼
server/src/services/agent.py              ← NEW: orchestrator + session store
        │ uses
        ├─ services/agent_tools.py        ← NEW: tool schemas + dispatch table
        │       (adapters over existing services)
        └─ providers/<provider>.chat_with_tools(...)  ← NEW method on base
```

Existing modules touched (no behavior change, only additions):
- `providers/base.py` — abstract `chat_with_tools`.
- `providers/chatgpt.py`, `providers/gemini.py` — implement `chat_with_tools`.
- `providers/lmstudio.py`, `providers/ollama.py` — JSON-mode shim (Phase 5).
- `services/db.py` — add `query_photos(filters)` helper and chat-session tables.
- `geniusai_server.py` — register the new `routes/chat.py` Blueprint.

CLAUDE.md rules to obey throughout:
- Lua: `LrTasks.pcall`, never native `pcall`. All GUI strings via `LOC(...)` with entries added to all three `TranslatedStrings_*.txt` files.
- Lua: errors surface via `ErrorHandler.handleError`.
- Python: use the configured `logger` with `exc_info=True` on exceptions. Endpoints return `{results, error, warning}`. Imports sibling-relative within a subpackage, absolute across subpackages.
- Dependency changes via `uv add`; commit both `pyproject.toml` and `uv.lock`.
- All Python must pass `bash server/scripts/lint_format.sh`.
- Keep `plugin/.../APISearchIndex.lua` in sync with new endpoints.

---

## Phase 0 — Foundations & scaffolding

**Goal:** create empty modules and tables; everything wired but nothing functional.

### 0.1 Create the new files

- `server/src/services/agent.py` — module with a `Session` dataclass, an in-memory `SESSIONS: dict[str, Session]` registry, and stub `start_session()`, `run_turn()`, `commit_proposal()` functions that just raise `NotImplementedError`.
- `server/src/services/agent_tools.py` — empty module with a `TOOLS: dict[str, ToolSpec]` registry (also empty for now) and a `dispatch(name, args, session) -> dict` stub.
- `server/src/routes/chat.py` — Flask Blueprint named `chat_bp`, prefix `/chat`. Routes (all 501 stubs):
  - `POST /chat/session` → `{session_id}`
  - `POST /chat/turn` → `{events: []}` (events listed in Phase 3)
  - `GET /chat/turn/<turn_id>/events?cursor=<n>` → polling stream
  - `POST /chat/commit` → `{ok: true, applied: {...}}`
  - `GET /chat/session/<id>` → transcript replay
- Register the Blueprint in `server/src/geniusai_server.py` next to the other `register_blueprint` calls.

### 0.2 Persistence tables

Add to `services/db.py` (or wherever schema bootstrap lives — grep `CREATE TABLE` to find the right module):

```sql
CREATE TABLE IF NOT EXISTS chat_sessions (
    session_id   TEXT PRIMARY KEY,
    catalog_id   TEXT,
    created_at   INTEGER NOT NULL,
    updated_at   INTEGER NOT NULL,
    provider     TEXT,
    model        TEXT
);

CREATE TABLE IF NOT EXISTS chat_messages (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id   TEXT NOT NULL REFERENCES chat_sessions(session_id),
    turn_id      TEXT NOT NULL,
    role         TEXT NOT NULL,        -- 'user' | 'assistant' | 'tool' | 'proposal'
    content_json TEXT NOT NULL,
    created_at   INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_chat_messages_session ON chat_messages(session_id, id);
```

Also pre-emptively add SQLite indexes the chat will lean on (skip any that already exist):

```sql
CREATE INDEX IF NOT EXISTS idx_photos_capture_date ON photos(capture_date);
CREATE INDEX IF NOT EXISTS idx_photos_rating ON photos(rating);
CREATE INDEX IF NOT EXISTS idx_photos_camera ON photos(camera);
```

### 0.3 Plugin scaffold

- `plugin/LrGeniusAI.lrdevplugin/TaskCatalogChat.lua` — file with a single `LrTasks.startAsyncTask` block that opens a placeholder dialog ("Catalog Chat — coming soon"). Wrap all work in `LrTasks.pcall`.
- Register the task in `Info.lua` (look at how other `Task*` actions are registered) so it appears under *Library → Plug-in Extras*.
- Add stub LOC strings to `TranslatedStrings_en.txt`, `TranslatedStrings_de.txt`, `TranslatedStrings_fr.txt`: `$$$/LrGeniusAI/CatalogChat/MenuItem=Catalog Chat (beta)`, `$$$/LrGeniusAI/CatalogChat/Title=Catalog Chat`. Run `python sync_translations.py` after editing.
- Add the new endpoints to `APISearchIndex.lua`.

**Acceptance:** server boots, plugin loads, menu item opens placeholder dialog, hitting `/chat/session` returns a JSON 501.

---

## Phase 1 — Tool adapters (read-only)

**Goal:** every read tool callable in isolation and unit-tested. No LLM yet.

### 1.1 Tool schema model

In `services/agent_tools.py`:

```python
@dataclass
class ToolSpec:
    name: str
    description: str               # what the LLM sees
    json_schema: dict              # JSONSchema for the arguments
    handler: Callable[[dict, "Session"], dict]
    requires_catalog: bool = True
```

`dispatch(name, args, session)` validates `args` against `json_schema` (use `jsonschema` — already a transitive dep; if not, `uv add jsonschema`), then calls the handler. Wrap every handler in try/except and return `{"error": "..."}` on failure. Log every dispatch with `logger.info(f"tool={name} args={...} elapsed=...")`.

### 1.2 Implement these tools (one per sub-step; each is a wrapper)

For each: write the tool, register it in `TOOLS`, write a pytest in `server/test/test_agent_tools.py`.

| Tool name | Wraps | Notes |
|---|---|---|
| `semantic_search` | `services.search.search_images` | Args: `query: str`, `max_results: int = 30`, `strictness: "low"\|"medium"\|"high" = "medium"`, `sources: list[str] = ["semantic_siglip"]`, `scope_photo_ids: list[str]\|null`, `quality_sort: bool = false`. Inject `catalog_id` from session. Return shape: `{photo_ids:[...], summary:{count,top_scores}, result_ref:"r_..."}` |
| `similar_to` | `services.search.find_similar_images` | Args: `photo_id: str`, `max_results: int = 30`. |
| `group_bursts` | `services.search.group_similar_images` | Args: `photo_ids: list[str]\|result_ref`, `time_window_seconds:int\|null`, `phash_threshold:int\|null`. Return groups with a "best" photo_id per group. |
| `list_persons` | `services.persons.list_persons` | No args. Return list of `{person_id, name, photo_count}`. Truncate to top 50 by photo_count and note truncation in `summary`. |
| `photos_of_person` | `services.persons.get_photo_ids_for_person` + name resolution | Args: `person_id: str \| null`, `name: str \| null` (one must be set), `scope_photo_ids: list\|null`. Name resolution: case-insensitive match against `services.persons._load_person_names`. If multiple matches, return `{error:"ambiguous", candidates:[...]}`. |
| `metadata_query` | new helper `services.db.query_photos(...)` (see 1.3) | Args: `filters: dict`, `max_results: int = 200`. |
| `photo_details` | `services.db` + `services.chroma.get_image` | Args: `photo_ids: list[str]`, hard-cap 25. Returns full per-photo bundle: capture_date, rating, flag, keywords, persons, camera, lens, iso, aperture, shutter, dimensions, description (if indexed). |
| `index_stats` | `services.db.get_database_stats` | No args. |
| `propose_action` | none (echo + validate) | Args: `kind: enum`, `payload: dict`, `dry_run_summary: str`. Validate that any `photo_ids` in payload are present in this session's accumulated retrieval set (the prompt-injection guard). Persist the proposal in `chat_messages` with role `'proposal'` and a freshly generated `proposal_id`. |

### 1.3 New helper: `services.db.query_photos`

Supported filter keys (all optional, combined with AND):
- `photo_ids: list[str]` — pre-scope
- `catalog_id: str` (injected automatically by session)
- `date_range: [iso, iso]` on `capture_date`
- `rating_min: int`, `rating_max: int`
- `flag: "pick"|"reject"|"unflagged"`
- `color_label: str`
- `camera: str` (LIKE), `lens: str` (LIKE)
- `iso_range: [int, int]`, `aperture_range: [float, float]`, `shutter_range: [float, float]`
- `keywords_any: list[str]`, `keywords_all: list[str]`
- `has_keywords: bool` (any vs none)
- `has_indexed_description: bool`

Returns `[{photo_id, capture_date, rating, flag, camera, lens, keywords[], persons[]}]` — keep the column set tight; use `photo_details` for the full bundle.

Add unit tests with a temp SQLite fixture; aim for one test per filter dimension.

### 1.4 Result-ref store

In `Session`, maintain `result_refs: dict[str, list[str]]`. Whenever a tool returns more than 50 photo_ids, mint a `result_ref` (`r_<short-uuid>`), store the full list there, and return only the ref + summary to the LLM. When a downstream tool receives `scope_photo_ids` that is a string starting with `r_`, resolve it via the session.

**Acceptance:** `pytest server/test/test_agent_tools.py` green. Manually `curl`able via a temp debug endpoint `POST /chat/_debug_tool` that takes `{name, args, session_id}` and dispatches one tool. (Remove the debug endpoint before Phase 7.)

---

## Phase 2 — Provider tool-calling

**Goal:** Gemini and ChatGPT can run the loop end-to-end against the tool set.

### 2.1 Extend `LLMProviderBase`

Add new types in `providers/base.py`:

```python
@dataclass
class ChatMessage:
    role: Literal["system", "user", "assistant", "tool"]
    content: str | None = None
    tool_calls: list["ToolCall"] | None = None
    tool_call_id: str | None = None      # when role == "tool"

@dataclass
class ToolCall:
    id: str
    name: str
    arguments: dict

@dataclass
class ChatEvent:
    kind: Literal["assistant_text", "tool_call", "done"]
    payload: dict
```

Abstract method:

```python
def chat_with_tools(
    self,
    messages: list[ChatMessage],
    tools: list[ToolSpec],
    *,
    model: str | None = None,
    temperature: float = 0.2,
) -> Iterator[ChatEvent]: ...
```

Non-streaming for v1 is fine — return one `assistant_text` (possibly with tool_calls) and then `done`.

### 2.2 Implement for ChatGPT and Gemini

- `providers/chatgpt.py`: map `ToolSpec.json_schema` → OpenAI `tools=[{type:"function", function:{...}}]`. Parse `tool_calls` from the response. Re-entry on the next iteration includes prior `tool_calls` and corresponding `role:"tool"` messages.
- `providers/gemini.py`: map to `function_declarations`. Gemini returns parallel `function_call` parts — handle list of tool_calls per assistant turn.

Each provider must accept a model override from the session (so the LLM model is chosen at session start and persisted).

### 2.3 System prompt

Build it in `services/agent.py::_build_system_prompt(session)`. Should include:
- Role: "You are an assistant inside Adobe Lightroom Classic helping the user query and act on their photo catalog."
- Hard rules:
  - "You cannot modify the catalog directly. To make changes, call `propose_action` with a clear `dry_run_summary`. The user will confirm or reject."
  - "Always cite the tool result that produced any photo_id you reference."
  - "Prefer narrow `metadata_query` + `semantic_search` over broad lists. Use `scope_photo_ids` or `result_ref` to intersect."
  - "Never invent photo_ids, person_ids, or keywords. If a lookup returns nothing, say so."
- Brief catalog summary (from `index_stats`): photo count, date range, indexed coverage. Inject at session start so the model knows the rough scale.

### 2.4 Loop driver

`services/agent.py::run_turn(session_id, user_text) -> turn_id`:

1. Append user message.
2. Iteratively call provider with `(history, tools)`.
3. If response has `tool_calls`, dispatch each via `agent_tools.dispatch`, append `role="tool"` messages with `{tool_call_id, name, summary_json}`. Loop.
4. Stop when the model returns plain text (no tool_calls). Cap at `MAX_ITERS = 8`. On cap, append a synthetic system message "tool budget exhausted, finalize your answer" and do one more turn.
5. Append every event to an `events` ring (indexed by turn_id) so `GET /chat/turn/<id>/events?cursor=` can stream them.

**Acceptance:** scripted integration test in `server/test/test_agent_loop.py` (gated behind an env var like `GENIUSAI_RUN_LLM_TESTS=1` so CI doesn't pay for it) that runs three prompts against a real provider and asserts the final transcript references plausible photo_ids from a fixture catalog. Also a mocked test using a fake provider that emits a scripted sequence of tool_calls, asserting dispatch order and event log.

---

## Phase 3 — HTTP surface & event polling

**Goal:** the plugin can drive a full conversation through HTTP.

### 3.1 Implement the routes

- `POST /chat/session` body `{catalog_id, provider, model}` → `{session_id, system_summary}`.
- `POST /chat/turn` body `{session_id, message}` → `{turn_id}` (returns immediately; the turn runs in a background thread).
- `GET /chat/turn/<turn_id>/events?cursor=<int>` → `{events:[{seq, kind, payload}], next_cursor, done: bool}`. Long-poll: block up to ~10s if no new events, then return.
- `POST /chat/commit` body `{session_id, proposal_id}` → `{ok, action:{kind, payload}}`. Returns the (validated) action descriptor to the plugin so the plugin can apply it. Mark the proposal as `applied` in the DB.
- `GET /chat/session/<id>` → full transcript replay.

Event payload shapes:

```jsonc
// kind: "tool_call"
{ "seq": 4, "kind": "tool_call", "payload": {
  "tool": "semantic_search", "args_preview": "query=…",
  "tool_call_id": "tc_…" } }

// kind: "tool_result"
{ "seq": 5, "kind": "tool_result", "payload": {
  "tool_call_id": "tc_…",
  "summary_text": "188 photos, top score 0.42",
  "result_ref": "r_xyz" } }

// kind: "assistant_text"
{ "seq": 6, "kind": "assistant_text", "payload": { "text": "..." } }

// kind: "proposal"
{ "seq": 7, "kind": "proposal", "payload": {
  "proposal_id": "p_…",
  "kind": "create_collection",
  "dry_run_summary": "Create collection 'Anna 2024 portfolio' with 30 photos",
  "photo_ids": [...],
  "extra": { "collection_name": "Anna 2024 portfolio" } } }

// kind: "error" / "done"
{ "seq": 8, "kind": "done", "payload": {} }
```

### 3.2 Background turn execution

Use a `concurrent.futures.ThreadPoolExecutor` keyed per-session (max 1 concurrent turn per session — second turn while one is running returns `409 turn_in_progress`). Plug it into `services/jobs.py` if there's an existing pattern; otherwise wire a tiny new executor in `services/agent.py`.

**Acceptance:** end-to-end curl script: create session → post turn → poll events → see at least one `tool_call`/`tool_result` pair → see `assistant_text` or `proposal` → call `/chat/commit` if applicable.

---

## Phase 4 — Lightroom UI

**Goal:** photographers can have the conversation from inside Lightroom.

### 4.1 `TaskCatalogChat.lua` — main panel

Use `LrFunctionContext.callWithContext` + `LrView.osFactory()` to build a non-modal dialog (`LrDialogs.presentFloatingDialog`). Layout (column):

1. **Transcript** — `f:scrolled_view` containing dynamically-appended `f:static_text` and `f:row` blocks. One row per event. Tool-call rows shown as a small italic "$$$/LrGeniusAI/CatalogChat/ToolCall=^[tool] ^[summary]" pill.
2. **Input** — `f:edit_field` (multiline) + Send button. Submit also bound to Cmd/Ctrl+Enter via `f:edit_field`'s `immediate=true` is *not* enough — add a Send button only at v1; keystroke handling on edit_field is unreliable.
3. **Action preview pane** (only when latest event is a `proposal`) — render `dry_run_summary` + a horizontal `f:row` of up to 8 thumbnails (fetch via the existing thumbnail endpoint, base64 → `LrFileUtils.writeFile` to temp jpg → `f:picture`). Apply / Discard buttons.

Use `LrTasks.startAsyncTask` for all I/O; wrap each block in `LrTasks.pcall`. Errors via `ErrorHandler.handleError`. Static text: no `wrap = true` (per memory `feedback_lr_sdk_wrap.md`) — chunk long strings into `\n`-separated lines manually.

### 4.2 Polling loop

After `POST /chat/turn`, kick off a polling loop in a second `LrTasks.startAsyncTask`:

```lua
local cursor = 0
while not done do
  local resp = http.get(serverUrl .. "/chat/turn/" .. turnId .. "/events?cursor=" .. cursor)
  for _, event in ipairs(resp.events) do
    appendEventToTranscript(event)   -- updates LrView properties; use LrBinding observable
    cursor = event.seq + 1
  end
  done = resp.done
  if not done then LrTasks.sleep(0.25) end
end
```

Use observable property table (`LrBinding.makePropertyTable`) so transcript view updates reactively without re-creating the dialog.

### 4.3 Apply path

On Apply click: `POST /chat/commit` → server returns the action descriptor. Lua dispatch table:

| `kind` | Lua action |
|---|---|
| `create_collection` | `catalog:withWriteAccessDo` → `catalog:createCollection(name, parent, returnExisting=true)` then `collection:addPhotos(photos)`. Resolve `photo_ids` → `LrPhoto` via `Util.getPhotoForGlobalPhotoId` (or whatever the existing helper is — grep). |
| `set_rating` | iterate, `photo:setRawMetadata("rating", value)` inside `withWriteAccessDo`. |
| `set_flag` | `photo:setRawMetadata("pickStatus", 1 \| 0 \| -1)`. |
| `set_color_label` | `photo:setRawMetadata("colorNameForLabel", label)`. |
| `export_csv` | write a CSV with `LrFileUtils`; show success dialog with file path. |

All strings via `LOC(...)`; add entries to all three `TranslatedStrings_*.txt`. Update `Info.lua` to register the menu item. Update `APISearchIndex.lua` with the new `/chat/*` endpoints.

**Acceptance:** in Lightroom, run "Catalog Chat (beta)" → ask "show me 10 photos shot in 2024 with rating >= 4" → see tool calls render → see a textual answer with photo descriptions → ask "make a collection from those" → see proposal card → click Apply → collection appears in Lightroom.

---

## Phase 5 — Local providers (LM Studio / Ollama)

**Goal:** the chat works on local-only setups, even when the model lacks native function-calling.

### 5.1 JSON-mode shim

In `providers/base.py` add a concrete `_chat_with_tools_jsonmode(...)` helper that subclasses can call. It:

1. Serializes tool schemas into the system prompt as a numbered list with JSON examples.
2. Instructs the model to respond with **exactly one** of:
   - `{"tool": "<name>", "arguments": {...}}`
   - `{"final": "<assistant text>"}`
   - `{"proposal": {kind, payload, dry_run_summary}}`
3. Parses the JSON (use `json.loads` with a forgiving regex extractor that strips code fences).
4. Returns the result as a `ChatEvent` so the loop driver sees the same shape as native function-calling.

`providers/lmstudio.py` and `providers/ollama.py` implement `chat_with_tools` by calling `_chat_with_tools_jsonmode`.

### 5.2 Capability flag

`LLMProviderBase` gets a `supports_parallel_tool_calls: bool` class attribute. The loop driver caps `tool_choice` to one-at-a-time when this is false. Local-mode loops will be slower (sequential) — acceptable.

**Acceptance:** same scripted integration test as Phase 2.4 passes against Ollama with a 7B-class model that emits JSON reliably (Qwen2.5 or Llama-3.1-Instruct). Document the model recommendation in `docs/wiki/Help-Choosing-AI-Model.md`.

---

## Phase 6 — Stretch tools & polish

### 6.1 `group_bursts` tool

Already an adapter over `services.search.group_similar_images`; just register it once Phase 4 is stable.

### 6.2 `propose_edit` tool + `apply_edit_recipe` action

- Tool: `propose_edit(photo_ids \| result_ref, instruction)` calls `services/style_engine.py` in dry-run mode and returns a recipe JSON plus a human-readable summary.
- Action: Lua reuses `TaskAiEditPhotos`'s recipe applier (refactor that file's apply path into a callable helper `AiEdit.applyRecipe(photos, recipe)` if not already factored).

### 6.3 Transcript persistence

Verify `chat_messages` rows survive plugin restart and `GET /chat/session/<id>` rebuilds the panel. Add a "Sessions" dropdown to resume past chats.

### 6.4 Error UX

- Tool error → transcript shows an error pill, the LLM gets the error message and can recover (typically by trying a different filter).
- Provider quota/auth errors → surface via `ErrorHandler.handleError` in Lua and offer a "Switch provider" button.

### 6.5 Telemetry (opt-in, respect existing privacy posture)

Count tool calls per turn, average loop iterations, proposal acceptance rate. Local logs only unless the user has opted into telemetry.

---

## Phase 7 — Hardening before release

- Remove the `POST /chat/_debug_tool` endpoint.
- Add rate limit per session (e.g. 30 turns/hour) to bound provider cost.
- Confirm `metadata_query` performance on a 200k-photo fixture (use `EXPLAIN QUERY PLAN`). Add indexes if needed.
- Confirm thread safety: ChromaDB calls under load, SQLite WAL mode for `chat_messages`.
- Update `docs/wiki/Plugin-Guide.md` and `docs/wiki/Server-Guide.md` with a "Catalog Chat" section.
- Add a wiki help page `docs/wiki/Help-Catalog-Chat.md` modeled on `Help-Advanced-Search.md`.
- Final lint/format: `bash server/scripts/lint_format.sh`. Run `uv run pytest test/` from `server/`.
- Smoke test via `TaskAutomatedTests.lua` — add a minimal "chat session round-trip" check.

---

## File-by-file checklist

Server (new):
- [ ] `server/src/routes/chat.py`
- [ ] `server/src/services/agent.py`
- [ ] `server/src/services/agent_tools.py`
- [ ] `server/test/test_agent_tools.py`
- [ ] `server/test/test_agent_loop.py`

Server (edited):
- [ ] `server/src/geniusai_server.py` — register `chat_bp`
- [ ] `server/src/services/db.py` — `query_photos`, chat tables, indexes
- [ ] `server/src/providers/base.py` — `ChatMessage`, `ToolCall`, `chat_with_tools`, JSON-mode shim
- [ ] `server/src/providers/chatgpt.py` — implement `chat_with_tools`
- [ ] `server/src/providers/gemini.py` — implement `chat_with_tools`
- [ ] `server/src/providers/lmstudio.py` — JSON-mode `chat_with_tools`
- [ ] `server/src/providers/ollama.py` — JSON-mode `chat_with_tools`

Plugin (new):
- [ ] `plugin/LrGeniusAI.lrdevplugin/TaskCatalogChat.lua`

Plugin (edited):
- [ ] `plugin/LrGeniusAI.lrdevplugin/Info.lua` — register menu item
- [ ] `plugin/LrGeniusAI.lrdevplugin/APISearchIndex.lua` — new endpoints
- [ ] `plugin/LrGeniusAI.lrdevplugin/TranslatedStrings_en.txt`
- [ ] `plugin/LrGeniusAI.lrdevplugin/TranslatedStrings_de.txt`
- [ ] `plugin/LrGeniusAI.lrdevplugin/TranslatedStrings_fr.txt`

Docs:
- [ ] `docs/wiki/Help-Catalog-Chat.md`
- [ ] `docs/wiki/Plugin-Guide.md` — section
- [ ] `docs/wiki/Server-Guide.md` — section

---

## Risks / known unknowns

1. **Provider tool-calling reliability on local models.** Mitigation: ship cloud-only at v1, gate local mode behind a "experimental" flag in Phase 5.
2. **Long-poll behind `LrHttp`** — verify whether `LrHttp.get` honors a long timeout cleanly; if not, fall back to 0.5s tight polling.
3. **Prompt injection via photo metadata** — a malicious filename/keyword could try to steer the agent. The proposal validator (Phase 1.2 `propose_action`) is the backstop: any action must reference photo_ids retrieved this turn. Worth one explicit red-team test in Phase 7.
4. **`metadata_query` schema drift** — `services/db.py`'s photo table columns might not match the filter dimensions one-to-one. Verify column names before writing the filter translator; ask the user if anything is missing rather than guessing.
5. **Thumbnail bandwidth** — proposal previews fetch up to 8 thumbnails per turn. If existing thumbnail endpoint is slow, add a batch endpoint `POST /thumbs` returning a JSON map.
