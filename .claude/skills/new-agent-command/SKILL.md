---
name: new-agent-command
description: Adds a Lightroom command for AI agents to LrGeniusAI end to end — the handler in AgentCommands.lua (and CHANGES if it writes photos), the MCP tool in lrg-mcp that sends it with the right read-only/destructive annotations, the simulated-plugin and listing tests, and the row in the contract table of Dev-Agent-Bridge.md. Use whenever an MCP tool needs Lightroom to do something it cannot do yet, or an agent-bridge command or its MCP tool changes.
---

# Add an agent-bridge command

An MCP tool that works on Lightroom is two halves joined by an action name: a
Rust tool in `lrg-mcp` sends `{action, args}` through the agent bridge, and a
Lua handler in `AgentCommands.lua` does the work inside Lightroom. The two
halves and the contract table in `docs/wiki/Dev-Agent-Bridge.md` change
together. Nothing checks that mechanically — `check-docs.py` only warns that
the page may be stale — so this runbook is the check.

Read *Command contract* and *Adding a tool* in `docs/wiki/Dev-Agent-Bridge.md`
first.

## 1. Decide what kind of command it is

- **Reads only** (status, details, previews): no gate.
- **Changes photos** (metadata, keywords, collections, develop settings): it
  goes into `CHANGES`, so the user's read-only switch in the Plug-in Manager
  refuses it. When in doubt, it changes photos.
- **Developer-only** (runs arbitrary code, reloads modules): it goes into
  `DEVELOPER`, refused unless *Allow developer commands* is on. Its MCP tool
  belongs in `tools_dev.rs`.

## 2. The Lua handler

In `plugin/LrGeniusAI.lrdevplugin/AgentCommands.lua`:

- Add `function handlers.<action>(args, env)` next to related handlers.
  Models: `handlers.photo_info` (read-only, resolves photos) and
  `handlers.update_photos` (changes photos: filters locked photos with
  `editable(photos)`, collects `warnings`, writes inside
  `catalog:withWriteAccessDo` named from `UNDO_NAMES`, so it is one Undo
  step).
- Resolve photo IDs with `AgentCommands.resolvePhotos(env, args.photo_ids)`,
  which returns `(photos, missing)`. Report `missing` back as a warning; never
  drop it.
- Fail with `error("…")` in words the agent can act on. `AgentCommands.run`
  catches it with `LrTasks.pcall`, strips the `file:line:` prefix and returns
  `{ok = false, error, warnings}`; a success returns `{ok = true, result,
  warnings}`. Never use native `pcall`.
- Add `<action> = true` to `CHANGES` (photo-changing) or `DEVELOPER`
  (developer-only). A new kind of write gets its Undo-step name in
  `UNDO_NAMES` (short keys such as `update`, `develop`; the value is what the
  user sees under *Edit → Undo*).
- Pure helpers get specs in `plugin/spec/agent_commands_spec.lua`. A check that
  needs the real catalog goes into `PluginTests.lua`.

## 3. The MCP tool

In `server-rs/crates/lrg-mcp/src/tools_lightroom.rs` (or `tools_dev.rs`):

- An args struct deriving the schema, every field with a doc comment — the
  comment *is* the description the agent reads. Photos come in through
  `#[serde(flatten)] photos: Photos` and `self.resolve_photos(&args.photos)`.
- A `#[tool(name = "…", annotations(title = "…", read_only_hint = …,
  destructive_hint = …, idempotent_hint = …))]` method. **The annotations are
  the permission model:** `lrg-chat`'s `ToolClass::of` (`permissions.rs`)
  makes `read_only_hint = true` a *Read*, `destructive_hint = false` a
  *Change*, and everything else — missing annotations included — a
  *Destructive* tool the user must approve. Set them deliberately.
- Call the plugin with `self.lightroom("<action>", payload,
  CallOptions::read(…)` or `CallOptions::write(…), Some(&context))`, cap the
  photo count (`MAX_PHOTOS_PER_CALL`), and answer with
  `json_result(with_warnings(reply.result, &reply.warnings))`, errors through
  `bridge_error(e)`. `update_photos` in the same file shows the whole shape.
- The tool's doc comment says what it does, what it changes, and what the
  agent should do with the result. Agents choose tools by that text.

## 4. Tests

- `server-rs/crates/lrg-mcp/tests/mcp_roundtrip.rs`: add the tool to the list
  in `every_tool_and_prompt_is_listed_and_described`, and an arm for the action
  to `simulate_plugin`. If it changes photos, check that it is refused when
  changes are off, like the existing read-only refusal test.
- Optionally an `#[ignore]` case in `live_lightroom.rs` — run only within the
  live-test rules (`/lrgenius-mcp-full-test`), never in a catalog the developer
  has not approved.

## 5. The contract table

Add a row to *Command contract* in `docs/wiki/Dev-Agent-Bridge.md`:

```markdown
| `mcp_tool_name` | `bridge_action` | yes / no / own collection only | one Undo step; … |
```

Update the page's other sections the command touches (the file list, *Adding a
tool*), and `docs/wiki/Help-AI-Agents.md` if users see the tool. Run
`python3 scripts/check-docs.py --for <each changed file>` for any other page.

## 6. Finish

```bash
cd server-rs && cargo fmt && cargo clippy --workspace --all-targets && cargo test -p lrg-mcp -p lrg-chat
cd .. && busted && python3 scripts/check-docs.py
```

Then hand the change to the `lrg-rules-reviewer` agent. To try the command in
Lightroom, the developer reloads the plugin (or an agent with developer
commands calls `reload_plugin_modules`) and you follow
`/lrgenius-mcp-full-test`, Rule 1 first.
