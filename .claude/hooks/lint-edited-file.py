#!/usr/bin/env python3
"""PostToolUse hook: lint the file Claude just edited and feed any problems
straight back into the conversation.

The goal is to close the feedback loop *during* editing instead of waiting for
pre-commit or CI. Whatever the just-touched file is, we run the same checks CI
runs — but only on that one file, so it is fast:

  * plugin/**/*.lua        -> luacheck (uses the repo .luacheckrc) + stylua --check
  * server-rs/**/*.rs      -> cargo fmt --check (whole workspace; cheap) + clippy on the touched package
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


def _run(cmd, cwd, timeout=60):
    """Run a command. Returns (ran, returncode, output) where ran is False if
    the binary was not found (so we can fail-open)."""
    try:
        proc = subprocess.run(
            cmd,
            cwd=cwd,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            timeout=timeout,
        )
        return True, proc.returncode, proc.stdout
    except FileNotFoundError:
        return False, 0, ""
    except subprocess.TimeoutExpired:
        return False, 0, ""


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

    elif path.endswith(".rs") and rel.startswith("server-rs/"):
        server_rs = os.path.join(repo, "server-rs")
        ran, rc, out = _run(["cargo", "fmt", "--all", "--", "--check"], cwd=server_rs)
        if ran and rc != 0:
            problems.append(f"cargo fmt: file is not formatted.\n{out.strip()}")
        ran, rc, out = _run(
            ["cargo", "clippy", "--workspace", "--all-targets", "--", "-D", "warnings"],
            cwd=server_rs,
            timeout=180,
        )
        if ran and rc != 0:
            problems.append(f"cargo clippy:\n{out.strip()}")

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
