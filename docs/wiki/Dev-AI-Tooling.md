# Dev: AI Tooling

How AI coding agents work in this repository: where their rules live, what
Claude Code adds on top (hooks, language servers, subagents, skills), the
guard against live tests in a real catalog, and the optional graphify
knowledge graph. For the rules themselves, read [`AGENTS.md`](https://github.com/LrGenius/LrGeniusAI/blob/main/AGENTS.md).

---

## Rule files

| File | For | Holds |
|---|---|---|
| `AGENTS.md` | every coding agent (Codex reads it by itself) | **all project rules** — architecture, commands, *Key Rules*, docs, graphify. Authoritative. |
| `CLAUDE.md` | Claude Code | the line `@AGENTS.md`, which imports the file above, plus only what Claude Code alone has: hooks, language servers, subagents, skills |
| `CONTRIBUTING.md` | people | the short version; may leave details out, never contradicts `AGENTS.md` |

A rule that applies to any agent goes into `AGENTS.md`. `scripts/check-docs.py`
fails when `CLAUDE.md` loses the import or repeats `AGENTS.md` (a shared
heading, or a shared line of 60 characters or more) — that is how the two files
used to drift apart when they were kept as line-for-line copies.

---

## What Claude Code gets (`.claude/`)

| Path | What it is | Shared? |
|---|---|---|
| `settings.json` | hooks, language-server plugins, permissions | yes, in git |
| `settings.local.json` | your own: test catalogs, graphify hooks, MCP servers only you use | no, git-ignored |
| `hooks/lint-edited-file.py` | lints each edited file, runs `check-docs.py` for contract files, names the wiki pages to update | yes |
| `hooks/live-test-guard.py` | permission prompt before a live test in a catalog not listed as a test catalog | yes |
| `agents/*.md` | subagents, each on the model its work needs | yes |
| `skills/*/SKILL.md` | runbooks, each a slash command | yes |
| `rules/lrc-plugin-dev.md` | the Lightroom Classic SDK reference, loaded when a plugin `.lua` file is read | yes |

### Hooks

**`lint-edited-file.py`** (`PostToolUse`, after every Edit/Write) runs, on the
edited file alone, what CI runs:

| Edited file | Check |
|---|---|
| `plugin/**/*.lua` | `luacheck`, `stylua --check` |
| `server-rs/**/*.rs` | `cargo fmt --check`, `cargo clippy -p <crate> --all-targets` |
| `*.py` | `ruff`, syntax errors and pyflakes only (`E9`, `F`) |
| a route, `lrg-api/src/lib.rs`, the chat routes, `Dev-Backend-API.md`, `APISearchIndex.lua`, `docs/doc-sources.toml`, `CLAUDE.md`/`AGENTS.md`, the READMEs wiki pages are built from | `scripts/check-docs.py`, errors only |

A finding blocks the agent until it is fixed. Clippy runs on the touched crate
only, to stay fast; a break in a crate that depends on it shows up in
pre-commit and CI, which still check the whole workspace. Ruff is limited to
real errors because the repository has no Python style policy — wider rule sets
would report old findings on every edit. A linter that is not installed is
skipped silently; on a Python older than 3.11 the crate lookup is, too, and
clippy checks the whole workspace.

The same hook names the wiki pages that document the edited file, once per page
per session (from `docs/doc-sources.toml`), so the docs move with the code.

**`live-test-guard.py`** (`PreToolUse`, before Bash and the `lrgeniusai` MCP
tools) backs up *Live tests in Lightroom* in `AGENTS.md`. Before a call that
works on the catalog open in Lightroom it asks the backend which catalog is
open. The calls it recognises:

- running `scripts/mcp_call.py` with any tool but `get_status` (and not
  `--list`) — naming the file, as `cat` or `git add` do, is fine;
- `cargo test` runs that reach the `live_lightroom` tests: named, or through
  `--ignored`/`--include-ignored` without `-p` narrowing to packages other than
  `lrg-mcp`;
- requests to `:19819/mcp`, and the stdio proxy (`geniusai-server mcp`);
- the tools of the LrGeniusAI MCP server registered in Claude Code as
  `lrgeniusai`, except `get_status`.

Then:

- a catalog listed in `LRG_TEST_CATALOGS` passes into the normal permission
  flow;
- any other catalog becomes a permission prompt naming it, so you approve the
  call, not the agent;
- when it cannot tell (no agent token, backend down) or the hook itself fails,
  it prompts as well;
- when Lightroom is not connected, there is no catalog to protect, and the call
  passes.

It is a guard against mistakes, not a sandbox. It does not see an endpoint
reached through `$LRG_MCP_URL` or from inside another script, nor MCP clients
other than Claude Code, and an agent set on it could edit the hook. The rule
in `AGENTS.md` — ask before testing in a catalog — stays the safeguard.

Set your test catalogs — catalogs holding *copies* of photos — yourself. The
guard reads `LRG_TEST_CATALOGS` from the environment only, never from a file
an agent could change mid-session; put it in the shell you start Claude Code
from, or in `.claude/settings.local.json`, whose `env` block Claude Code passes
on (an agent must never set it):

```json
{
  "env": {
    "LRG_TEST_CATALOGS": "/Users/me/Pictures/LrG Test/LrG Test.lrcat;Scratch"
  }
}
```

Entries are separated by `;` — a path to the `.lrcat` file, or the catalog's
name (its file name without `.lrcat`). The guard reads the agent token from
`LRGENIUS_MCP_TOKEN` in the environment Claude Code was started from, or from a
`LRGENIUS_MCP_TOKEN=…` prefix on the command; it never stores it. It caches the
answer for a minute per session.

### Language servers

`settings.json` enables rust-analyzer and lua-language-server, and `.luarc.json`
sets the latter up for Lightroom's Lua 5.1. Claude asks them where a symbol is
defined and who calls it before it reads files; grep stays the tool for text
and for code behind `#[cfg(feature = "llamacpp")]`.

### Subagents

| Agent | Model | Job |
|---|---|---|
| `lrg-explore` | Haiku | finds code: definitions, callers, which route serves an endpoint, which page documents a file |
| `lrg-checks` | Haiku | runs the CI checks and reports only failures; never touches Lightroom |
| `lrg-docs-sync` | Sonnet | brings the wiki and the rule files in step with a change |
| `lrg-rules-reviewer` | Sonnet | reviews a change against *Key Rules* |
| `lrg-live-tester` | Sonnet | the mechanical phases of the live test, within an approved catalog and scope |

### Skills

| Command | Runbook for |
|---|---|
| `/lrgenius-mcp-full-test` | the full live test inside Lightroom over MCP, ending in a report |
| `/new-endpoint` | a backend route end to end: router, access token, API doc, plugin wrapper, warnings, tests, curl check |
| `/new-agent-command` | an agent-bridge command and its MCP tool, with annotations, tests and the contract table |
| `/release` | pre-flight, tag, watching `release.yml`, reviewing the draft release |

They are plain Markdown, so other agents and people can follow them as
checklists.

---

## Other coding agents

They read `AGENTS.md` and get none of the above automatically. After editing a
file, run what the hooks would:

```bash
pre-commit run --files <path>          # the linters
python3 scripts/check-docs.py --for <path>   # the wiki pages to update
python3 scripts/check-docs.py          # the contract checks
```

and follow the runbooks in `.claude/skills/` by hand. Nothing guards a live
test for them: the approval rule in `AGENTS.md` is all there is.

---

## graphify (optional)

[graphify](https://pypi.org/project/graphifyy/) turns the repository into a
knowledge graph: the code by its syntax tree (free, no model), the wiki and
images by a model (once, then cached). Agents query it for how the parts of
the system connect — which page describes which code, what a change touches —
where the language servers only answer for one symbol. The rules for using it
are under *graphify* in `AGENTS.md`.

Set it up once per clone:

```bash
uv tool install graphifyy
/graphify .                 # in Claude Code: builds graphify-out/ (the docs part runs a model)
graphify hook install       # keeps the code part current after commit, checkout, merge and rebase
```

- `graphify-out/` is in `.gitignore`: each developer builds their own.
  `graph.json` is about 12 MB and rewritten whole on every build.
- `graphify hook install` also writes a merge driver for `graph.json` into
  `.gitattributes`. The file is not tracked, so the driver does nothing — leave
  that line out of your commits.
- The git hooks re-extract code only. After wiki changes, `/graphify . --update`
  re-reads the changed pages.
- `graphify claude install` adds `PreToolUse` hooks that put graph hints next to
  every search and file read. If you want them, put them into your
  `.claude/settings.local.json`, not the shared `settings.json`: they name your
  local `graphify` binary, which other contributors do not have.

---

## Changing the tooling

A change under `.claude/` is a change like any other: this page lists all of it
and is mapped to those files in `docs/doc-sources.toml`, so the lint hook and
`check-docs.py` point here when one of them changes. A rule change goes into
`AGENTS.md` (and `CONTRIBUTING.md` where it covers the rule), a Claude Code
mechanism into `CLAUDE.md` and `.claude/`.
