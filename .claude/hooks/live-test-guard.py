#!/usr/bin/env python3
"""PreToolUse hook: no live Lightroom test in a catalog nobody approved.

AGENTS.md's *Live tests in Lightroom* rule says an agent never tests in a
developer's production catalog without their approval. This hook backs up the
part a machine can check. Before a call that works on the catalog open in
Lightroom runs, it asks the backend (`get_status`) which catalog that is:

  * a catalog listed in LRG_TEST_CATALOGS passes silently, into the normal
    permission flow;
  * any other catalog turns the call into a permission prompt that names it,
    so the developer approves it, not the agent;
  * when the catalog cannot be determined (no agent token, backend down), the
    call is put to the developer too: this guard fails closed.

When Lightroom is not connected there is no catalog to protect, and the call
passes. When the hook itself fails, it asks too: a crash would let the call
through.

Calls covered: running `scripts/mcp_call.py` (except `--list` and
`get_status`, which are how the check itself is done), `cargo test` runs that
reach the `live_lightroom` tests (named, or through `--ignored` without
narrowing to another package), requests to `:19819/mcp`, the stdio proxy
(`geniusai-server mcp`), and the tools of the LrGeniusAI MCP server when Claude
Code has it registered as `lrgeniusai` (again except `get_status`).

This is a guard against mistakes, not against a determined agent: it cannot
see the endpoint behind `$LRG_MCP_URL` or inside another script, nor other
MCP clients, and an agent could edit this file. The rule in AGENTS.md stays
the safeguard; the hook makes forgetting it hard.

LRG_TEST_CATALOGS lists test catalogs — ones holding copies, not originals —
separated by `;`: a path to the `.lrcat` file, or a catalog name (the file
name without `.lrcat`). It is read from the environment only (Claude Code
passes on the `env` block of `.claude/settings.local.json`), never from a file
the agent could edit mid-session. Only the developer sets it.

The agent token comes from LRGENIUS_MCP_TOKEN in the environment or, for a
Bash command, from a `LRGENIUS_MCP_TOKEN=…` prefix on the command itself. It
is never written anywhere, and the catalog answer is cached per session for a
minute in the temp directory (path and name only).
"""

import json
import os
import re
import subprocess
import sys
import tempfile
import time

CACHE_SECONDS = 60
MCP_PREFIX = "mcp__lrgeniusai__"
# The status check itself, and listing the tools: neither touches photos.
EXEMPT_TOOLS = frozenset({"get_status"})

# mcp_call.py being *run* — by an interpreter, or as the command itself — not
# merely named (`cat`, `git add`, `ruff check` on it are harmless).
MCP_CALL = re.compile(
    r"(?:^|[;&|(]\s*|\bpython[\d.]*\s+(?:-\S+\s+)*|\buv\s+run\s+(?:python[\d.]*\s+)?)"
    r"(?:\S*/)?mcp_call\.py\b((?:\s+--?[\w-]+)*)(?:\s+([A-Za-z_]\w*))?"
)
CARGO_TEST = re.compile(r"\bcargo\s+(?:\+\S+\s+)?test\b([^;&|]*)")
RAW_MCP = re.compile(r":19819/mcp\b")
STDIO_PROXY = re.compile(r"\b(?:geniusai-server|lrgenius-server|lrg-server)\b[^;&|]*\smcp\b")
TOKEN_PREFIX = re.compile(r"\bLRGENIUS_MCP_TOKEN=(?:'([^']*)'|\"([^\"]*)\"|(\S+))")
LIVE_PACKAGE = "lrg-mcp"


def _project_dir() -> str:
    return os.environ.get("CLAUDE_PROJECT_DIR") or os.getcwd()


def covered(tool: str, tool_input: dict) -> bool:
    """Whether this call works on the catalog open in Lightroom."""
    if tool.startswith(MCP_PREFIX):
        return tool[len(MCP_PREFIX) :] not in EXEMPT_TOOLS
    if tool != "Bash":
        return False
    command = str(tool_input.get("command") or "")
    if RAW_MCP.search(command) or STDIO_PROXY.search(command):
        return True
    for args in CARGO_TEST.findall(command):
        if reaches_live_tests(args):
            return True
    for flags, name in MCP_CALL.findall(command):
        if "--list" in flags.split():
            continue
        if name not in EXEMPT_TOOLS:
            return True
    return False


def reaches_live_tests(args: str) -> bool:
    """Whether `cargo test <args>` runs the ignored live_lightroom tests."""
    if "live_lightroom" in args:
        return True
    if "--ignored" not in args and "--include-ignored" not in args:
        return False
    packages = re.findall(r"(?:-p|--package)[\s=]+([\w-]+)", args)
    # `--workspace`, or no package at all, takes lrg-mcp's ignored tests along.
    return not packages or LIVE_PACKAGE in packages


def allowlist() -> list:
    raw = os.environ.get("LRG_TEST_CATALOGS") or ""
    return [entry.strip() for entry in raw.split(";") if entry.strip()]


def approved(catalog: dict, entries: list) -> bool:
    path = catalog.get("path") or ""
    name = (catalog.get("name") or "").casefold()
    for entry in entries:
        if entry.lower().endswith(".lrcat"):
            if path and os.path.realpath(os.path.expanduser(entry)) == os.path.realpath(path):
                return True
        elif name and entry.casefold() == name:
            return True
    return False


def agent_token(tool_input: dict):
    token = os.environ.get("LRGENIUS_MCP_TOKEN", "").strip()
    if token:
        return token
    match = TOKEN_PREFIX.search(tool_input.get("command") or "")
    return next((group for group in match.groups() if group), None) if match else None


def _cache_path(session: str) -> str:
    return os.path.join(tempfile.gettempdir(), f"lrgenius-live-catalog-{session or 'nosession'}")


def open_catalog(repo: str, token, session: str):
    """(catalog, None) with catalog None when Lightroom is not connected, or
    (None, reason) when the open catalog could not be determined."""
    try:
        with open(_cache_path(session)) as fh:
            cached = json.load(fh)
        if time.time() - cached["at"] < CACHE_SECONDS:
            return cached["catalog"], None
    except (OSError, ValueError, KeyError):
        pass

    if not token:
        return None, (
            "LRGENIUS_MCP_TOKEN is not set, so the guard could not ask which "
            "catalog is open"
        )
    script = os.path.join(repo, "scripts", "mcp_call.py")
    try:
        proc = subprocess.run(
            [sys.executable, script, "get_status"],
            cwd=repo,
            env={**os.environ, "LRGENIUS_MCP_TOKEN": token},
            capture_output=True,
            text=True,
            timeout=25,
        )
    except (OSError, subprocess.TimeoutExpired) as exc:
        return None, f"get_status did not answer ({exc.__class__.__name__})"
    if proc.returncode != 0:
        return None, f"get_status failed: {(proc.stderr or proc.stdout).strip()[:200]}"
    try:
        answer = json.loads(proc.stdout)
    except ValueError:
        return None, "get_status did not answer with JSON"
    lightroom = answer.get("lightroom") if isinstance(answer, dict) else None
    if not isinstance(lightroom, dict):
        lightroom = {}

    catalog = None
    if lightroom.get("connected"):
        found = lightroom.get("catalog")
        found = found if isinstance(found, dict) else {}
        catalog = {"path": found.get("path"), "name": found.get("name")}
        if not (catalog["path"] or catalog["name"]):
            return None, "Lightroom is connected but did not name its catalog"
    try:
        with open(_cache_path(session), "w") as fh:
            json.dump({"at": time.time(), "catalog": catalog}, fh)
    except OSError:
        pass
    return catalog, None


def ask(reason: str) -> int:
    print(
        json.dumps(
            {
                "hookSpecificOutput": {
                    "hookEventName": "PreToolUse",
                    "permissionDecision": "ask",
                    "permissionDecisionReason": reason,
                }
            }
        )
    )
    return 0


RULE = (
    "AGENTS.md: never test in a developer's production catalog without their "
    "approval. Approve only if you agreed to testing in this catalog."
)


def check(payload: dict) -> int:
    tool = str(payload.get("tool_name") or "")
    tool_input = payload.get("tool_input")
    tool_input = tool_input if isinstance(tool_input, dict) else {}
    if not covered(tool, tool_input):
        return 0

    catalog, problem = open_catalog(
        _project_dir(), agent_token(tool_input), str(payload.get("session_id") or "")
    )
    if problem:
        return ask(f"Live Lightroom test, but {problem}. {RULE}")
    if catalog is None or approved(catalog, allowlist()):
        return 0
    return ask(
        f"Live Lightroom test in catalog \"{catalog['name']}\" ({catalog['path']}), "
        f"which is not in LRG_TEST_CATALOGS. {RULE}"
    )


def main() -> int:
    try:
        payload = json.load(sys.stdin)
    except ValueError:
        return 0
    if not isinstance(payload, dict):
        return 0
    try:
        return check(payload)
    except Exception as exc:  # noqa: BLE001 — a crash would let the call through
        return ask(f"The live-test guard failed ({exc.__class__.__name__}: {exc}). {RULE}")


if __name__ == "__main__":
    sys.exit(main())
