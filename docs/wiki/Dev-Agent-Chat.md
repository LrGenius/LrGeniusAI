# Dev: Agent Chat

The in-app chat: an AI model — on this computer (llama.cpp, MLX), on an AI
server (LM Studio, Ollama, the user's own) or in the cloud (OpenAI, Anthropic,
Gemini) — works the user's catalog through the same tools the [MCP endpoint](Dev-Agent-Bridge) offers external
agents, and the user talks to it — and approves what it may change — in a page
the backend serves. Route reference: [Backend API](Dev-Backend-API#agent-chat).

> **Status:** working end to end — the backend, the page and the menu item
> *Chat with Your Catalog...* — with local models (tested with Gemma 4 on
> MLX and SmolVLM on llama.cpp) and with cloud models and AI servers (tested
> against fakes of each API; not yet against the real services). Cloud
> answers are not streamed yet.

---

## The pieces

```
browser page ──fetch + event stream, chat token──► lrg-chat routes (/v1/chat/*)
                                                     │
                                       ChatSession: the loop
                     ┌───────────────────────────────┴───────────────────────┐
     ChatModel (lrg-providers::chat)                      in-memory MCP client (lrg-chat::mcp)
     ├ LocalChatModel ─► LocalEngine::chat                └► LrGeniusMcp — the same tools as /mcp
     │    ├ lrg-llama (worker thread, llguidance)              ├ index tools ─► REST routes in-process
     │    └ lrg-mlx ─► lrgenius-mlx sidecar (XGrammar)         └ Lightroom tools ─► agent bridge ─► plugin
     └ cloud_chat: OpenAiChat · AnthropicChat · GeminiChat ─► the provider's API
```

| Where | What |
|---|---|
| `server-rs/crates/lrg-providers/src/local.rs` | `LocalEngine::chat`: one assistant turn for a whole conversation, text streamed as it is decoded |
| `…/lrg-llama/src/worker.rs` | llama.cpp: the `Chat` job (sequence 0, chunked prefill, UTF-8-safe deltas) |
| `…/lrg-mlx/src/engine.rs`, `native/mlx-sidecar/` | MLX: the sidecar's `chat` op and its `delta` lines |
| `…/lrg-providers/src/chat.rs` | `ChatModel` and the model-neutral conversation types |
| `…/lrg-providers/src/local_chat.rs` | `LocalChatModel`: tool calling by constrained decoding |
| `…/lrg-providers/src/cloud_chat.rs` | Cloud models and AI servers, through each API's own tool calling |
| `server-rs/crates/lrg-chat/src/session.rs` | One conversation and its loop |
| `…/lrg-chat/src/mcp.rs` | The chat's in-process MCP connection |
| `…/lrg-chat/src/permissions.rs` | What runs without asking; `chat_permissions.json` |
| `…/lrg-chat/src/events.rs` | What the page is told, numbered for reconnects |
| `…/lrg-chat/src/routes.rs` | The HTTP routes and their two tokens |
| `…/lrg-chat/src/prompt.rs` | What the model is told about its job |
| `…/lrg-chat/src/ui/chat.html` | The page (`/v1/ui/chat`), one file with no external resources |
| `plugin/…/TaskChat.lua` | The menu item: picks the model, opens a chat, opens the page |

The chat is an MCP *client* of the tools rather than a caller of their code,
so a chat call takes exactly the path an external agent's takes: the same
result sets, warnings and undo steps, and the same refusals from the plugin's
read-only switch and its "stop agent control" button.

---

## Tool calling without a tool-call format

A local model's native tool-call format lives in its Jinja chat template, and
llama.cpp's C API cannot render Jinja — it applies only built-in templates that
know system, user and assistant. So the chat does not use the native format.
Both engines can constrain decoding to a JSON Schema, and every answer is
constrained to one of

```json
{"analysis": "...", "call": {"search_photos": {"query": "dog on a beach"}}}
{"analysis": "...", "reply": "Here are your photos."}
```

with one schema branch per offered tool. A small model can then neither invent
a tool nor send arguments its schema rejects. Tool results go back as a user
turn (`Result of search_photos: …`), and the model's own earlier turns are
rendered back as the JSON it produced, so it keeps copying the format.

**The key names are chosen for their sort order.** Both grammar engines emit
properties in the order the schema lists them, and `serde_json` lists them
alphabetically unless its `preserve_order` feature is on — which only
llguidance turns on, so only in builds with `llamacpp`. The macOS release is
MLX-only and sorts. `analysis` < `call` < `reply` keeps the reasoning first in
both, and because the tool's *name* is the key under `call`, the tool is chosen
before its arguments are written. Renaming a key means checking that order.

**Tool schemas are cut down** to what both grammar engines take (`grammar_schema`):
`$ref`s inlined (each tool brings its own `$defs`, one grammar holds all of
them), `oneOf` read as `anyOf`, objects closed with `additionalProperties:
false`, and `format` (`int64`, `double` from schemars), defaults, descriptions
and numeric bounds dropped. The prompt carries the descriptions; the tool still
validates its arguments.

**A choice of required arguments becomes one schema per choice.** A tool
that works on photos takes `set` *or* `photo_ids`, and needs exactly one. The
MCP schema cannot say so — it would take a `oneOf` at the top of the tool's
schema, which Anthropic's and OpenAI's APIs refuse — so `lrg-chat/src/mcp.rs`
adds `oneOf: [{required: [set]}, {required: [photo_ids]}]` for the chat only,
and `grammar_schema` turns it into two closed objects, each requiring one and
leaving the other out (`split_choice`). Before, Gemma 4 E4B left both out and
the call failed. A cloud model adapter has to leave such a `oneOf` out again.

**Every string is capped** — 1,000 characters for an argument, 600 for
`analysis`, 6,000 for `reply` — and both engines enforce `maxLength` (checked
by the engine smoke tests). Small models fall into repeating a phrase inside a
string until the token budget runs out: Gemma 4 12B once filled a search text
with `rating_max_5_rating_min_1_…` for 1,500 tokens, which then sat in the
history. And the MLX sidecar keeps control tokens out of answers (see its
README): Gemma 4 had written `<|channel>thought{` into its reasoning.

**The prompt** lists each tool with one line per argument and describes a
shared argument only once — `set` and `photo_ids` repeated cost about 600
tokens. With every tool offered it is about 3,000 tokens. Tool results are
re-serialized without indentation, and only the last two photos stay pixels.

---

## Cloud models and AI servers

These call tools the way their APIs do (`cloud_chat.rs`), so none of the
grammar above applies. Three formats cover every provider:

* **OpenAI's Chat Completions** (`OpenAiChat`) — OpenAI, LM Studio, Ollama at
  its `/v1` endpoint, and the user's own server. A `tool` message takes text
  only, so a photo a tool returned follows in a user message. OpenAI itself
  gets `max_completion_tokens` and no temperature (its reasoning models reject
  one); servers get `max_tokens` and the chat's temperature.
* **Anthropic's Messages** (`AnthropicChat`) — the system prompt and the
  conversation up to its last block are marked for prompt caching, so a turn
  pays full price only for what is new.
* **Gemini's `generateContent`** (`GeminiChat`) — tools as
  `parametersJsonSchema`, and every `functionResponse` carries the id and name
  of the call it answers, which Gemini 3 needs to pair them.

**A turn goes back as its API wrote it.** Claude's thinking blocks and
Gemini's thought signatures must return unchanged with the tool results, and
the neutral parts cannot carry them. So a turn keeps its API's own form
(`ChatPart::Native`), the same adapter sends that back, and every other model
ignores it.

**Schemas** go out with references inlined and without the top-level
`set`-or-`photo_ids` choice, which Anthropic's and OpenAI's APIs refuse there
(`portable_tool_schema`).

**Budget.** A cloud model gets 64,000 tokens of context in the chat's
accounting — far below its window — and only the newest four photos stay
attached: every turn sends the whole conversation again and is billed for it.
Answers get 8,192 tokens on top for thinking (2,048 on a server, whose window
may be small).

**Keys** come from the plugin with the request that opens the chat, as for
indexing, and stay in that chat's memory. When the conversation leaves this
computer, the session says to whom (`sends_to`), the page says so above the
conversation, and the plugin asks once per provider before the first such
chat.

---

## The loop

Per user message, at most 12 model turns (`MAX_STEPS`):

1. Resolve the model **per turn** (`ModelSource`), through the same
   `engine_for_request` an indexing request uses. A session never holds an
   engine: the slots unload idle models and swap them, and a held reference
   would keep the old one alive — for llama.cpp it makes the swap fail.
2. Fit the history into the window (below), offer every tool the permissions
   do not block, and stream the turn to the page.
3. No tool call: done. Otherwise, for each call: announce it, refuse a repeat
   of an identical call made for this message (a loop seen with Gemma 4 12B),
   check the permissions — waiting for the page if they say ask — run it, and
   record the result. **Every call gets a result**, even one that never ran,
   because a history with an unanswered call is one no model API accepts.

**Stop** cancels the model's turn (llama.cpp stops at the next token; the MLX
sidecar finishes its answer and the next request skips its lines), denies
calls waiting for approval, and starts nothing new. A tool already running in
Lightroom is not interrupted; its result is recorded. What the model had
streamed of the cut-off turn is dropped, on the page too, and the history gets
a note that the user stopped there (`STOPPED`): without it the unanswered
request was merged with the next message, and Gemma 4 E4B answered both.

**Context.** The history is estimated at 3 characters per token and 600 per
photo; the system prompt and tools cost what the first turn's prompt tokens
reveal (4,000 assumed before that). When it does not fit, the oldest tool
results give up their text and photos first — the model can run a tool again
— and the last exchange is never touched. llama.cpp reports its window
(`n_ctx` divided by Photos in parallel, since a conversation runs in one
sequence); MLX does not, and 32,768 is assumed.

**The cache carries over between turns.** Every turn sends the whole
conversation, but the engine keeps the KV cache the last turn left and
prefills only what follows the part both share — the tool list and the history
are most of every prompt.

* llama.cpp: the conversation stays in sequence 0, and the worker remembers
  its tokens and image chunks (`Held`); the next turn drops what differs from
  the end of the sequence and evaluates the rest. A new photo after the shared
  part means evaluating all of it (the bindings evaluate image chunks only for
  a whole prompt). Indexing clears it and pins its own prefix again.
* MLX: see the sidecar README. The sidecar keeps the cache, and a copy from
  right after the prompt for sliding-window layers that cannot be trimmed back;
  it also keeps the compiled grammar and rolls it back after each turn.

Measured with Gemma 4 E4B on MLX and the full tool list (~4,000 tokens): a
message that takes two model turns went from ~8–10 s to ~2.5 s, the first
token of a turn coming after 70–210 ms instead of 3.7 s. Only the first turn
of a chat still prefills everything and compiles the grammar (~6 s plus
loading the model).

---

## Permissions

The user decides how much to approve by hand. All of it is enforced in the
loop; the page only answers.

* **Kind of tool**, from its MCP annotations: *read* (`readOnlyHint`),
  *change* (not read-only, `destructiveHint: false`: collections, showing
  results, indexing) and *destructive* (photo metadata, ratings, develop
  settings — and any tool without annotations, MCP's default). Developer tools
  (`tools_dev.rs`, `LrGeniusMcp::developer_tool_names`) are never offered.
* **Mode:** `ask_all`, `auto_read` (default), `auto_safe` (read and change run,
  destructive asks), `auto_all`.
* **Rule per tool:** `allow`, `ask`, or `block` — a blocked tool is not
  offered to the model at all.
* **Bulk limit** (default 200, 0 = off): a destructive call aimed at more
  photos asks even when allowed. The count comes from `photo_ids`, a result set,
  or — for `selection`/`view` — a `get_photos` round-trip to the plugin.
* **Answers to a question:** once, for this chat, always (writes the rule), or
  deny with an optional note that is passed to the model.

The plugin's read-only switch stays underneath all of it: the agent bridge
refuses a change before it reaches Lightroom, whatever the chat allowed.
Settings live in `chat_permissions.json` beside the index (like
`person_names.json`); before the plugin has bound an index they are kept in
memory, and the page is told they are not saved.

Probing local models with the real tools showed why the default asks before
any change: Gemma 4 E4B set the *title* to "5 Sterne vergeben" instead of the
rating, and Gemma 4 12B rated the whole selection when asked for the best five.

What to expect from local models (probed 2026-10-22, simulated tool results
and over HTTP): Gemma 4 12B handles one- and two-step tasks — search and show
the results, status questions, a rating change with the right arguments. It is
unreliable beyond that: after a denial with a note ("only the best three") it
restated the request instead of acting on it. Gemma 4 E4B picks wrong
arguments for changes and repeats a denied call unchanged (the repeat check
catches it). SmolVLM 500M is for testing the plumbing only.

---

## Events

`GET /v1/chat/sessions/{id}/events` is a server-sent event stream of JSON
objects with a `type`. Everything that changes what the page shows is
numbered (`seq`) and kept, so a page that reconnects with `?after=<last seq>`
rebuilds the conversation exactly. The model's streamed pieces (`delta`) are
the exception: live only, unnumbered, followed by one numbered
`assistant_turn` — so the log holds one entry per turn, not one per token. A
page that falls more than 1,024 events behind gets an `event: resync` and the
stream ends; it reconnects and catches up from the log.

| `type` | Fields |
|---|---|
| `user_message` | `text` |
| `delta` | `kind` (`reasoning`/`text`), `text` — live only |
| `assistant_turn` | `reasoning`, `text` |
| `tool_call` | `call_id`, `tool`, `title`, `class`, `arguments`, `photos` |
| `approval_needed` | `call_id`, `reason` (`{kind: mode}`, `{kind: rule}`, `{kind: bulk, photos, limit}`) |
| `tool_running` | `call_id` |
| `tool_done` | `call_id`, `ok`, `text`, `set_id`, `count`, `warnings`, `images` |
| `tool_skipped` | `call_id`, `reason` (`not allowed`, `stopped`, `repeated call`, `turned off`) |
| `warning`, `error` | `message` |
| `stopped` | — (the user stopped the chat; streamed text of an unfinished turn is dropped) |
| `busy` | `busy` |
| `usage` | `prompt_tokens`, `context_window` |

---

## The page and the menu item

*Library → Plug-in Extras → Chat with Your Catalog...* (`TaskChat.lua`)
picks the model — the one chosen for Analyze & Index when it is local,
otherwise the first installed one, MLX first on a Mac — opens a chat with the
agent token and the same engine settings indexing uses (so the backend does
not reload the model with other ones), and opens the page in the browser.
Nothing stays open in Lightroom: the model works through the agent bridge,
which runs whenever Lightroom does. Without a local model or with AI agents
turned off, the task says what to set up instead.

The page (`ui/chat.html`) is one file: no external resources, which its
`Content-Security-Policy` enforces. It reads the event stream with `fetch`
(EventSource cannot send the token), reconnects with `?after=` and backs off,
and rebuilds everything from the numbered events, so a reload shows the same
conversation. It draws:

* the model's turns with their reasoning (*Thinking:*), streamed as decoded;
* a card per tool call — its title, kind, the photos it aims at, its
  arguments in words (`set: selection` reads "the photos selected in
  Lightroom"), its status, warnings, and the tool's answer folded away;
* the approval box on a card that waits: *Allow once*, *Allow in this chat*,
  *Always allow* (not offered for a bulk question: the limit asks even for an
  allowed tool, so "never ask again" would not hold), and *Don't allow* with an
  optional note;
* *What it may do...*: the mode, a rule per tool, the bulk limit, and whether
  the settings are saved;
* how much of the context the last turn used.

---

## Security

Opening a chat takes the **agent token**, which only the plugin and the user's
own AI apps have — the same check as `/mcp`. The session then has its own
**chat token**, which the plugin hands the page in the URL fragment (never sent
to a server, so never in a log). Every page request carries it as `Bearer`; a
request with a foreign `Origin` is refused even with the right token. Wrong
token, foreign origin and ended chat all get the same 403.

This is stricter than `/v1/ui/people` on purpose: that page can do only what
the plugin's own menu items do, and only while its task is open in Lightroom.
The chat can change the catalog whenever Lightroom is open.

---

## Testing

```bash
cd server-rs
cargo test -p lrg-chat                     # permissions, events, the loop with a scripted model, the routes
cargo test -p lrg-providers local_chat     # schema, prompt, parsing, streaming
cargo test -p lrg-providers --test cloud_chat_fake   # each cloud API's wire format, against a fake server
# Engines, against real weights (ignored by default):
LRG_TEST_MODEL_GGUF=… LRG_TEST_MMPROJ_GGUF=… cargo test -p lrg-llama --test engine_smoke -- --ignored
LRG_MLX_SIDECAR=… LRG_TEST_MLX_MODEL_DIR=… cargo test -p lrg-mlx --test sidecar_smoke -- --ignored
```

`tests/chat_loop.rs` drives the real MCP tools with a scripted `ChatModel` and
answers approvals the way the page does; without Lightroom, the Lightroom tools
fail with the bridge's own error, which is exercised too. `tests/routes.rs`
also runs a whole chat on a fake OpenAI-compatible server: its tool call runs
the real `get_status`, and the result goes back as the call's `tool` message.
