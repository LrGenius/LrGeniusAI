---
name: new-endpoint
description: Adds a backend HTTP route to LrGeniusAI end to end — the axum handler and router, the access-token decision, the heading in Dev-Backend-API.md, the ENDPOINTS entry and wrapper in APISearchIndex.lua, warnings that reach a Lightroom dialog, a contract test and a live check with curl. Use whenever a new /v1 route (or a changed method or path) is being added to server-rs, or the plugin needs to call something the backend does not serve yet.
---

# Add a backend route

A route is not done when the handler compiles. It is done when the plugin can
call it, the API page documents it, a failure reaches a dialog in Lightroom and
`scripts/check-docs.py` passes. These steps get there in order. The rules
behind them are *Key Rules* in `AGENTS.md`; read *Errors must surface* before
writing the handler.

## 1. Place the route

- Routes live in `server-rs/crates/lrg-api/src/routes/`, one module per API
  domain. Add to the domain's module if it has one; create a module only for a
  new domain (and add `pub mod` in `routes/mod.rs`).
- A small module to copy from: `routes/species.rs` — a `GET` and a `POST`,
  `pub fn router() -> Router<Arc<AppState>>` with `.route("/species/…", get(h))`.
- Write the **domain path only** (`/faces/detect`), without `/v1`:
  `lrg_api::build_router` (`lib.rs`) nests every module under `API_PREFIX`.
  The exceptions are `bootstrap.rs` and `update.rs`, merged at the root because
  an old backend must answer them; a new route almost never belongs there.
- A new module is merged with one `.merge(routes::<module>::router())` line in
  `build_router`.

## 2. Write the handler

- Extractors: `State(state): State<Arc<AppState>>` plus `Query`, `Json` or
  `Multipart`. Keep the handler thin: logic goes into `lrg-analysis`,
  `lrg-imaging` or `lrg-ml`, LLM calls into `lrg-providers`.
- Success: `Json(json!({ ... }))`. Failure:
  `(StatusCode::…, Json(json!({"error": msg}))).into_response()` — there is no
  shared error type; see `routes/cull_taste.rs` (a `bad` closure for 400s).
- **A degraded success is not a success.** If the call worked but lost a
  signal, return it in `"warnings": [...]` next to the result —
  `routes/faces.rs` (`POST /faces/search`) is the example; batch routes return
  `error_messages` and `warnings` together (`routes/index_upload.rs`).
  Collect warnings in a list, never in one slot, and word each one as what
  the user should do.
- Log through the `log` facade (`log::warn!` …) — and a log line is not a
  report: whatever the user should know goes into the response.

## 3. Access token

A route in `lrg-api` is behind the access token
(`lrg_api::api_auth::require_api_token`) without you doing anything — unless
its path falls under an existing exemption in `exempt` (`api_auth.rs`):
`/ping`, `/version`, `/version/check`, `/mcp` and `/mcp/*`, all of
`/v1/chat/*`, and the `GET` pages `/v1/ui/people`, `/v1/ui/search`,
`/v1/ui/chat`. Read that list before picking a path. Routes of the agent chat
live in `lrg-chat/src/routes.rs` and the MCP endpoint in `lrg-mcp`; both are
merged by `lrg-server`, not by `build_router`, and check their own token (a
chat session's bearer token in `authorized()`, the agent token for `/mcp`) —
the documentation and contract steps below apply to them all the same. Do
not exempt a new route unless it truly must answer without the token (a page the
browser opens, the bootstrap handshake). Exempting one is a security decision:
add it to `exempt` in `api_auth.rs`, give the reason in that module's docs, and
under *Access* in `docs/wiki/Dev-Backend-API.md`. If you are unsure, ask the
developer instead of exempting it.

## 4. Document it

Add a heading to `docs/wiki/Dev-Backend-API.md` in the domain's section, in
exactly this form — `check-docs.py` matches method **and** path:

```markdown
### `POST /v1/faces/detect`
```

Under it: what the route does, the request and response fields (including
`warnings` and when they appear), and the errors. Check
`python3 scripts/check-docs.py --for <route file>` for other pages that
describe the domain, and update them in the same change.

## 5. Call it from the plugin (if the plugin needs it)

- Add the path to `ENDPOINTS` in
  `plugin/LrGeniusAI.lrdevplugin/APISearchIndex.lua` (`NAME = "/v1/…",`).
  `check-docs.py` fails when an entry there has no backend route.
- Write a wrapper next to the domain's others: build the URL with
  `SearchIndexAPI.url("NAME")` and call `_request(method, url, body, timeout)`
  (or `_requestMultipart`). Never `LrHttp` directly — the wrappers add the
  access token; a call that has to bypass them passes `withAuthHeader(...)`,
  and nothing logs a URL or header that carries the token.
- Return `(ok, response)` so the caller sees `response.warnings` even when
  `ok` is true. Then follow every caller in the `Task*.lua` files: each one
  reads `response.warnings` and shows them with `ErrorHandler.handleError` or
  `LrDialogs.message(..., "warning")`. A wrapper whose warnings no caller reads
  breaks the chain as surely as a backend that never sends them.
- `LrTasks.pcall`, never native `pcall`; user-facing strings in plain English,
  no new `LOC(...)`.

## 6. Test

- A contract test in `server-rs/crates/lrg-api/tests/contract.rs`: `fresh_app()`
  gives `(Router, Arc<AppState>)`, `app.oneshot(Request::…)` sends the request,
  `body_json` reads the answer. Cover the success, a bad request, and the
  warnings path if the route has one.
- Pure Lua helpers you added get a busted spec in `plugin/spec/`.

## 7. Check it live

Unit tests alone have repeatedly missed real bugs here. Start the binary and
call the route:

```bash
cd server-rs
cargo run -p lrg-server -- --db-path /tmp/lrg-dev.db --debug
# second shell:
curl -s -H "Authorization: Bearer $(cargo run -q -p lrg-server -- token 2>/dev/null)" \
  -X POST http://127.0.0.1:19819/v1/faces/detect -d '{...}' | jq .
```

Also call it without the header and confirm the 403 (unless you exempted it on
purpose). Use a throwaway `--db-path`, never the developer's index.

## 8. Finish

```bash
cd server-rs && cargo fmt && cargo clippy --workspace --all-targets   # zero warnings
python3 scripts/check-docs.py                                          # must pass
```

Then hand the change to the `lrg-rules-reviewer` agent, and report the curl
call you made and what it answered.
