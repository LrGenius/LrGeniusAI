#!/usr/bin/env python3
"""Keeps the docs honest about the code.

Three checks, two of them exact and one advisory:

1. Every axum route in `server-rs/crates/lrg-api/src/routes/` has a heading in
   `docs/wiki/Dev-Backend-API.md`, and every heading in that page names a route
   that exists. Method and path both have to match.
2. Every endpoint the plugin calls (the `ENDPOINTS` table in
   `APISearchIndex.lua`) exists on the backend. This one is not really a docs
   check — it is the plugin/backend contract `CLAUDE.md` asks to keep in sync —
   but it lives here because it is the same kind of drift.
3. Doc pages whose sources have moved on since the page was last touched. This
   is a heuristic, so it only warns. Silence it by updating the page, or, when
   the page really is still correct, with a `Docs-Reviewed: <Page-Name>.md`
   trailer in the commit message.

Exit code is 1 when check 1 or 2 fails, or when check 3 fails under --strict.

    scripts/check-docs.py                 # run everything
    scripts/check-docs.py --strict        # stale pages fail too
    scripts/check-docs.py --for <path>    # which pages document this file?
"""

from __future__ import annotations

import argparse
import fnmatch
import os
import re
import subprocess
import sys
import tempfile
import tomllib
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
ROUTES_DIR = REPO / "server-rs/crates/lrg-api/src/routes"
API_DOC = REPO / "docs/wiki/Dev-Backend-API.md"
PLUGIN_API = REPO / "plugin/LrGeniusAI.lrdevplugin/APISearchIndex.lua"
DOC_MAP = REPO / "docs/doc-sources.toml"

HTTP_METHODS = ("get", "post", "put", "delete", "patch", "head", "options")

# `lrg_api::build_router` nests every route module under this prefix, so the
# path a module writes is not the path it serves. Keep in step with
# `API_PREFIX` in `server-rs/crates/lrg-api/src/lib.rs`.
API_PREFIX = "/v1"

# ...except these two, merged at the root instead. They are the unversioned
# bootstrap contract a new plug-in uses to talk to an old backend, so they are
# spelled exactly as served. Keep in step with `build_router`.
UNVERSIONED_ROUTE_FILES = frozenset({"bootstrap.rs", "update.rs"})


# --------------------------------------------------------------------------
# Extraction
# --------------------------------------------------------------------------


def normalize_path(path: str) -> str:
    """`/a/{id}/b` and `/a/<id>/b` are the same endpoint, spelled two ways.

    axum uses `{}`, the docs use `<>`. Parameter names are dropped so a rename
    on one side does not read as a missing endpoint.
    """
    path = re.sub(r"\{[^}]*\}", "{}", path)
    path = re.sub(r"<[^>]*>", "{}", path)
    return path.rstrip("/") or "/"


def _matching_paren(text: str, open_idx: int) -> int:
    """Index of the `)` closing the `(` at `open_idx`, skipping string bodies."""
    depth = 0
    i = open_idx
    while i < len(text):
        c = text[i]
        if c == '"':
            i += 1
            while i < len(text) and text[i] != '"':
                i += 2 if text[i] == "\\" else 1
        elif c == "(":
            depth += 1
        elif c == ")":
            depth -= 1
            if depth == 0:
                return i
        i += 1
    return len(text) - 1


def backend_routes() -> dict[tuple[str, str], str]:
    """(METHOD, path) -> "file.rs" for every route the axum routers register."""
    found: dict[tuple[str, str], str] = {}
    for src in sorted(ROUTES_DIR.rglob("*.rs")):
        text = src.read_text(encoding="utf-8")
        for m in re.finditer(r"\.route\(\s*\n?\s*\"([^\"]+)\"", text):
            open_idx = text.index("(", m.start())
            body = text[open_idx : _matching_paren(text, open_idx) + 1]
            path = m.group(1)
            if src.name not in UNVERSIONED_ROUTE_FILES:
                path = API_PREFIX + path
            path = normalize_path(path)
            methods = {
                v.lower()
                for v in re.findall(
                    r"\b(?:axum::routing::)?(" + "|".join(HTTP_METHODS) + r")\s*\(",
                    body,
                )
            }
            for method in methods or {"get"}:
                found[(method.upper(), path)] = src.name
    return found


def documented_routes() -> dict[tuple[str, str], int]:
    """(METHOD, path) -> line number, from the `### \\`POST /x\\`` headings."""
    found: dict[tuple[str, str], int] = {}
    for lineno, line in enumerate(API_DOC.read_text(encoding="utf-8").splitlines(), 1):
        if not line.startswith("### "):
            continue
        for method, path in re.findall(r"`([A-Z]+) (/[^`]*)`", line):
            if method.lower() in HTTP_METHODS:
                found[(method, normalize_path(path))] = lineno
    return found


def plugin_endpoints() -> dict[str, str]:
    """ENDPOINTS constant name -> path, from `APISearchIndex.lua`."""
    text = PLUGIN_API.read_text(encoding="utf-8")
    block = re.search(r"^local ENDPOINTS = \{(.*?)^\}", text, re.S | re.M)
    if not block:
        raise SystemExit("check-docs: no ENDPOINTS table found in APISearchIndex.lua")
    return {
        name: value
        for name, value in re.findall(
            r"(\w+)\s*=\s*\"(/[^\"]*)\"",
            block.group(1),
        )
    }


# --------------------------------------------------------------------------
# Checks
# --------------------------------------------------------------------------


def check_api_doc() -> list[str]:
    actual = backend_routes()
    documented = documented_routes()
    doc_rel = API_DOC.relative_to(REPO)
    problems = []

    for (method, path), src in sorted(actual.items()):
        if (method, path) in documented:
            continue
        # A route documented under a different verb is a wrong doc, not a
        # missing one — say which verb the page currently claims.
        other = sorted(m for (m, p) in documented if p == path)
        if other:
            problems.append(
                f"{doc_rel}: `{path}` is documented as {'/'.join(other)} "
                f"but {ROUTES_DIR.name}/{src} registers it as {method}"
            )
        else:
            problems.append(f"{doc_rel}: undocumented endpoint {method} {path} ({src})")

    for (method, path), lineno in sorted(documented.items(), key=lambda kv: kv[1]):
        if (method, path) not in actual and not any(p == path for (_, p) in actual):
            problems.append(
                f"{doc_rel}:{lineno}: documents {method} {path}, "
                "which no router registers"
            )
    return problems


def check_plugin_contract() -> tuple[list[str], list[str]]:
    """Returns (errors, known-gap notes)."""
    actual_paths = {path for (_, path) in backend_routes()}
    gaps = known_gaps()
    problems: list[str] = []
    notes: list[str] = []
    for name, path in sorted(plugin_endpoints().items()):
        normalized = normalize_path(path)
        # Several plugin constants are prefixes that get an id appended
        # (`/training` -> `/training/<photo_id>`), so a prefix match counts.
        if normalized in actual_paths:
            continue
        if any(p.startswith(normalized + "/") for p in actual_paths):
            continue
        if normalized in gaps:
            reason = " ".join(gaps[normalized].split())
            notes.append(f"ENDPOINTS.{name} -> {path} is a known gap: {reason}")
            continue
        problems.append(
            f'APISearchIndex.lua: ENDPOINTS.{name} = "{path}" '
            "has no matching backend route"
        )

    for endpoint in sorted(set(gaps) - {normalize_path(p) for p in plugin_endpoints().values()}):
        problems.append(
            f"docs/doc-sources.toml: known_gap {endpoint} is no longer called "
            "by the plugin — remove the entry"
        )
    return problems, notes


def check_generated_pages() -> list[str]:
    """Three wiki pages are built from READMEs; they must match their source.

    Regenerating and diffing catches both halves of the mistake: a README that
    changed without the page being rebuilt, and a page someone edited by hand
    (which the next build would silently throw away).
    """
    builder = REPO / "scripts/build-wiki-pages.sh"
    if not builder.exists():
        return []
    with tempfile.TemporaryDirectory() as tmp:
        result = subprocess.run(
            ["bash", str(builder)],
            cwd=REPO,
            env={**os.environ, "WIKI_DIR": tmp},
            capture_output=True,
            text=True,
            check=False,
        )
        if result.returncode != 0:
            return [f"scripts/build-wiki-pages.sh failed: {result.stderr.strip()}"]
        problems = []
        for built in sorted(Path(tmp).glob("*.md")):
            committed = REPO / "docs/wiki" / built.name
            if not committed.exists():
                problems.append(f"docs/wiki/{built.name} is missing; run scripts/build-wiki-pages.sh")
            elif committed.read_text(encoding="utf-8") != built.read_text(encoding="utf-8"):
                problems.append(
                    f"docs/wiki/{built.name} does not match its source README. "
                    "Edit the README, then run scripts/build-wiki-pages.sh — "
                    "hand edits to this page are overwritten."
                )
        return problems


def _git(*args: str) -> str:
    return subprocess.run(
        ["git", *args], cwd=REPO, capture_output=True, text=True, check=False
    ).stdout.strip()


def _last_touched(paths: list[str]) -> int:
    out = _git("log", "-1", "--format=%ct", "--", *paths)
    return int(out) if out else 0


def _last_reviewed(doc_name: str) -> int:
    """Newest commit whose message carries `Docs-Reviewed: <doc_name>`."""
    out = _git(
        "log",
        "--format=%ct%x00%B%x1e",
        f"--grep=Docs-Reviewed:.*{re.escape(doc_name)}",
        "--extended-regexp",
        "-1",
    )
    return int(out.split("\x00", 1)[0]) if out else 0


def _config() -> dict:
    if not DOC_MAP.exists():
        return {}
    return tomllib.loads(DOC_MAP.read_text(encoding="utf-8"))


def load_doc_map() -> dict[str, dict]:
    return _config().get("page", {})


def known_gaps() -> dict[str, str]:
    """Endpoint -> reason, for plugin calls the backend knowingly does not serve."""
    gaps = _config().get("contract", {}).get("known_gap", [])
    return {normalize_path(g["endpoint"]): g.get("reason", "") for g in gaps}


def check_freshness() -> list[str]:
    problems = []
    for page, entry in sorted(load_doc_map().items()):
        doc_path = f"docs/wiki/{page}"
        if not (REPO / doc_path).exists():
            problems.append(f"docs/doc-sources.toml: {page} does not exist")
            continue
        sources = entry.get("sources", [])
        if not sources:
            continue
        # A glob that matches nothing means a file was renamed out from under
        # the map — which would silently make the page look permanently fresh.
        for pattern in sources:
            if not list(REPO.glob(pattern)):
                problems.append(
                    f"docs/doc-sources.toml: {page} lists \"{pattern}\", "
                    "which matches no file"
                )
        doc_ts = max(_last_touched([doc_path]), _last_reviewed(page))
        src_ts = _last_touched(sources)
        if src_ts > doc_ts:
            changed = _git(
                "log",
                "--format=%h %s",
                f"--since=@{doc_ts}",
                "--",
                *sources,
            ).splitlines()
            detail = "; ".join(changed[:3]) or "sources changed"
            problems.append(f"{doc_path} may be stale — {detail}")
    return problems


def docs_for(path: str) -> list[str]:
    """Pages covering `path`. Generated pages resolve to their source README,
    since editing the page itself would be thrown away by the next build."""
    try:
        rel = str(Path(path).resolve().relative_to(REPO))
    except ValueError:
        rel = path
    return [
        entry.get("generated_from") or f"docs/wiki/{page}"
        for page, entry in sorted(load_doc_map().items())
        if any(fnmatch.fnmatch(rel, pat) for pat in entry.get("sources", []))
    ]


# --------------------------------------------------------------------------


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--strict",
        action="store_true",
        help="treat possibly-stale pages as failures too",
    )
    parser.add_argument(
        "--for",
        dest="for_path",
        metavar="PATH",
        help="print the doc pages that cover PATH, one per line, and exit",
    )
    args = parser.parse_args()

    if args.for_path:
        for page in docs_for(args.for_path):
            print(page)
        return 0

    contract_errors, gap_notes = check_plugin_contract()
    errors = check_api_doc() + contract_errors + check_generated_pages()
    warnings = check_freshness()

    for problem in errors:
        print(f"error: {problem}", file=sys.stderr)
    for problem in warnings:
        print(f"{'error' if args.strict else 'warning'}: {problem}", file=sys.stderr)
    for note in gap_notes:
        print(f"note: {note}", file=sys.stderr)

    if errors or (warnings and args.strict):
        print(
            "\nUpdate the affected page under docs/wiki/, or record that you "
            "checked it with a `Docs-Reviewed: <Page>.md` commit trailer.",
            file=sys.stderr,
        )
        return 1

    if not warnings:
        print("check-docs: docs match the code")
    return 0


if __name__ == "__main__":
    sys.exit(main())
