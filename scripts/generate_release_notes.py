#!/usr/bin/env python3
"""Generate end-user-facing release notes for a LrGeniusAI release.

GitHub's own auto-generated notes are a list of pull request titles, which read
like a commit log to the photographers who actually install this plugin. This
script rewrites them for that audience:

  1. ask GitHub for the auto-generated notes of the tag (which also resolves
     "since which previous tag" for us),
  2. pull the title/body/labels of every PR referenced in them,
  3. have a model on GitHub Models turn that into plain-language notes,
  4. wrap the result in the static download/troubleshooting sections and keep
     the technical list in a collapsed <details> block.

Everything except step 3 is deterministic, and step 3 degrades gracefully: if
the model is unreachable, disabled for the org, or returns nothing usable, the
notes fall back to GitHub's auto-generated body so a release is never blocked
on this script.

Only the standard library is used, so it runs on a bare runner.

Usage (in CI):
    python3 scripts/generate_release_notes.py \
        --repo "$GITHUB_REPOSITORY" --tag "$GITHUB_REF_NAME" \
        --manifest "update-manifest-$GITHUB_REF_NAME.json" \
        --output release_notes.md

Preview locally (needs a token with public repo read + models access):
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
MODELS_API = "https://models.github.ai/inference/chat/completions"

# Overridable so the model can be swapped without touching the workflow.
DEFAULT_MODEL = os.getenv("LRG_NOTES_MODEL", "openai/gpt-4o-mini")

# Cap on how much PR prose is fed to the model. Long PR bodies are mostly
# implementation discussion; the first paragraphs carry the intent.
MAX_PR_BODY_CHARS = 1500
MAX_PRS = 60

SYSTEM_PROMPT = """\
You write the release notes for LrGeniusAI, a plug-in for Adobe Lightroom \
Classic that adds AI photo tagging, descriptions, semantic search, culling, \
face recognition and automatic develop edits.

Your readers are photographers. Most of them are not programmers, and they do \
not know how the plug-in is built. They want to know one thing: what is \
different for me when I use this update?

Rules:
- Write about what the reader can observe: what is new, what got faster or \
more reliable, what no longer goes wrong.
- Never name internal machinery: file names, function names, crate or module \
names, API endpoints, database or library names, CI, linting, refactors.
- Translate jargon into what it does. "delta-mode indexing" is "only \
re-analysing photos that changed"; "embedding" is "the way photos are matched \
to your search words".
- Leave out changes with no effect for the reader (documentation, tests, \
build tooling, dependency bumps, internal cleanups) unless they visibly change \
speed, stability, or installation.
- One line per change, starting with a verb. No sub-bullets.
- Be specific and factual. No marketing language, no superlatives, no emoji, \
no promises about future releases.
- Only state what the input supports. If you cannot tell what a change does \
for the reader, leave it out rather than guessing.

Output format — Markdown, nothing else:
- Start with one or two sentences summarising the release in plain language. \
No heading above it.
- Then, only for the sections that actually have content, in this order: \
"### New", "### Improved", "### Fixed". Omit any section that would be empty.
- At most 8 bullets in total across all sections.
- Do not add a title, a version number, a date, download links, or a \
changelog list. Those are added separately.
"""


def log(message):
    print(message, file=sys.stderr)


def http_json(url, token, payload=None, accept="application/vnd.github+json"):
    """POST/GET JSON with a couple of retries on transient failures."""
    data = json.dumps(payload).encode() if payload is not None else None
    headers = {
        "Accept": accept,
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


def fetch_pull_requests(repo, numbers, token):
    pulls = []
    for number in numbers:
        try:
            data = http_json(f"{GITHUB_API}/repos/{repo}/pulls/{number}", token)
        except RuntimeError as error:
            log(f"WARNING: could not read PR #{number}: {error}")
            continue
        pulls.append(
            {
                "number": number,
                "title": data.get("title") or "",
                "body": (data.get("body") or "").strip()[:MAX_PR_BODY_CHARS],
                "labels": [label.get("name", "") for label in data.get("labels") or []],
            }
        )
    return pulls


def commit_subjects(tag):
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
    return [line.strip() for line in output.splitlines() if line.strip()]


def build_user_prompt(tag, pulls, subjects):
    lines = [f"Release {tag} of LrGeniusAI contains the following changes.", ""]
    if pulls:
        for pull in pulls:
            lines.append(f"--- change #{pull['number']} ---")
            lines.append(f"Title: {pull['title']}")
            if pull["labels"]:
                lines.append(f"Labels: {', '.join(pull['labels'])}")
            if pull["body"]:
                lines.append(f"Description:\n{pull['body']}")
            lines.append("")
    else:
        lines.append("Commit subjects (no pull requests were referenced):")
        lines.extend(f"- {subject}" for subject in subjects)
        lines.append("")
    lines.append(
        "Write the release notes for these changes, following your instructions."
    )
    return "\n".join(lines)


def strip_code_fence(text):
    """Models occasionally wrap the whole answer in a ```markdown fence."""
    stripped = text.strip()
    if not stripped.startswith("```"):
        return stripped
    lines = stripped.splitlines()
    if len(lines) >= 2 and lines[-1].strip().startswith("```"):
        return "\n".join(lines[1:-1]).strip()
    return stripped


def generate_summary(token, model, tag, pulls, subjects):
    """Plain-language notes from GitHub Models, or None if that is unavailable."""
    if not pulls and not subjects:
        log("WARNING: nothing to summarise (no PRs, no commits).")
        return None

    payload = {
        "model": model,
        "temperature": 0.2,
        "messages": [
            {"role": "system", "content": SYSTEM_PROMPT},
            {"role": "user", "content": build_user_prompt(tag, pulls, subjects)},
        ],
    }
    try:
        result = http_json(MODELS_API, token, payload, accept="application/json")
    except RuntimeError as error:
        log(f"WARNING: GitHub Models request failed: {error}")
        return None

    try:
        content = result["choices"][0]["message"]["content"]
    except (KeyError, IndexError, TypeError):
        log(f"WARNING: unexpected response shape from GitHub Models: {str(result)[:300]}")
        return None

    summary = strip_code_fence(content or "")
    if len(summary) < 20:
        log("WARNING: model returned an empty or too-short summary.")
        return None
    return summary


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
    parser.add_argument("--model", default=DEFAULT_MODEL, help=f"GitHub Models model id (default: {DEFAULT_MODEL})")
    parser.add_argument("--target", help="Commit-ish to generate notes against when the tag does not exist yet")
    parser.add_argument("--no-llm", action="store_true", help="Skip the model call; emit the deterministic parts only")
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

    pulls = fetch_pull_requests(args.repo, extract_pr_numbers(generated_body), token)
    subjects = [] if pulls else commit_subjects(args.tag)
    log(f"Collected {len(pulls)} pull request(s), {len(subjects)} commit subject(s).")

    summary = None
    if args.no_llm:
        log("Skipping the model call (--no-llm).")
    else:
        summary = generate_summary(token, args.model, args.tag, pulls, subjects)

    if summary is None and not args.no_llm:
        # Never block a release on this: fall back to GitHub's own notes, but
        # make the workflow log say so, since the body will read technical.
        print(
            "::warning title=Release notes::Could not generate plain-language "
            "release notes; falling back to the auto-generated changelog."
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
