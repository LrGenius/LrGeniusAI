#!/usr/bin/env python3
"""
Generate a v2 update manifest for LrGeniusAI releases (Rust backend era).

Unlike the v1 (Python backend) manifest — which listed every individual
.py source file for the plugin to patch in place — the backend is now a
single compiled binary per platform, so the manifest instead points at
one binary asset per platform (with its sha256) that the self-updater
downloads and swaps in atomically. The plugin (still interpreted Lua) is
still listed file-by-file, since patching individual .lua files in place
remains cheap and doesn't require a backend restart.

Usage (in CI):
    python3 scripts/generate_update_manifest.py \
        --version v2.16.0 \
        --repo LrGenius/LrGeniusAI \
        --binary macos-arm64=update-assets/lrgenius-server-macos-arm64 \
        --binary windows-x64=update-assets/lrgenius-server-windows-x64.exe \
        --output update-manifest.json
"""

import argparse
import hashlib
import json
import subprocess
import sys
from pathlib import Path

PLUGIN_DIR = Path("plugin/LrGeniusAI.lrdevplugin")
PLUGIN_EXTENSIONS = {".lua", ".txt"}

RAW_BASE = "https://raw.githubusercontent.com/{repo}/{tag}"
RELEASES_BASE = "https://github.com/{repo}/releases/tag/{tag}"
DOWNLOAD_BASE = "https://github.com/{repo}/releases/download/{tag}"

# Files that must NOT be updated via in-place patching because they
# affect the plugin identity / LR registration and require a full reload.
EXCLUDE_PLUGIN_FILES: set[str] = set()

# A dependency version bump here is the Rust-era equivalent of the old
# pyproject.toml/uv.lock signal: native library upgrades (e.g. onnxruntime)
# sometimes need installer-level changes, not just a binary swap.
DEPENDENCY_FILES = ["server-rs/Cargo.toml", "server-rs/Cargo.lock"]


def sha256_of_file(path: Path) -> str:
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(65536), b""):
            h.update(chunk)
    return h.hexdigest()


def collect_plugin_files(repo: str, tag: str) -> list[dict]:
    entries = []
    for ext in PLUGIN_EXTENSIONS:
        for path in sorted(PLUGIN_DIR.rglob(f"*{ext}")):
            rel = path.relative_to(PLUGIN_DIR)
            if rel.name in EXCLUDE_PLUGIN_FILES:
                continue
            url = f"{RAW_BASE.format(repo=repo, tag=tag)}/{PLUGIN_DIR}/{rel.as_posix()}"
            entries.append(
                {
                    "path": rel.as_posix(),
                    "url": url,
                    "sha256": sha256_of_file(path),
                    "size": path.stat().st_size,
                }
            )
    return entries


def collect_platform_binaries(repo: str, tag: str, binaries: list[str]) -> dict[str, dict]:
    platforms: dict[str, dict] = {}
    for entry in binaries:
        if "=" not in entry:
            print(f"ERROR: --binary must be PLATFORM=PATH, got: {entry}", file=sys.stderr)
            sys.exit(1)
        platform, raw_path = entry.split("=", 1)
        path = Path(raw_path)
        if not path.is_file():
            print(f"ERROR: binary not found for platform {platform}: {path}", file=sys.stderr)
            sys.exit(1)
        asset_name = path.name
        platforms[platform] = {
            "asset_name": asset_name,
            "url": f"{DOWNLOAD_BASE.format(repo=repo, tag=tag)}/{asset_name}",
            "sha256": sha256_of_file(path),
            "size": path.stat().st_size,
        }
    return platforms


def _detect_dependency_changes(current_tag: str) -> bool:
    """Return True if any dependency files changed since the previous release tag."""
    try:
        result = subprocess.run(
            ["git", "tag", "--sort=-creatordate"],
            capture_output=True,
            text=True,
            check=True,
        )
        tags = [t.strip() for t in result.stdout.splitlines() if t.strip()]
        prev_tags = [t for t in tags if t != current_tag]
        if not prev_tags:
            print("WARNING: No previous tag found; assuming breaking changes.", file=sys.stderr)
            return True
        prev_tag = prev_tags[0]

        diff = subprocess.run(
            ["git", "diff", "--name-only", prev_tag, "HEAD", "--"] + DEPENDENCY_FILES,
            capture_output=True,
            text=True,
            check=True,
        )
        changed = [f for f in diff.stdout.splitlines() if f.strip()]
        if changed:
            print(f"Dependency files changed since {prev_tag}: {changed}", file=sys.stderr)
            print("Auto-setting breaking_changes=True.", file=sys.stderr)
            return True
        return False
    except subprocess.CalledProcessError as e:
        print(f"WARNING: Could not detect dependency changes: {e}", file=sys.stderr)
        return False


def main():
    parser = argparse.ArgumentParser(description="Generate LrGeniusAI v2 update manifest")
    parser.add_argument("--version", required=True, help="Version tag (e.g. v2.16.0)")
    parser.add_argument("--repo", default="LrGenius/LrGeniusAI", help="GitHub repo in owner/name format")
    parser.add_argument("--output", default="update-manifest.json", help="Output JSON file path")
    parser.add_argument(
        "--binary",
        action="append",
        default=[],
        metavar="PLATFORM=PATH",
        help="Local path to a platform's built binary, e.g. macos-arm64=path/to/lrgenius-server. Repeatable.",
    )
    parser.add_argument(
        "--breaking",
        action="store_true",
        default=False,
        help="Mark release as requiring a full installer. Also auto-detected from git diff of Cargo.toml/Cargo.lock against the previous tag.",
    )
    args = parser.parse_args()

    tag = args.version
    version = tag.lstrip("v")
    repo = args.repo

    if not PLUGIN_DIR.exists():
        print(f"ERROR: Plugin directory not found: {PLUGIN_DIR}", file=sys.stderr)
        sys.exit(1)
    if not args.binary:
        print("WARNING: no --binary entries given; manifest will have no platform binaries.", file=sys.stderr)

    is_breaking = args.breaking or _detect_dependency_changes(tag)

    print(f"Generating update manifest for {tag}...")

    plugin_files = collect_plugin_files(repo, tag)
    platforms = collect_platform_binaries(repo, tag, args.binary)

    total_size = sum(f["size"] for f in plugin_files) + sum(p["size"] for p in platforms.values())

    manifest = {
        "version": version,
        "tag": tag,
        "update_type": "binary",
        "breaking_changes": is_breaking,
        "requires_restart": True,
        "release_url": RELEASES_BASE.format(repo=repo, tag=tag),
        "total_size_bytes": total_size,
        "file_counts": {
            "platforms": len(platforms),
            "plugin": len(plugin_files),
        },
        "platforms": platforms,
        "plugin_files": plugin_files,
    }

    output_path = Path(args.output)
    with open(output_path, "w", encoding="utf-8") as f:
        json.dump(manifest, f, indent=2, ensure_ascii=False)

    print(f"Manifest written to: {output_path}")
    print(f"  Platform binaries: {len(platforms)} ({', '.join(platforms.keys())})")
    print(f"  Plugin files:      {len(plugin_files)}")
    print(f"  Total size:        {total_size / 1024:.1f} KB")
    print(f"  Breaking:          {is_breaking}")


if __name__ == "__main__":
    main()
