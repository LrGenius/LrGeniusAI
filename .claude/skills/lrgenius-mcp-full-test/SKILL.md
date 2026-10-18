---
name: lrgenius-mcp-full-test
description: Full end-to-end test of LrGeniusAI inside a running Lightroom Classic, driven through the backend's MCP endpoint and the agent bridge — automated suites, every MCP tool, the safety switches and the failure paths — ending in a report. Use before merging changes to the agent bridge, the MCP tools, AgentCommands.lua, PluginTests.lua or other plugin code that works on the catalog, and whenever asked to test LrGeniusAI live, in Lightroom, over MCP or end to end. Never tests in a developer's production catalog without their approval.
---

# Full test of LrGeniusAI through MCP

This test drives the real plugin in a running Lightroom Classic through the
backend's `/mcp` endpoint, the same way a user's AI agent does. It reaches
what the headless tests cannot: the Lightroom SDK, the agent bridge and a real
catalog. The architecture and the command contract are described in
`docs/wiki/Dev-Agent-Bridge.md`, and the server lists its tools and their
arguments itself (`python3 scripts/mcp_call.py --list`).

Every Lightroom tool works on whichever catalog is open in Lightroom, which is
why this rule comes first.

## Rule 1: no testing in a production catalog without approval

A developer's production catalogs hold their real photos and work. Never test
in one without the developer's approval. Once they approve, you can test there
too, within the scope they approved.

- **Every catalog counts as production until the developer tells you it is a
  test catalog.** They can say so in this conversation, or in their standing
  instructions: their own `CLAUDE.md`, memory or agent settings. A name or
  path with "test" in it proves nothing, so ask.
- **A test catalog holds copies.** If its photos are the original files of a
  production catalog, treat it as production. With *Automatically write
  changes into XMP* on, metadata written in one catalog ends up in files the
  other one reads.
- **Read-only testing needs approval too.** It sends previews, metadata, file
  paths and the names of recognised people to you, and through you to your
  model provider unless the model runs locally. Meanwhile the plugin keeps
  its usual bookkeeping (the catalog ID and cached photo IDs) in the open
  catalog.
- **Ask before Phase 1.** Name the catalog (`lightroom.catalog.name`,
  `.path` and `.photo_count` from `get_status`) and offer the narrowest
  scope that serves the test:
  - *read-only*: searches, details, previews, logs. The one exception is
    `show_in_lightroom`, which fills a collection in the "LrGeniusAI Agent"
    set.
  - *indexing*: `analyze_photos` adds to the LrGeniusAI database (`db_path`
    in `GET /v1/agent/bridge`). By default that database lives next to the
    catalog. If the plugin settings name a custom location, all catalogs
    share it.
  - *changes*: ratings, flags, labels, titles, captions, keywords,
    collections and develop settings. This scope also covers
    `run_plugin_tests` with `include_catalog_writes`, and
    `LRG_LIVE_WRITES=1`. Snapshots, develop history steps, keywords and
    collections stay behind.
  - *developer commands*: `eval_lua` and `reload_plugin_modules` run code
    inside Lightroom.

  If the answer is no or unclear, stop, or ask the developer to open a test
  catalog.
- **An approval covers one catalog in this conversation.** It extends
  further only if the developer's standing instructions say so. Call
  `get_status` again before every phase that changes something. Opening
  another catalog restarts Lightroom, and the approval stays behind with the
  old catalog.
- **When changes are approved,** record the state of everything you change
  beforehand and restore it afterwards. Report every change, including what
  you restored and what remains.
- Headless checks (`cargo`, `busted`, `scripts/check-docs.py`) touch no
  catalog and need no approval.

## Setup

1. **Use the backend from this checkout.** The plugin talks to whatever
   answers on port 19819. When nothing answers, it starts the *installed*
   backend, which has no agent routes.

   ```bash
   cd server-rs && cargo build --release -p lrg-server
   curl -s -X POST http://127.0.0.1:19819/shutdown   # stops whichever backend runs there
   ./target/release/geniusai-server                  # keep it running in the background
   ```

   Requests from the plugin carry its `db_path`, and the backend binds that
   database itself. For a local build, `get_status` → `backend.version` reads
   `0.0.0-dev`.
2. **Use the plugin from this checkout.** In the Plug-in Manager, load
   `plugin/LrGeniusAI.lrdevplugin`; `get_status` →
   `lightroom.plugin_version` then reads `9.9.9`. The agent bridge starts
   with the plugin.
3. **Get the agent token.** Ask the developer to copy it (Plug-in Manager →
   LrGeniusAI → AI Agents → *Copy token*). Keep it in the environment only,
   with `export LRGENIUS_MCP_TOKEN=<token>`. Never write it into a file in the
   repository, a commit, a PR or a log you share.
4. **Pick a client.** `scripts/mcp_call.py` calls one tool per run. It prints
   the answer and writes progress to stderr. It exits with 0 on success, 1
   when the tool answers with an error, and 2 when the call itself fails.

   ```bash
   python3 scripts/mcp_call.py --list
   python3 scripts/mcp_call.py get_status
   python3 scripts/mcp_call.py search_photos '{"query": "dog on a beach", "limit": 5}'
   python3 scripts/mcp_call.py view_photo '{"photo_id": "<id>", "size": 1024}' --image-out <file>.jpg
   ```

   Open the saved image and look at it: a JPEG coming back proves little.
   You can also connect an MCP client instead. The tools then appear in a new
   session:
   `claude mcp add --transport http lrgeniusai http://127.0.0.1:19819/mcp --header "Authorization: Bearer $LRGENIUS_MCP_TOKEN"`
5. **Set up Lightroom.** Go to the Library module and show a folder or
   collection with a few dozen photos. Select one photo, because the live
   tests use the active photo. Developer commands need *Allow developer
   commands* in the Plug-in Manager, which asks for confirmation. Turn them
   on only in a test catalog, or with the developer's approval.

## Phase 0: status and the catalog gate

```bash
python3 scripts/mcp_call.py get_status
curl -s http://127.0.0.1:19819/v1/agent/bridge
```

- Check that `lightroom.connected` is `true`.
- `lightroom.socket` should read `connected`, which means the LrSocket
  doorbell works. Any other value means the plugin falls back to polling
  every second. That still works, but put it in the report: the doorbell has
  not been tested on Windows yet.
- Settle Rule 1 for `lightroom.catalog` before you go any further.
- Check that `lightroom.allow_changes` and `lightroom.developer_commands`
  match the scope you need.
- Note the versions: `backend.version`, `lightroom.plugin_version`,
  `lightroom.lightroom_version`, and `plugin.platform` from the bridge
  status.
- Note `coverage` and `index.photos`. Search, people and culling only see
  photos that are indexed.

## Phase 1: automated suites

| Suite | Command | Passes when |
|---|---|---|
| Rust | `cd server-rs && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace` | no warnings, all tests pass |
| Lua helpers | `busted` (from the repository root) | all tests pass |
| Docs | `python3 scripts/check-docs.py` | nothing fails |
| Plugin in Lightroom | `run_plugin_tests` `{}` | `failed` is 0 |
| … with catalog writes | `run_plugin_tests` `{"include_catalog_writes": true}` | `failed` is 0; copy `leftovers` into the report |
| Live MCP suite | `cd server-rs && cargo test -p lrg-mcp --test live_lightroom -- --ignored --test-threads=1` | all tests pass; `LRG_LIVE_WRITES=1` adds the writing tests |

- The writing rows need the *changes* scope.
- The live suite reads the token from `LRGENIUS_MCP_TOKEN`. Its
  developer-command test skips itself when those commands are off.
- When something fails, read `failures` first. Then read `get_logs` for
  `plugin` and for `backend` around that time.

## Phase 2: every read tool

Keep the set IDs (`s1`, `s2`, …) and photo IDs for the later phases. Wherever
a tool returns photos, open two or three with `view_photo` and judge them by
eye.

| Tool | Try | Check |
|---|---|---|
| `search_photos` | `{"query": "<a subject in this catalog>", "limit": 10}` | It returns a set ID, and the photos you open show the subject. |
| `find_similar_photos` | `{"photo_id": "<id>", "mode": "duplicates"}`, then `"similar"` | Near-identical frames come first; then scenes of the same kind. |
| `list_people`, `find_photos_of_people` | `{}`, then `{"people": ["<name>"], "match": "any"}` | Names and counts match People, and the person is in the photos. With no named people, a plain answer saying so. |
| `cull_photos` | `{"set": "view", "preset": "default"}` | It returns groups and sets for picks, alternates and reject candidates. Single photos are counted (`single_photos`), not listed. Each pick is the best frame of its group. |
| `combine_sets`, `get_set`, `make_set` | `{"a": "s1", "b": "s2", "operation": "intersection"}`; `{"set_id": "s1", "offset": 5, "limit": 5}`; `{"photo_ids": ["<id>"], "label": "test"}` | The counts are plausible and paging works. |
| `find_photos_in_catalog` | `{"flag": "pick"}`, `{"keywords": ["<keyword>"]}`, `{"captured_after": "2024-01-01"}`, `{"file_type": "raw"}`, `{"edited": true}` | The counts match Lightroom's Library Filter. |
| `get_photo_details` | `{"set": "selection"}` | Rating, flag, keywords and capture time match the Metadata panel. A photo without a label reads `none`, not `gray`. |
| `view_photo` | `{"photo_id": "<id>", "size": 1024}` | The photo looks as it does in Lightroom now, with its develop settings. |
| `list_collections` | `{}` | The list matches the Collections panel. |
| `get_develop_settings` | `{"photo_id": "<id>", "keys": ["Exposure2012", "Shadows2012"]}` | The values match the Develop panel. |
| `get_logs` | `{"source": "plugin", "lines": 50}`, `{"source": "backend", "contains": "error"}` | The logs are readable, with no errors you cannot explain. |

Every tool that takes photos accepts `set: "selection"` and `set: "view"`.
`selection` is what is selected in Lightroom. `view` is every photo in the
active folders and collections, and the filter bar does not apply.

## Phase 3: changes (in a test catalog, or with the *changes* scope)

Before each change, record what it will overwrite with `get_photo_details` or
`get_develop_settings`. Afterwards:

- read the tool's answer, including `warnings`;
- confirm the change with a separate read (`get_photo_details` or
  `find_photos_in_catalog`);
- look at Lightroom where it matters.

1. **Metadata.** Run `update_photos` on two or three photos with `rating`,
   `flag`, `color_label`, `title`, `caption` and
   `add_keywords: ["LrGeniusAI Tests > Agent Run"]`. Then run it again with
   `remove_keywords`. Each call should be one step under *Edit > Undo*.
   Restore the recorded values.
2. **Show.** Run `show_in_lightroom` `{"set": "s1", "collection": "Agent check"}`.
   Lightroom should switch to that collection, inside the "LrGeniusAI Agent"
   set, with the photos selected. From now on, `view` means that collection.
3. **Collections.** Run `manage_collection`
   `{"set": "s1", "collection": "Agent Run", "collection_set": "LrGeniusAI Tests"}`.
   It creates the set if it is missing. Check the result with
   `list_collections` and `find_photos_in_catalog {"collection": "Agent Run"}`,
   then repeat the call with `"remove": true`.
4. **Develop.** Use one photo:
   1. Read it with `get_develop_settings` and `view_photo`.
   2. Run `apply_develop_settings` with an absolute value
      (`{"settings": {"Exposure2012": 0.5}}`), then with a relative one
      (`{"settings": {"Exposure2012": 0.3}, "relative": true}`). The values
      must add up.
   3. Run `view_photo` again. The photo must look brighter than in the first
      view.
   4. A snapshot named "Before LrGeniusAI agent edit …" must exist.
   5. Put the recorded values back with an absolute call. The history steps
      and snapshots remain.
5. **Cull round trip.**
   1. Run `cull_photos`.
   2. Run `update_photos` with `flag: "pick"` on the picks set and with
      `flag: "reject"` on the reject candidates.
   3. `find_photos_in_catalog {"flag": "pick"}` must count at least the
      picks.
   4. Restore the flags.
6. **Indexing** (needs the *indexing* scope). Run `analyze_photos` on a few
   dozen photos with `"tasks": ["embeddings", "faces"]`:
   - one progress line per photo must arrive on stderr;
   - afterwards, `get_status` must show higher coverage;
   - `{"tasks": ["metadata"]}` must be refused, with a pointer to Analyze &
     Index.

## Phase 4: safety switches and failure paths

| Check | How | Expected |
|---|---|---|
| Read-only | The developer unticks *Allow agents to change photos*. With developer commands, you can do it yourself: `eval_lua` `prefs.agentBridgeAllowChanges = false; AgentBridge.refreshRegistration(); return true` | `get_status` shows `allow_changes: false`. `update_photos` is refused with the read-only message, but `show_in_lightroom` still works. Switch back and check again. |
| Developer commands off | The developer unticks *Allow developer commands*. | `eval_lua` is refused with "Developer commands are off…". |
| Bad input | `update_photos {"photo_ids": ["x"], "flag": "maybe"}`; `get_set {"set_id": "s999"}` | The message names the valid values or the unknown set; exit code 1. |
| Wrong token | `LRGENIUS_MCP_TOKEN=wrong python3 scripts/mcp_call.py get_status` | "Missing or wrong LrGeniusAI agent token…"; exit code 2. |
| Browsers kept out | `curl -s -o /dev/null -w '%{http_code}\n' -H 'Origin: http://example.com' http://127.0.0.1:19819/v1/agent/bridge` | `403` |
| One command at a time | Start a long `analyze_photos`, then call `get_photo_details` from a second shell. | The second call waits, then says what Lightroom is busy with. |
| Stop button | While a long command runs, ask the developer to click the X on *LrGeniusAI: AI agent at work*. | The command ends early and a bezel says the agent stopped. After that every tool answers "agent bridge is not running". Untick and tick *Enable the agent bridge* to start it again. |
| Reload Plug-in | The developer clicks *Reload Plug-in*. | Within seconds `GET /v1/agent/bridge` shows one session, and tools work with the same token. `get_logs {"contains": "took over"}` shows the old loop stopping. |
| Backend restart | `curl -X POST http://127.0.0.1:19819/shutdown`, then start the backend again. | The plugin registers again within about 5 s, `mcp_call.py` reconnects by itself, and the token is unchanged. |

## Phase 5: timing

Time these with `time` and report each one together with the catalog size:

- The first call after the bridge starts that maps backend photo IDs to
  catalog photos, for example `show_in_lightroom` on a search result. The
  plugin builds its ID cache from the whole catalog on that call.
- `get_photo_details {"set": "view"}` on a large folder.
- `analyze_photos`, in photos per minute.

For reference: before `Util.indexCachedPhotoIds`, the first ID match took
23 s on 14,884 photos. A time that climbs back towards that is a regression.

## Phase 6: clean up and report

Clean up only what this run created, and only where changes are allowed:

- restore the ratings, flags, labels, titles, captions and develop values you
  recorded;
- take the test photos out of the test collections;
- leave everything else as it was, above all anything this run did not
  create.

Some things cannot be removed through the SDK, or are not worth the risk of
removing. List them for the developer:

- keywords such as `LrGeniusAI Tests > Agent Bridge`;
- collections in the `LrGeniusAI Tests` and `LrGeniusAI Agent` sets;
- develop history steps;
- snapshots named "Before LrGeniusAI agent edit …".

The report contains:

- **Environment:** the catalog name and path, whether it is a test or a
  production catalog (and if production, the approved scope), the Lightroom,
  plugin and backend versions, the platform, and whether the doorbell
  connected or the plugin polled.
- **Results:** a table of phase, check, result (pass, fail, or skipped with
  the reason) and evidence (counts, messages, timings).
- **Failures:** for each one, what happened, the relevant `get_logs` lines,
  the likely cause and the file to fix.
- **Changes:** every change made to the catalog, with what was restored and
  what remains.
- **Rough edges:** anything a user would find confusing, such as messages,
  dialogs or waits.

## Development loop

- **Plugin Lua.** After changing `AgentCommands.lua`, `PluginTests.lua`,
  `Util.lua` or another module, run
  `reload_plugin_modules {"modules": ["AgentCommands"]}`, then
  `run_plugin_tests {"suite": "agent_commands"}`. `Task*.lua` files need no
  reload. Both of these are developer commands.
- **`AgentBridge.lua`.** Reloading the module does not change the loop that is
  already running. Restart the bridge with this `eval_lua` snippet, or click
  *Reload Plug-in*. The new session registers within a second, with the same
  token.

  ```lua
  AgentBridge.stop()
  LrFunctionContext.postAsyncTaskWithContext("restart agent bridge", function()
    for _ = 1, 150 do
      if not AgentBridge.isRunning() then break end
      LrTasks.sleep(0.2)
    end
    AgentBridge.start()
  end)
  return "restarting"
  ```

- **Backend.** Rebuild, run `curl -X POST http://127.0.0.1:19819/shutdown`,
  and start the new binary. The plugin registers again by itself and
  `mcp_call.py` reconnects.
- **A new tool.** Follow *Adding a tool* in `docs/wiki/Dev-Agent-Bridge.md`,
  then add the tool to Phase 2 or Phase 3 of this file.

## Pitfalls

- Lightroom's Lua has no `print`, `setfenv` or `getfenv`. `eval_lua` passes
  in a `print` of its own; `return` a value to see it.
- A setting changed in the Plug-in Manager just before *Reload Plug-in* can be
  lost. Check it in `get_status` (`allow_changes`, `developer_commands`)
  rather than assuming it took.
- If two bridge sessions keep taking turns in the backend log after a reload,
  the old loop is running code from before the takeover check. Only
  restarting Lightroom ends it.
- MCP request IDs must be unique within a session: reusing one ends the other
  call's stream. `mcp_call.py` picks random IDs.
- The plugin runs one bridge command at a time. A long `analyze_photos` or
  `run_plugin_tests` holds up every other Lightroom tool; tools that only use
  the index still answer.
- `show_in_lightroom` changes the active source, and with it what `view`
  means.
- Never stop processes by pattern (`pkill -f`): a pattern such as a catalog
  path can match Lightroom itself. Stop the backend with `/shutdown` or by
  its PID.
