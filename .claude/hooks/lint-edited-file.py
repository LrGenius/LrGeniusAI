#!/usr/bin/env python3
"""PostToolUse hook: lint the file Claude just edited and feed any problems
straight back into the conversation.

The goal is to close the feedback loop *during* editing instead of waiting for
pre-commit or CI. Whatever the just-touched file is, we run the same checks CI
runs — but only on that one file, so it is fast:

  * plugin/**/*.lua        -> luacheck (uses the repo .luacheckrc) + stylua --check
  * server/**/*.py         -> ruff check + ruff format --check
  * TranslatedStrings_*.txt -> scripts/check_translations.py (key-set parity)

Exit codes (per the Claude Code hook protocol):
  0  -> silent success, nothing to report
  2  -> blocking; stderr is shown back to Claude so it fixes the issue now

The hook is deliberately fail-open: if a linter binary is missing or crashes
for an unrelated reason, we stay quiet (exit 0) rather than derail the session.
"""

import json
import os
import shutil
import subprocess
import sys


def _project_dir() -> str:
    return os.environ.get("CLAUDE_PROJECT_DIR") or os.getcwd()


def _run(cmd, cwd):
    """Run a command. Returns (ran, returncode, output) where ran is False if
    the binary was not found (so we can fail-open)."""
    try:
        proc = subprocess.run(
            cmd,
            cwd=cwd,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            timeout=60,
        )
        return True, proc.returncode, proc.stdout
    except FileNotFoundError:
        return False, 0, ""
    except subprocess.TimeoutExpired:
        return False, 0, ""


def _ruff_cmd(repo):
    """Prefer a ruff on PATH; fall back to `uv run` inside server/."""
    if shutil.which("ruff"):
        return ["ruff"], os.path.join(repo, "server")
    if shutil.which("uv"):
        return ["uv", "run", "ruff"], os.path.join(repo, "server")
    return None, None


def lint(path: str, repo: str):
    """Return a list of problem strings for the given file (empty == clean)."""
    problems = []
    rel = os.path.relpath(path, repo)

    if path.endswith(".lua") and rel.startswith("plugin/"):
        ran, rc, out = _run(["luacheck", path], cwd=repo)
        if ran and rc != 0:
            problems.append(f"luacheck:\n{out.strip()}")
        ran, rc, out = _run(["stylua", "--check", path], cwd=repo)
        if ran and rc != 0:
            problems.append("stylua: file is not formatted. Run `stylua " + rel + "`.")

    elif path.endswith(".py") and rel.startswith("server/"):
        base, cwd = _ruff_cmd(repo)
        if base:
            ran, rc, out = _run(base + ["check", path], cwd=cwd)
            if ran and rc == 1:
                problems.append(f"ruff check:\n{out.strip()}")
            ran, rc, out = _run(base + ["format", "--check", path], cwd=cwd)
            if ran and rc != 0:
                problems.append("ruff format: file is not formatted. Run `uv run ruff format " + rel + "`.")

    elif os.path.basename(path).startswith("TranslatedStrings_") and path.endswith(".txt"):
        checker = os.path.join(repo, "scripts", "check_translations.py")
        if os.path.exists(checker):
            ran, rc, out = _run([sys.executable, checker], cwd=repo)
            if ran and rc != 0:
                problems.append(out.strip())

    return problems


def main():
    try:
        payload = json.load(sys.stdin)
    except Exception:
        return 0

    path = (payload.get("tool_input") or {}).get("file_path")
    if not path:
        return 0
    if not os.path.isabs(path):
        path = os.path.join(_project_dir(), path)
    if not os.path.exists(path):
        return 0

    repo = _project_dir()
    problems = lint(path, repo)
    if not problems:
        return 0

    rel = os.path.relpath(path, repo)
    sys.stderr.write(
        f"Lint issues in {rel} (introduced by this edit). Please fix before continuing:\n\n"
        + "\n\n".join(problems)
        + "\n"
    )
    return 2


if __name__ == "__main__":
    sys.exit(main())
