# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository. The project's rules live in `AGENTS.md`, shared with every other coding agent and imported here; what follows the import is only what Claude Code alone has. A rule that applies to any agent goes into `AGENTS.md`, never here — `scripts/check-docs.py` fails when this file repeats it.

@AGENTS.md

---

## Claude Code

### Hooks

Wired in `.claude/settings.json`, scripts in `.claude/hooks/`. Fix what a hook reports before moving on — don't disable it to get past a warning.

- **After every edit** (`PostToolUse`, `lint-edited-file.py`) the edited file is linted the way CI lints it, on that file alone: `luacheck` + `stylua` for plugin Lua; `cargo fmt` and `cargo clippy` on the touched crate for `server-rs/` Rust (pre-commit and CI still run clippy on the whole workspace, so a break in a crate that depends on it shows up there); `ruff` for syntax errors and pyflakes findings in Python. An edit to a file behind a hard failure of `scripts/check-docs.py` — a route, `Dev-Backend-API.md`, `APISearchIndex.lua`, `CLAUDE.md`/`AGENTS.md`, a README a wiki page is built from — runs `check-docs.py` at once.
- The same hook names the wiki pages documenting the edited file (once per page per session). Treat that as part of the task: update the page now, or state that it is still correct.
- **Before a live Lightroom test** (`PreToolUse`, `live-test-guard.py`) — running `scripts/mcp_call.py` with any tool but `get_status`, `cargo test` runs that reach the `live_lightroom` tests, requests to `:19819/mcp`, the stdio proxy, the tools of the `lrgeniusai` MCP server — the hook asks the backend which catalog is open. A catalog in `LRG_TEST_CATALOGS` passes; any other, or one it cannot identify, turns the call into a permission prompt that names the catalog, so the developer approves it. The hook reads `LRG_TEST_CATALOGS` from the environment only, where the developer puts it (their shell, or the `env` block of `.claude/settings.local.json`). It backs up *Live tests in Lightroom* above and catches mistakes; it does not replace asking first, and it does not see everything (an endpoint behind `$LRG_MCP_URL`, other MCP clients).

### Language servers

**Ask the language servers before reading files.** `.claude/settings.json` enables rust-analyzer for `server-rs/` and lua-language-server for the plugin; `.luarc.json` sets the latter up for Lightroom's Lua 5.1 and its globals, and leaves out `plugin/spec/`, whose mocks would shadow the real definitions. For "where is X defined, who calls Y, what type is Z", use `workspaceSymbol`, `goToDefinition`, `findReferences`, `incomingCalls` and `hover`, then read only the lines they point to. Grep stays the tool for text (messages, pref keys, JSON fields, route strings, the links between an endpoint in `APISearchIndex.lua` and its axum route) and for code behind `#[cfg(feature = "llamacpp")]`, which rust-analyzer does not compile. rust-analyzer needs about a minute to index a new session, so an empty answer then means "not ready", not "not there". On a large Lua file `documentSymbol` lists every local and every block (Util.lua: about 75 KB); look the name up with `workspaceSymbol` instead. Questions about how parts connect go to graphify when its graph exists (see *graphify* above).

### Subagents

The agents in `.claude/agents/` each run on the model their work needs: `lrg-explore` (Haiku) finds code, `lrg-checks` (Haiku) runs the CI checks and reports only failures, `lrg-docs-sync` (Sonnet) brings the wiki in step with a change, `lrg-rules-reviewer` (Sonnet) checks a change against *Key Rules*, and `lrg-live-tester` (Sonnet) runs the mechanical phases of the live-test runbook. Hand them build output and wide searches rather than reading those in the main conversation; design and implementation stay on the session's model.

### Skills

The runbooks in `.claude/skills/`, each a slash command:

- `/lrgenius-mcp-full-test` — the full live test inside Lightroom, ending in a report (see *Live tests in Lightroom* above).
- `/new-endpoint` — a backend route end to end: router, access token, API doc, plugin wrapper, tests, live check.
- `/new-agent-command` — an agent-bridge command together with the MCP tool that sends it, and the contract table.
- `/release` — cutting a release: pre-flight, tag, CI, the draft release.

`.claude/rules/lrc-plugin-dev.md`, the Lightroom Classic SDK reference, is path-scoped to `plugin/**/*.lua` and loads by itself.
