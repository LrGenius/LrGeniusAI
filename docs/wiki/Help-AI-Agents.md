# Help: AI Agents (MCP)

LrGeniusAI can let an AI agent work with your Lightroom catalog. You chat with
an assistant such as **Claude Code**, **Claude Desktop**, **LM Studio** or
**Cursor**, and it uses LrGeniusAI's search, people, species and culling, and
works in Lightroom itself. It can find photos, look at them, rate and flag
them, add keywords, fill collections and change develop settings.

It talks to LrGeniusAI through the **Model Context Protocol (MCP)**, an open
standard many AI apps support. Everything runs on your computer: the agent
reaches LrGeniusAI's backend on `127.0.0.1`, and the backend reaches Lightroom
through the plugin.

> **⚠️ Read this first.** An agent changes your catalog on its own and can be
> wrong, including on many photos at once. Text in your photos' metadata can
> mislead it. What it looks at — photo previews, metadata, names of recognised
> people — goes to your AI app and, unless that app runs a local model, to the
> company behind it. Back up your catalog, try it on a test catalog or in
> read-only mode first, and read the [Disclaimer](Disclaimer). LrGeniusAI
> comes without any warranty.

---

## What an agent can do

Ask in plain words, for example:

- "Find the 30 best photos of Anna from last summer's holiday, without closed
  eyes, and put them in a collection."
- "Cull the photos I have selected: keep the best frame of each burst and
  reject the blurry ones."
- "Which bird species did I photograph in 2025?"
- "Put my 10 best landscape photos in a collection."
- "Make these three photos a bit warmer and lift the shadows, then show me."
- "Add the keywords *Hamburg* and *Harbour* to every photo of the harbour from
  yesterday."

Behind those requests the agent uses these tools:

| Tool | What it does |
|---|---|
| `get_status` | Whether Lightroom is connected, the open catalog, the selection, how much is indexed |
| `search_photos` | Semantic search: describe what is in the picture. Can also rank by aesthetic score and keep only some genres; with no description it returns your best photos (see [Advanced Search](Help-Advanced-Search#ranking)) |
| `find_similar_photos` | Photos that look alike, or near-duplicates |
| `list_people`, `find_photos_of_people` | People from LrGeniusAI's face recognition |
| `find_photos_in_catalog` | Lightroom's own filters: rating, flag, label, keywords, folder, dates, camera, lens |
| `cull_photos` | Groups bursts and duplicates, names winners and reject candidates |
| `get_photo_details`, `view_photo` | Metadata (including aesthetic score and genre), and the photo as Lightroom renders it now |
| `show_in_lightroom` | Fills a collection and switches Lightroom to it |
| `update_photos` | Rating, flag, colour label, title, caption, keywords, IPTC fields |
| `manage_collection`, `list_collections` | Collections |
| `get_develop_settings`, `apply_develop_settings` | Develop settings, absolute or relative |
| `analyze_photos` | Indexes photos for search, people, species and culling |
| `run_plugin_tests`, `get_logs` | For plugin development (see below) |

The agent works with **result sets**: a search answers with a short ID such as
`s3`, and the next step uses it. So thousands of matches never have to go
through the chat.

---

## Step 1: check the agent bridge is on

The **agent bridge** is the part of the LrGeniusAI plugin that lets agents
work in Lightroom. It starts with Lightroom and stays out of sight until an
agent works. Its settings are in
`File → Plug-in Manager → LrGeniusAI → AI Agents (MCP)`:

- **Enable the agent bridge** — on by default. The line below it says whether
  it is ready and how many agent requests it handled.
- **Allow agents to change photos** — see *Staying in control*.

---

## Step 2: connect your AI app

AI apps need the **agent token**, LrGeniusAI's password for agents. The same
Plug-in Manager section has a button per app. The button copies that app's
complete setup, token included, to the clipboard, ready to paste. The first
time, LrGeniusAI shows what an agent can do and asks you to confirm before it
copies anything:

- **Claude Code** — copies a command; paste it into a terminal. It looks like
  this (with your token):

  ```
  claude mcp add --transport http lrgeniusai http://127.0.0.1:19819/mcp --header "Authorization: Bearer <token>"
  ```

- **Claude Desktop** — copies a configuration. Merge it into
  `claude_desktop_config.json` (*Settings → Developer → Edit Config*), then
  restart Claude Desktop. It runs a small relay to the LrGeniusAI backend that
  is already running; it does not start a second server.
- **LM Studio, Cursor, others** — copies an `mcpServers` entry with the URL and
  the token as an `Authorization` header. Merge it into the app's `mcp.json`. In
  VS Code the same entry goes under `servers`. With a local model in LM Studio,
  nothing leaves your computer. Pick a model that is good at tool calling.

The token is shown masked in the Plug-in Manager and never in plain text.
**Copy token** copies just the token. **New token...** makes a new one; every
app set up with the old one stops working until you copy the new setup into it.

The token lives in your system's password storage (the Keychain on macOS, the
Credential Manager on Windows), like your AI provider keys.

---

## Or chat right here: Chat with Your Catalog

You do not need another app to try this. **Library → Plug-in Extras → Chat with
Your Catalog...** opens a chat page in your browser, and an AI model works with
the same tools.

The chat uses **the model chosen for Analyze & Index**:

- **A model on this computer** (`Plug-in Manager → LrGeniusAI → Local AI
  Model`), LM Studio or Ollama: nothing leaves your computer.
- **A cloud model** (OpenAI, Anthropic, Google Gemini) or **your own AI
  server**: the chat uses the key or address you entered for it. What the
  model reads — your messages, photo previews, metadata and the names of
  recognised people — goes to that company or server with every step, under
  its terms, and it costs what that provider charges. LrGeniusAI asks once per
  provider before the first such chat, and the chat page says where it runs.

If the model for Analyze & Index cannot chat, the chat uses a local model if
one is installed. It also needs the **agent bridge** (Step 1). You do not
need the token or Step 2.

**You decide what it may do on its own.** Each step the model wants to take
appears as a card. When your settings ask first, the card has buttons:

- **Allow once** — just this step.
- **Allow in this chat** — this tool, until you close the chat.
- **Always allow** — this tool from now on, in every chat.
- **Don't allow** — optionally with a note saying what to do instead, for
  example "only the three best ones". The model gets your note.

**What it may do...** at the top right sets this for every chat:

- *Ask before every step* — even searching waits for you.
- *Look freely, ask before changes* — the default. Searching and looking run on
  their own; anything that changes something asks first.
- *Also allow easy-to-undo changes* — collections, showing results and
  indexing run on their own; changes to the photos themselves (ratings,
  keywords, develop settings) ask.
- *Allow everything* — nothing asks, except large changes (below).

Per tool you can also choose *Allow*, *Ask* or *Off* (the model is not even
told about a tool that is off). And a change to more photos than your limit
(200 unless you change it) always asks, whatever else you allowed.

**Stop** ends what the model is doing: a step waiting for you is not done, and
nothing new starts. A step already running in Lightroom finishes.

Everything under *Staying in control* below applies to the chat too: read-only
mode, Undo, the develop snapshots and the stop button on the progress bar.

**Expect mistakes, more so from small models.** Models on your computer
misunderstand more often than the large ones behind Claude or ChatGPT: they
pick a wrong field, or act on more photos than you meant. Read what a card says before you allow it. Bigger models do better —
Gemma 4 12B handles one- and two-step tasks such as "find my beach photos and
show them" or "give the selected photos four stars"; Gemma 4 E4B often gets the
details of a change wrong.

Two hours after you close its browser tab, a chat is gone; open a new one from
the menu. So is every chat when LrGeniusAI's backend restarts.

---

## Staying in control

- **Watch it work.** While an agent works in Lightroom, a progress bar named
  *LrGeniusAI: AI agent at work* appears at the top left and says what the agent
  is doing.
- **Stop button.** The X on that bar stops the agent at once. An agent that is
  waiting is told that you stopped it, and the bridge stays off until you start
  it again in the Plug-in Manager (untick and tick *Enable*) or restart
  Lightroom. Unticking *Enable* turns it off for good.
- **Read-only mode.** Untick **Allow agents to change photos**. Agents can then
  search, look and show results in their own collection, but cannot change
  ratings, keywords, collections or develop settings.
- **Undo.** Every change an agent makes is one step in `Edit → Undo`, named
  after what it did (for example *LrGeniusAI agent: update photos*).
- **Develop snapshots.** Before an agent changes develop settings, each photo
  gets a snapshot named *Before LrGeniusAI agent edit …*. You can go back to it
  in the Develop module's Snapshots panel, even much later.
- **Nothing is deleted.** Lightroom's plug-in interface cannot delete photos,
  and the agent tools do not delete collections either.
- **Only your apps, only this computer.** The endpoint answers only requests
  addressed to `127.0.0.1`/`localhost` that carry the agent token. It refuses
  web pages, other accounts on the same computer and programs you have not
  given the token to.

A note on trust: an agent reads text from your catalog — captions, keywords,
file names, also on photos you got from others. Text written to trick an AI
could steer it. Keep an eye on what it changes. Read-only mode and Undo are
there for that.

---

## Limits

- Search, people, species and culling only see photos LrGeniusAI has
  **indexed**. `get_status` says how many are missing, and the agent can index
  them with `analyze_photos`.
- `analyze_photos` computes search embeddings, faces, species and culling
  signals. It does **not** write AI titles, captions or keywords. Those come
  from *Analyze & Index Photos*, which lets you review them first. An agent can
  also write metadata itself after looking at the photos.
- "The photos Lightroom is showing" (`set: "view"`) means every photo in the
  selected folders or collections. The Library filter bar is not applied.
- Develop changes work on the settings Lightroom's plug-in interface exposes
  (Basic, Tone, Presence, HSL, Detail, Effects and so on). Masks and AI
  features are out of reach.
- The bridge works with a backend on this computer, which is the default
  setup.

---

## For plugin developers: automated testing

Developers can use the same connection to test the Lightroom half of
LrGeniusAI:

- `run_plugin_tests` runs the plugin's tests inside Lightroom. They cover the
  Lua environment, the backend connection, the catalog, photo identity and
  every bridge command. Catalog-writing tests are optional and put every change
  back. The same tests run from `Help → Developer: Run Automated Tests...`.
- `get_logs` reads the plugin's and the backend's logs.
- With the plugin installed from its `.lrdevplugin` folder and **Allow
  developer commands** ticked, `eval_lua` runs Lua inside the plugin and
  `reload_plugin_modules` loads changed Lua files without restarting
  Lightroom. A released plugin never offers these. Ticking the box asks for
  confirmation: a connected agent can then run any code in Lightroom, so use
  it only with a test catalog.
- `cargo test -p lrg-mcp --test live_lightroom -- --ignored --test-threads=1`
  runs all of this from a terminal. Set `LRGENIUS_MCP_TOKEN` to the agent token
  first (**Copy token**).
- All of these work on the catalog that is open in Lightroom. Test in a test
  catalog that holds copies of your photos. Agents are told to ask before they
  test in any other catalog, and to stay within what you approve. The
  repository has a [step-by-step full test](https://github.com/LrGenius/LrGeniusAI/blob/main/.claude/skills/lrgenius-mcp-full-test/SKILL.md)
  for agents to follow.

Details: [Dev: Agent Bridge](Dev-Agent-Bridge).

---

## Troubleshooting

**The agent says "Missing or wrong LrGeniusAI agent token".** The app has no
token, or an old one. Copy its setup again from the Plug-in Manager.

**The agent says "LrGeniusAI's agent bridge is not running".** Lightroom is
closed, the bridge is off (*Enable the agent bridge*), or it was stopped from
the activity bar. Open Lightroom or turn the bridge back on.

**The agent says Lightroom is not connected.** The backend has the token but
the plugin is not answering: Lightroom may be busy with a dialog. If the
Plug-in Manager says *Waiting for the LrGeniusAI backend*, the backend is not
running: restart Lightroom or use *Restart Backend* in the Plug-in Manager.

**The status says *(polling)*.** The fast channel to Lightroom (a local
socket) is not connected, so the bridge checks for work every second instead.
Everything still works, just a little slower. A firewall that blocks local
connections can cause this.

**Claude Desktop shows "The LrGeniusAI backend is not running".** The relay
reached nothing on port 19819. Open Lightroom with the LrGeniusAI plugin
enabled; it starts the backend.

**A change failed with "read-only".** Agent changes are turned off in the
Plug-in Manager (*Allow agents to change photos*).
