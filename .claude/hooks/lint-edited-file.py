#!/usr/bin/env python3
"""PostToolUse hook: lint the file Claude just edited and feed any problems
straight back into the conversation.

The goal is to close the feedback loop *during* editing instead of waiting for
pre-commit or CI. Whatever the just-touched file is, we run the same checks CI
runs — but only on that one file, so it is fast:

  * plugin/**/*.lua        -> luacheck (uses the repo .luacheckrc) + stylua --check
  * server-rs/**/*.rs      -> cargo fmt --check (whole workspace; cheap) + clippy on the touched package

It also names the wiki pages that document the edited file (per
`docs/doc-sources.toml`), so the docs get updated in the same session as the
code instead of drifting until someone notices. That reminder fires at most
once per page per session, and not at all once the page itself has been edited.

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
import tempfile


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

    # The TranslatedStrings_*.txt parity check that used to live here was
    # dropped along with the translation files themselves: the plugin now
    # ships LOC() defaults only, so there is nothing left to keep in parity.

    return problems


def _seen_path(session: str) -> str:
    """Per-session scratch file recording which pages we already mentioned."""
    return os.path.join(
        tempfile.gettempdir(), f"lrgenius-docs-reminded-{session or 'nosession'}"
    )


def _seen(session: str) -> set:
    try:
        with open(_seen_path(session)) as fh:
            return {line.strip() for line in fh if line.strip()}
    except OSError:
        return set()


def _remember(session: str, pages) -> None:
    try:
        with open(_seen_path(session), "a") as fh:
            for page in pages:
                fh.write(page + "\n")
    except OSError:
        pass


def docs_reminder(path: str, repo: str, session: str):
    """Name the wiki pages documenting `path`, once per page per session.

    Editing a page counts as handling it, so touching the page first and the
    code afterwards does not produce a reminder for work already done.
    """
    rel = os.path.relpath(path, repo)
    checker = os.path.join(repo, "scripts", "check-docs.py")
    if not os.path.exists(checker):
        return None

    # Editing a page (or the README a page is generated from) counts as
    # having handled it.
    if rel.startswith("docs/wiki/") or rel.endswith("README.md"):
        _remember(session, [rel])
        return None

    ran, rc, out = _run([sys.executable, checker, "--for", rel], cwd=repo, timeout=20)
    if not ran or rc != 0:
        return None

    already = _seen(session)
    pages = [p for p in out.split() if p and p not in already]
    if not pages:
        return None

    _remember(session, pages)
    listed = ", ".join(pages)
    return (
        f"{rel} is documented by {listed}. Check whether this change makes the "
        "page wrong, and update it in this session if so — do not leave it for "
        "later. If the page is still accurate, say so and move on."
    )


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

    reminder = docs_reminder(path, repo, payload.get("session_id") or "")
    if reminder:
        problems.append(reminder)

    if not problems:
        return 0

    rel = os.path.relpath(path, repo)
    header = (
        f"Follow-up on {rel}:"
        if reminder and len(problems) == 1
        else f"Lint issues in {rel} (introduced by this edit). Please fix before continuing:"
    )
    sys.stderr.write(header + "\n\n" + "\n\n".join(problems) + "\n")
    return 2


if __name__ == "__main__":
    sys.exit(main())
