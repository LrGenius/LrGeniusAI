#!/usr/bin/env python3
"""Generate end-user-facing release notes for a LrGeniusAI release.

GitHub's own auto-generated notes are a list of pull request titles, which read
like a commit log to the photographers who actually install this plugin. This
script rewrites them for that audience, without an AI service and without any
API key beyond the workflow's built-in GITHUB_TOKEN:

  1. ask GitHub for the auto-generated notes of the tag (which also resolves
     "since which previous tag" for us),
  2. pull the title and labels of every PR referenced in them,
  3. sort those into New / Improved / Fixed using the PR's labels and its
     conventional-commit prefix, dropping changes with no user-visible effect,
  4. wrap the result in the static download/troubleshooting sections and keep
     the technical list in a collapsed <details> block.

Everything here is deterministic: the same release always produces the same
notes. The wording of each bullet is the PR title with its `type(scope):`
prefix stripped — so a clear PR title is what makes a clear release note.

Only the standard library is used, so it runs on a bare runner.

Usage (in CI):
    python3 scripts/generate_release_notes.py \
        --repo "$GITHUB_REPOSITORY" --tag "$GITHUB_REF_NAME" \
        --manifest "update-manifest-$GITHUB_REF_NAME.json" \
        --output release_notes.md

Preview locally (needs a token with read access to the repo):
    GITHUB_TOKEN=... python3 scripts/generate_release_notes.py \
        --repo LrGenius/LrGeniusAI --tag v2.20.1 --output -
"""

import argparse
import json
import os
import re
import subprocess
import sys
import time
import urllib.error
import urllib.request

GITHUB_API = "https://api.github.com"

MAX_PRS = 60

# Section order and headings. "other" catches anything that looks user-facing
# but doesn't classify — better a vague bullet than a silently dropped change.
SECTIONS = [
    ("new", "### New"),
    ("improved", "### Improved"),
    ("fixed", "### Fixed"),
    ("other", "### Other changes"),
]

# A label wins over the title prefix: it is a deliberate human judgement, while
# the prefix is a habit. `None` means "not worth telling a photographer about".
LABEL_CATEGORY = {
    "feature": "new",
    "features": "new",
    "enhancement": "new",
    "bug": "fixed",
    "bugfix": "fixed",
    "fix": "fixed",
    "performance": "improved",
    "perf": "improved",
    "improvement": "improved",
    "documentation": None,
    "docs": None,
    "chore": None,
    "ci": None,
    "build": None,
    "test": None,
    "tests": None,
    "refactor": None,
    "internal": None,
    "dependencies": None,
}

# Scopes naming an internal area. These override the type, because the area is
# the better signal: `fix(ci):` fixes the build, not anything a photographer
# can see, whereas `fix(server):` or `fix(plugin):` usually is visible.
INTERNAL_SCOPES = {
    "ci",
    "build",
    "release",
    "docs",
    "doc",
    "deps",
    "dependencies",
    "test",
    "tests",
    "meta",
    "workflow",
    "workflows",
}

# Conventional-commit types (`feat(plugin): ...`).
TYPE_CATEGORY = {
    "feat": "new",
    "feature": "new",
    "fix": "fixed",
    "bugfix": "fixed",
    "perf": "improved",
    "improve": "improved",
    "docs": None,
    "doc": None,
    "chore": None,
    "ci": None,
    "build": None,
    "test": None,
    "tests": None,
    "refactor": None,
    "style": None,
}

# Fallback for titles written as a plain sentence ("Add Dependabot config").
VERB_CATEGORY = [
    (r"^(add|introduce|support|enable|implement)\b", "new"),
    (r"^(fix|resolve|correct|prevent|stop|repair)\b", "fixed"),
    (r"^(improve|speed up|reduce|optimi[sz]e|make .* faster)\b", "improved"),
    (r"^(bump|revert|refactor|rename|document|delete .*(workflow|config|file))\b", None),
]

CONVENTIONAL_PREFIX = re.compile(r"^(?P<type>[a-zA-Z]+)(?:\((?P<scope>[^)]*)\))?!?:\s*")
TRAILING_PR_NUMBER = re.compile(r"\s*\(#\d+\)\s*$")


def log(message):
    print(message, file=sys.stderr)


def http_json(url, token, payload=None):
    """POST/GET JSON with a couple of retries on transient failures."""
    data = json.dumps(payload).encode() if payload is not None else None
    headers = {
        "Accept": "application/vnd.github+json",
        "Authorization": f"Bearer {token}",
        "X-GitHub-Api-Version": "2022-11-28",
        "User-Agent": "lrgeniusai-release-notes",
    }
    if data is not None:
        headers["Content-Type"] = "application/json"

    last_error = None
    for attempt in range(3):
        request = urllib.request.Request(url, data=data, headers=headers)
        try:
            with urllib.request.urlopen(request, timeout=60) as response:
                return json.loads(response.read().decode())
        except urllib.error.HTTPError as error:
            body = error.read().decode(errors="replace")[:400]
            last_error = f"HTTP {error.code} from {url}: {body}"
            # 4xx other than rate limiting will not fix themselves.
            if error.code < 500 and error.code != 429:
                break
        except (urllib.error.URLError, TimeoutError, ValueError) as error:
            last_error = f"{type(error).__name__} from {url}: {error}"
        if attempt < 2:
            time.sleep(2 * (attempt + 1))
    raise RuntimeError(last_error or f"request to {url} failed")


def fetch_generated_notes(repo, tag, token, target=None):
    """GitHub's own release notes for the tag — the technical changelog."""
    payload = {"tag_name": tag}
    if target:
        payload["target_commitish"] = target
    result = http_json(f"{GITHUB_API}/repos/{repo}/releases/generate-notes", token, payload)
    return result.get("body", "").strip()


def extract_pr_numbers(generated_body):
    """PR numbers referenced by the generated notes, in order, de-duplicated."""
    seen = []
    for match in re.finditer(r"/pull/(\d+)", generated_body):
        number = int(match.group(1))
        if number not in seen:
            seen.append(number)
    return seen[:MAX_PRS]


def fetch_changes(repo, numbers, token):
    """Title + labels for each referenced PR."""
    changes = []
    for number in numbers:
        try:
            data = http_json(f"{GITHUB_API}/repos/{repo}/pulls/{number}", token)
        except RuntimeError as error:
            log(f"WARNING: could not read PR #{number}: {error}")
            continue
        changes.append(
            {
                "title": data.get("title") or "",
                "labels": [(label.get("name") or "").lower() for label in data.get("labels") or []],
            }
        )
    return changes


def commit_changes(tag):
    """Commit subjects since the previous tag — used when no PRs are referenced."""
    try:
        tags = subprocess.run(
            ["git", "tag", "--sort=-creatordate"],
            capture_output=True,
            text=True,
            check=True,
        ).stdout.split()
        previous = next((t for t in tags if t != tag), None)
        range_spec = f"{previous}..{tag}" if previous else tag
        output = subprocess.run(
            ["git", "log", range_spec, "--no-merges", "--pretty=format:%s"],
            capture_output=True,
            text=True,
            check=True,
        ).stdout
    except (subprocess.CalledProcessError, OSError) as error:
        log(f"WARNING: could not read commit subjects: {error}")
        return []
    return [{"title": line.strip(), "labels": []} for line in output.splitlines() if line.strip()]


def classify(change):
    """Which section a change belongs in, or None to leave it out entirely."""
    for label in change["labels"]:
        if label in LABEL_CATEGORY:
            return LABEL_CATEGORY[label]

    match = CONVENTIONAL_PREFIX.match(change["title"])
    if match:
        scope = (match.group("scope") or "").strip().lower()
        if scope in INTERNAL_SCOPES:
            return None
        kind = match.group("type").lower()
        if kind in TYPE_CATEGORY:
            return TYPE_CATEGORY[kind]

    lowered = change["title"].strip().lower()
    for pattern, category in VERB_CATEGORY:
        if re.match(pattern, lowered):
            return category

    return "other"


def clean_title(title):
    """Strip the `type(scope):` prefix and tidy the sentence."""
    text = CONVENTIONAL_PREFIX.sub("", title.strip())
    text = TRAILING_PR_NUMBER.sub("", text).strip().rstrip(".")
    if text and text[0].islower():
        text = text[0].upper() + text[1:]
    return text


def lead_sentence(counts):
    """One plain sentence counting what is in the release."""
    nouns = {
        "new": ("new feature", "new features"),
        "improved": ("improvement", "improvements"),
        "fixed": ("fix", "fixes"),
        "other": ("other change", "other changes"),
    }
    parts = []
    for key, _ in SECTIONS:
        count = counts.get(key, 0)
        if count:
            singular, plural = nouns[key]
            parts.append(f"{count} {singular if count == 1 else plural}")
    if not parts:
        return None
    if len(parts) == 1:
        listed = parts[0]
    else:
        listed = ", ".join(parts[:-1]) + f" and {parts[-1]}"
    return f"This update brings {listed}."


def build_summary(changes):
    """The user-facing section, or None when nothing classified."""
    buckets = {key: [] for key, _ in SECTIONS}
    for change in changes:
        category = classify(change)
        if category is None:
            continue
        title = clean_title(change["title"])
        if title and title not in buckets[category]:
            buckets[category].append(title)

    counts = {key: len(items) for key, items in buckets.items()}
    lead = lead_sentence(counts)
    if lead is None:
        return None

    parts = [lead]
    for key, heading in SECTIONS:
        if buckets[key]:
            parts.append(heading + "\n" + "\n".join(f"- {item}" for item in buckets[key]))
    return "\n\n".join(parts)


def read_breaking_flag(manifest_path):
    """Whether this release needs the full installer, per the update manifest."""
    if not manifest_path or not os.path.exists(manifest_path):
        return None
    try:
        with open(manifest_path, encoding="utf-8") as handle:
            return bool(json.load(handle).get("breaking_changes"))
    except (OSError, ValueError) as error:
        log(f"WARNING: could not read {manifest_path}: {error}")
        return None


def download_section(tag, breaking):
    version = tag[1:] if tag.startswith("v") else tag
    lines = [
        "## Download",
        "",
        "| If you are on | Download |",
        "| --- | --- |",
        f"| Windows (64-bit) | `LrGeniusAI-windows-x64-{version}.exe` |",
        f"| macOS (Apple silicon) | `LrGeniusAI-macos-arm64-{version}.pkg` |",
        f"| Your own server or Docker | `LrGeniusAI-plugin-docker-backend-{tag}.zip` |",
        "",
    ]
    if breaking is True:
        lines += [
            "**Already have LrGeniusAI installed?** This update changes parts that "
            "the in-Lightroom updater cannot replace on its own, so please install "
            "it with the full installer above. Your catalog, settings and everything "
            "already analysed stay as they are.",
        ]
    elif breaking is False:
        lines += [
            "**Already have LrGeniusAI installed?** You do not need the installer. "
            "LrGeniusAI offers this update inside Lightroom — choose **Update Now** "
            "when it appears, then restart Lightroom.",
        ]
    else:
        lines += [
            "**Already have LrGeniusAI installed?** If LrGeniusAI offers the update "
            "inside Lightroom, choose **Update Now** and restart Lightroom. "
            "Otherwise use the installer above.",
        ]
    return "\n".join(lines)


HELP_SECTION = """\
## If your computer warns about the download

Both installers are downloaded rarely enough that Windows and macOS may not
recognise them yet. This is about how well known the file is, not about
anything found in it.

- **Windows** — if SmartScreen says "Windows protected your PC", click
  **More info**, then **Run anyway**.
- **macOS** — if the installer is blocked, open **System Settings → Privacy &
  Security** and click **Open Anyway**. In Terminal, `xattr -d
  com.apple.quarantine <path to the .pkg>` does the same thing.

Something not working? Please open an issue at
https://github.com/LrGenius/LrGeniusAI/issues — a description of what you did
and what happened is enough to get started."""


def assemble(summary, generated_body, tag, breaking):
    parts = []
    if summary:
        parts.append(summary)
    parts.append(download_section(tag, breaking))
    parts.append(HELP_SECTION)
    if generated_body:
        if summary:
            parts.append(
                "<details>\n<summary>Technical changelog</summary>\n\n"
                f"{generated_body}\n\n</details>"
            )
        else:
            # Without the plain-language summary this list is the only account
            # of what changed, so it must not be hidden behind a toggle.
            parts.append(generated_body)
    return "\n\n".join(parts) + "\n"


def main():
    parser = argparse.ArgumentParser(
        description="Generate end-user-facing release notes for a LrGeniusAI release"
    )
    parser.add_argument("--repo", default=os.getenv("GITHUB_REPOSITORY", "LrGenius/LrGeniusAI"))
    parser.add_argument("--tag", default=os.getenv("GITHUB_REF_NAME"), help="Release tag, e.g. v2.20.1")
    parser.add_argument("--manifest", help="Path to the update manifest, to tell whether the full installer is needed")
    parser.add_argument("--target", help="Commit-ish to generate notes against when the tag does not exist yet")
    parser.add_argument("--output", default="release_notes.md", help='Output file, or "-" for stdout')
    args = parser.parse_args()

    if not args.tag:
        parser.error("--tag is required (or set GITHUB_REF_NAME)")

    token = os.getenv("GITHUB_TOKEN")
    if not token:
        parser.error("GITHUB_TOKEN is not set")

    try:
        generated_body = fetch_generated_notes(args.repo, args.tag, token, args.target)
    except RuntimeError as error:
        log(f"WARNING: could not fetch GitHub's generated notes: {error}")
        generated_body = ""

    changes = fetch_changes(args.repo, extract_pr_numbers(generated_body), token)
    if not changes:
        changes = commit_changes(args.tag)
    log(f"Collected {len(changes)} change(s).")

    summary = build_summary(changes)
    if summary is None:
        # Everything was internal, or there was nothing to read at all.
        print(
            "::warning title=Release notes::No user-facing changes could be "
            "identified; falling back to the technical changelog."
        )

    notes = assemble(summary, generated_body, args.tag, read_breaking_flag(args.manifest))

    if args.output == "-":
        sys.stdout.write(notes)
    else:
        with open(args.output, "w", encoding="utf-8") as handle:
            handle.write(notes)
        log(f"Wrote {args.output} ({len(notes)} characters, summary={'yes' if summary else 'no'}).")


if __name__ == "__main__":
    main()
