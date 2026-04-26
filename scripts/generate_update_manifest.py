#!/usr/bin/env python3
"""
Generate a code-only update manifest for LrGeniusAI releases.

The manifest is a JSON file that lists all plugin (.lua, .txt) and backend
Python source files with their download URLs and SHA256 hashes. The plugin
uses this manifest to apply lightweight code-only updates without downloading
the full installer.

Usage (in CI):
    python3 scripts/generate_update_manifest.py \
        --version v2.15.0 \
        --repo LrGenius/LrGeniusAI \
        --output update-manifest.json
"""

import argparse
import base64
import datetime
import hashlib
import json
import sys
from pathlib import Path

PLUGIN_DIR = Path("plugin/LrGeniusAI.lrdevplugin")
BACKEND_SRC_DIR = Path("server/src")

PLUGIN_EXTENSIONS = {".lua", ".txt"}
BACKEND_EXTENSIONS = {".py"}

RAW_BASE = "https://raw.githubusercontent.com/{repo}/{tag}"
RELEASES_BASE = "https://github.com/{repo}/releases/tag/{tag}"

# Files that must NOT be updated via code-only update because they
# affect the plugin identity / LR registration and require a full reload.
EXCLUDE_PLUGIN_FILES: set[str] = set()

# version_info.py is excluded from raw URL fetching because the file in the
# repo at the tag commit still contains dev placeholders — the real values are
# baked in by CI after checkout and are NOT committed back. Instead, the
# manifest generator writes the correct content inline (base64-encoded) so the
# updater can write it directly without a download.
EXCLUDE_BACKEND_FILES = {"version_info.py"}


def sha256_of_file(path: Path) -> str:
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(65536), b""):
            h.update(chunk)
    return h.hexdigest()


def sha256_of_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def collect_plugin_files(repo: str, tag: str) -> list[dict]:
    entries = []
    for ext in PLUGIN_EXTENSIONS:
        # rglob picks up files in subdirectories too
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


def collect_backend_files(repo: str, tag: str) -> list[dict]:
    entries = []
    for ext in BACKEND_EXTENSIONS:
        for path in sorted(BACKEND_SRC_DIR.rglob(f"*{ext}")):
            rel = path.relative_to(BACKEND_SRC_DIR)
            if rel.name in EXCLUDE_BACKEND_FILES:
                continue
            url = f"{RAW_BASE.format(repo=repo, tag=tag)}/{BACKEND_SRC_DIR}/{rel.as_posix()}"
            entries.append(
                {
                    "path": rel.as_posix(),
                    "url": url,
                    "sha256": sha256_of_file(path),
                    "size": path.stat().st_size,
                }
            )
    return entries


def make_version_info_entry(version: str, tag: str) -> dict:
    """
    Generate a version_info.py entry with the correct release values embedded
    directly as base64 content (no raw URL, which would serve dev placeholders).
    """
    build_date = datetime.date.today().strftime("%Y%m%d")
    content = (
        f'BACKEND_VERSION = "{version}"\n'
        f'BACKEND_RELEASE_TAG = "{tag}"\n'
        f"BACKEND_BUILD = {build_date}\n"
    ).encode()
    return {
        "path": "version_info.py",
        "content": base64.b64encode(content).decode(),
        "sha256": sha256_of_bytes(content),
        "size": len(content),
    }


def main():
    parser = argparse.ArgumentParser(
        description="Generate LrGeniusAI code-only update manifest"
    )
    parser.add_argument("--version", required=True, help="Version tag (e.g. v2.15.0)")
    parser.add_argument(
        "--repo",
        default="LrGenius/LrGeniusAI",
        help="GitHub repo in owner/name format",
    )
    parser.add_argument(
        "--output", default="update-manifest.json", help="Output JSON file path"
    )
    parser.add_argument(
        "--breaking",
        action="store_true",
        default=False,
        help="Mark release as requiring a full installer (dependency changes)",
    )
    args = parser.parse_args()

    tag = args.version
    version = tag.lstrip("v")
    repo = args.repo

    # Validate that source directories exist
    if not PLUGIN_DIR.exists():
        print(f"ERROR: Plugin directory not found: {PLUGIN_DIR}", file=sys.stderr)
        sys.exit(1)
    if not BACKEND_SRC_DIR.exists():
        print(
            f"ERROR: Backend source directory not found: {BACKEND_SRC_DIR}",
            file=sys.stderr,
        )
        sys.exit(1)

    print(f"Generating update manifest for {tag}...")

    plugin_files = collect_plugin_files(repo, tag)
    backend_files = collect_backend_files(repo, tag)

    # Append version_info.py with baked-in release values
    backend_files.append(make_version_info_entry(version, tag))

    total_size = sum(f["size"] for f in plugin_files + backend_files)

    manifest = {
        "version": version,
        "tag": tag,
        "update_type": "code_only",
        "breaking_changes": args.breaking,
        "requires_restart": True,
        "release_url": RELEASES_BASE.format(repo=repo, tag=tag),
        "total_size_bytes": total_size,
        "file_counts": {
            "plugin": len(plugin_files),
            "backend_src": len(backend_files),
        },
        "files": {
            "plugin": plugin_files,
            "backend_src": backend_files,
        },
    }

    output_path = Path(args.output)
    with open(output_path, "w", encoding="utf-8") as f:
        json.dump(manifest, f, indent=2, ensure_ascii=False)

    print(f"Manifest written to: {output_path}")
    print(f"  Plugin files:   {len(plugin_files)}")
    print(f"  Backend files:  {len(backend_files)}")
    print(f"  Total size:     {total_size / 1024:.1f} KB")
    print(f"  Breaking:       {args.breaking}")


if __name__ == "__main__":
    main()
