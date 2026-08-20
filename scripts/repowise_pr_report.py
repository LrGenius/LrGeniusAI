#!/usr/bin/env python3
"""Render a repowise report for one pull request, as Markdown on stdout.

Scoped deliberately narrowly. `repowise health` reports 831 findings for this
repo; a bot that pasted those into every PR would be scrolled past by the
second review and muted by the fourth. This reports only:

  * the change's own defect-risk read (`repowise risk`), which is about the
    diff and its files' fix history, not the repo;
  * health findings in files this PR actually touched;
  * how each touched file's score *moved*, by running health at the merge base
    as well as at the head.

The delta is the point. An absolute score of 4.1 on a file the PR barely
touched is not news; the same file dropping from 6.0 to 4.1 in this PR is.

Exits 0 even when repowise fails: this is advisory, and a broken report must
not be able to block a merge. It says so in the output instead.
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
import tempfile
from pathlib import Path

# Matches the repo's own escalation rule for user-facing lists: show a handful,
# count the rest, never hide the remainder silently.
MAX_ROWS = 12
MAX_FINDINGS = 15
SEVERITY_ORDER = {"critical": 0, "high": 1, "medium": 2, "low": 3}
SEVERITY_ICON = {"critical": "🔴", "high": "🟠", "medium": "🟡", "low": "⚪"}

# Only files repowise scores; everything else (docs, lockfiles, workflows) is
# listed as touched but has no health to report.
CODE_SUFFIXES = {".rs", ".lua", ".py", ".swift", ".sh", ".ts", ".tsx", ".js"}


def run(cmd: list[str], cwd: Path | None = None) -> tuple[int, str]:
    proc = subprocess.run(
        cmd, cwd=cwd, capture_output=True, text=True, check=False
    )
    return proc.returncode, proc.stdout


def run_json(cmd: list[str], cwd: Path | None = None):
    """Run a repowise command and parse its JSON.

    `repowise health` prints a human banner line before the payload, so the
    parse starts at the first brace rather than at byte zero.
    """
    code, out = run(cmd, cwd=cwd)
    if code != 0:
        return None
    start = out.find("{")
    if start == -1:
        return None
    try:
        return json.loads(out[start:])
    except json.JSONDecodeError:
        return None


def changed_files(base: str, head: str, repo: Path) -> list[str]:
    # Three dots: what the PR added relative to the merge base, not everything
    # that landed on the base branch meanwhile.
    code, out = run(["git", "diff", "--name-only", f"{base}...{head}"], cwd=repo)
    if code != 0:
        return []
    return [line.strip() for line in out.splitlines() if line.strip()]


def health_scores(payload) -> dict[str, dict]:
    if not payload:
        return {}
    return {m["file_path"]: m for m in payload.get("metrics", [])}


def ordinal(n: float) -> str:
    """1st, 2nd, 3rd, 11th, 91st — the naive f"{n}th" got these wrong."""
    i = int(round(n))
    if 10 <= i % 100 <= 20:
        suffix = "th"
    else:
        suffix = {1: "st", 2: "nd", 3: "rd"}.get(i % 10, "th")
    return f"{i}{suffix}"


def fmt_score(value) -> str:
    return "—" if value is None else f"{value:.2f}"


def delta_cell(before, after) -> str:
    if before is None and after is None:
        return "—"
    if before is None:
        return f"new ({fmt_score(after)})"
    if after is None:
        return "removed"
    diff = after - before
    if abs(diff) < 0.005:
        return "±0"
    # Health is "higher is better", so a rise is the good direction.
    return f"{'📈' if diff > 0 else '📉'} {diff:+.2f}"


def finding_key(f: dict) -> tuple:
    """Identity of a finding, independent of where it sits and how it reads.

    Deliberately excludes both the line numbers and the `reason` text. Line
    numbers move in any edit, and the reason embeds live counts — "is 200
    lines long", "co-changes with 20 distinct files", "32 bug-fixes touched
    this file". Keying on either marks a finding as newly introduced when all
    that happened is that a number ticked by one, which floods the "new"
    section with things the PR did not do.
    """
    return (
        f.get("file_path"),
        f.get("function_name"),
        f.get("biomarker_type"),
    )


def sort_findings(findings: list[dict]) -> list[dict]:
    return sorted(
        findings,
        key=lambda f: (
            SEVERITY_ORDER.get(f.get("severity", "low"), 9),
            -(f.get("health_impact") or 0),
        ),
    )


def finding_line(f: dict) -> str:
    sev = f.get("severity", "low")
    fn = f.get("function_name")
    where = f"`{f['file_path']}`" + (f" · `{fn}`" if fn else "")
    return f"- {SEVERITY_ICON.get(sev, '⚪')} **{sev}** {where} — {f.get('reason', '')}"


def render(base: str, head: str, repo: Path) -> str:
    files = changed_files(base, head, repo)
    if not files:
        return "### repowise\n\nNo changed files to analyse."

    risk = run_json(
        ["repowise", "risk", f"{base}..{head}", "--format", "json"], cwd=repo
    )
    head_health = run_json(["repowise", "health", "--format", "json"], cwd=repo)

    # Health at the merge base, from a throwaway worktree so the checkout under
    # test is never touched.
    base_health = None
    with tempfile.TemporaryDirectory() as tmp:
        worktree = Path(tmp) / "base"
        code, _ = run(
            ["git", "worktree", "add", "--detach", str(worktree), base], cwd=repo
        )
        if code == 0:
            base_health = run_json(
                ["repowise", "health", "--format", "json"], cwd=worktree
            )
            run(["git", "worktree", "remove", "--force", str(worktree)], cwd=repo)

    if risk is None and head_health is None:
        return (
            "### repowise\n\n"
            "_repowise produced no output for this PR; skipping the report._"
        )

    head_scores = health_scores(head_health)
    base_scores = health_scores(base_health)

    out: list[str] = ["### repowise"]

    # --- the change's own risk -------------------------------------------
    if risk:
        fh = risk.get("fix_history") or {}
        shape = risk.get("score_percentile") or risk.get("risk_percentile")
        bits = []
        if fh.get("available"):
            pct = fh.get("percentile")
            if pct is not None:
                bits.append(
                    f"touches files that have broken before "
                    f"(**{ordinal(pct)} percentile** of this repo's fix-bearing files)"
                )
        if shape is not None:
            bits.append(f"diff shape at the **{ordinal(shape)} percentile** of recent commits")
        if bits:
            out.append("")
            out.append(" · ".join(bits) + ".")

        pressure = [f for f in (fh.get("files") or []) if f.get("fix_pressure", 0) >= 1]
        if pressure:
            out.append("")
            out.append("<details><summary>Files in this PR with prior fix pressure</summary>\n")
            out.append("| File | Lines changed | Prior fixes (recency-weighted) |")
            out.append("| --- | ---: | ---: |")
            for f in pressure[:MAX_ROWS]:
                out.append(
                    f"| `{f['path']}` | {f.get('churn', 0)} | {f.get('fix_pressure', 0):.1f} |"
                )
            if len(pressure) > MAX_ROWS:
                out.append(f"\n…and {len(pressure) - MAX_ROWS} more.")
            out.append("\n</details>")

    # --- how the touched files moved --------------------------------------
    rows = []
    for path in sorted(files):
        if Path(path).suffix not in CODE_SUFFIXES:
            continue
        before = (base_scores.get(path) or {}).get("score")
        after = (head_scores.get(path) or {}).get("score")
        if before is None and after is None:
            continue
        moved = before is not None and after is not None and abs(after - before) >= 0.005
        rows.append((path, before, after, moved))

    if rows:
        # Biggest movers first; a file that did not move is context, not news.
        rows.sort(key=lambda r: -(abs((r[2] or 0) - (r[1] or 0))))
        out.append("")
        out.append("#### Health of the files this PR touches")
        out.append("")
        out.append("| File | Before | After | Change |")
        out.append("| --- | ---: | ---: | ---: |")
        for path, before, after, _ in rows[:MAX_ROWS]:
            out.append(
                f"| `{path}` | {fmt_score(before)} | {fmt_score(after)} "
                f"| {delta_cell(before, after)} |"
            )
        if len(rows) > MAX_ROWS:
            out.append("")
            out.append(f"…and {len(rows) - MAX_ROWS} more changed file(s).")
        if base_health is None:
            out.append("")
            out.append("_Base-revision health unavailable, so the before column is empty._")

    # --- findings, restricted to what this PR touched ----------------------
    touched = set(files)
    head_findings = [
        f for f in (head_health or {}).get("findings", []) if f.get("file_path") in touched
    ]
    # What the base already had, so the PR is credited only with what it added.
    # Keyed on the finding's identity rather than its position: line numbers
    # move in any edit, and keying on them would mark every finding as new.
    base_keys = {finding_key(f) for f in (base_health or {}).get("findings", [])}
    introduced = [f for f in head_findings if finding_key(f) not in base_keys]
    pre_existing = [f for f in head_findings if finding_key(f) in base_keys]

    if introduced:
        out.append("")
        out.append(f"#### New findings in this PR ({len(introduced)})")
        out.append("")
        for f in sort_findings(introduced)[:MAX_FINDINGS]:
            out.append(finding_line(f))
        if len(introduced) > MAX_FINDINGS:
            out.append(f"\n…and {len(introduced) - MAX_FINDINGS} more.")
    elif head_findings and base_health is not None:
        out.append("")
        out.append("#### New findings in this PR")
        out.append("")
        out.append("None — every finding below was already there.")

    if pre_existing:
        out.append("")
        out.append(
            f"<details><summary>Pre-existing findings in these files "
            f"({len(pre_existing)}) — not introduced here</summary>\n"
        )
        for f in sort_findings(pre_existing)[:MAX_FINDINGS]:
            out.append(finding_line(f))
        if len(pre_existing) > MAX_FINDINGS:
            out.append(f"\n…and {len(pre_existing) - MAX_FINDINGS} more.")
        out.append("\n</details>")

    out.append("")
    out.append(
        "<sub>Advisory only — this check never fails a build. "
        "Health is 1–10, higher is better.</sub>"
    )
    return "\n".join(out)


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--base", required=True, help="Merge-base ref or SHA.")
    ap.add_argument("--head", default="HEAD", help="Head ref or SHA.")
    ap.add_argument("--repo", default=".", help="Path to the git repository.")
    args = ap.parse_args()
    try:
        print(render(args.base, args.head, Path(args.repo).resolve()))
    except Exception as exc:  # noqa: BLE001 - advisory tool, never blocks
        print(f"### repowise\n\n_Report failed: {exc}_")
    return 0


if __name__ == "__main__":
    sys.exit(main())
