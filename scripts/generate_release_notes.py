#!/usr/bin/env python3
"""
Generate AI-powered release notes for LrGeniusAI and maintain CHANGELOG.md.
Uses Google Gemini with a technical and concise tone.
"""

import os
import subprocess
import sys
import json
import re
from datetime import datetime

CHANGELOG_FILE = "CHANGELOG.md"
RELEASE_NOTES_FILE = "release_notes.md"
MODEL_ID = "gemini-3.1-flash-lite-preview"

# Static footer for the latest release notes (GitHub Release body)
STATIC_FOOTER = """
## Installers & System Integration
- The backend now runs as a persistent system service (LaunchAgent on macOS, Startup Registry on Windows).
- It starts automatically at login and remains active to manage background AI tasks even when Lightroom is closed.
- Manual management (troubleshooting):
  - Windows: Run `{commonpf}\\LrGeniusAI\\backend\\lrgenius-server.cmd`
  - macOS: Service `com.lrgenius.server` (managed via `launchctl`)

### Security & Permissions
- **Windows**: You may see a SmartScreen warning ("Windows protected your PC") during installation because the installer is not signed. Click "More info" and "Run anyway" to proceed.
- **macOS**: If Gatekeeper blocks the installer, go to **System Settings > Privacy & Security** and click **"Open Anyway"** under the Security section. Alternatively, run `xattr -d com.apple.quarantine <path-to-pkg>` in Terminal to clear the block.

## Docker Deployment
- For containerized environments, use the `LrGeniusAI-plugin-docker-backend-<version>.zip` asset which includes the pre-configured plugin and Docker setup.
"""

def run_command(cmd):
    try:
        result = subprocess.run(cmd, shell=True, check=True, capture_output=True, text=True)
        return result.stdout.strip()
    except subprocess.CalledProcessError as e:
        print(f"Error running command '{cmd}': {e.stderr}")
        return None

def get_tag_date(tag):
    date_str = run_command(f"git log -1 --format=%ai {tag}")
    if date_str:
        return date_str.split(' ')[0] # YYYY-MM-DD
    return "Unknown Date"

def get_all_tags():
    tags = run_command("git tag --sort=v:refname")
    if not tags:
        return []
    return [t for t in tags.split('\n') if t.strip()]

def get_commits(range_str):
    return run_command(f'git log {range_str} --pretty=format:"- %s" --no-merges')

def get_recorded_versions():
    if not os.path.exists(CHANGELOG_FILE):
        return []
    with open(CHANGELOG_FILE, "r") as f:
        content = f.read()
    # Match ## [v2.1.3] or ## v2.1.3 or ## [2.1.3]
    versions = re.findall(r"##\s*\[?v?([\d\.]+[\w\-]*)\]?", content)
    return versions

def generate_ai_notes(api_key, tag, commits, date):
    if not commits:
        return f"## [{tag}] - {date}\n\nNo technical changes detected."

    try:
        import google.genai as genai
        from google.genai import types
        
        client = genai.Client(api_key=api_key)
        
        system_instruction = (
            "You are a technical release manager for LrGeniusAI, an AI-powered Lightroom plugin. "
            "Your task is to generate concise, technical release notes based on a list of git commits. "
            "Group changes into logical sections: Features, Fixes, Architecture/Refactoring, and Documentation. "
            "Use a professional and efficient tone. Avoid marketing fluff. "
            "Only include significant technical changes. If a commit is a 'chore' or minor 'refactor' that doesn't impact "
            "functionality, you can group them or omit them if they are too trivial."
        )
        
        prompt = (
            f"Generate technical release notes for version {tag}.\n\n"
            "Here is the list of commits:\n"
            f"{commits}\n\n"
            "Format the output as clean Markdown starting with the heading: ## [{tag}] - {date}"
        )
        
        config = types.GenerateContentConfig(
            system_instruction=system_instruction,
            temperature=0.2,
        )
        
        response = client.models.generate_content(
            model=MODEL_ID,
            contents=prompt,
            config=config
        )
        
        return response.text
    except Exception as e:
        print(f"Error calling Gemini API for {tag}: {e}")
        return f"## [{tag}] - {date}\n\n{commits}"

def main():
    api_key = os.getenv('GEMINI_API_KEY')
    all_tags = get_all_tags()
    recorded_versions = get_recorded_versions()
    
    if not all_tags:
        print("No tags found in repository.")
        return

    # Filter out tags already in CHANGELOG.md
    # Handle version normalization (v9.9.9 -> 9.9.9) for comparison
    def normalize(v): return v.lstrip('v')
    missing_tags = [t for t in all_tags if normalize(t) not in [normalize(rv) for rv in recorded_versions]]

    new_entries = []
    
    # Check for unreleased changes since the last tag regardless of missing tags
    latest_tag = all_tags[-1]
    unreleased_commits = get_commits(f"{latest_tag}..HEAD")
    if unreleased_commits and not os.getenv('GITHUB_REF_TYPE') == 'tag':
        print(f"Detected unreleased changes since {latest_tag}. Generating preview...")
        date = datetime.now().strftime("%Y-%m-%d")
        tag = "Unreleased"
        if not api_key:
            entry = f"## [{tag}] - {date}\n\n{unreleased_commits}"
        else:
            entry = generate_ai_notes(api_key, tag, unreleased_commits, date)
        
        print("\n--- BEGIN GENERATED CHANGELOG (UNRELEASED PREVIEW) ---")
        print(entry)
        print("--- END GENERATED CHANGELOG ---\n")

    if not missing_tags:
        print("CHANGELOG.md is up to date with existing tags.")
    else:
        print(f"Found {len(missing_tags)} missing versions. Generating...")
        
        for i, tag in enumerate(missing_tags):
            date = get_tag_date(tag)
            
            # Range logic
            prev_idx = all_tags.index(tag) - 1
            if prev_idx >= 0:
                commits_range = f"{all_tags[prev_idx]}..{tag}"
            else:
                commits_range = tag
            
            commits = get_commits(commits_range)
            
            if not api_key:
                print(f"Skipping AI for {tag} (no API key)")
                entry = f"## [{tag}] - {date}\n\n{commits or 'No changes.'}"
            else:
                print(f"Generating AI notes for {tag} ({i+1}/{len(missing_tags)})...")
                entry = generate_ai_notes(api_key, tag, commits, date)
            
            print(f"\n--- BEGIN GENERATED CHANGELOG ({tag}) ---")
            print(entry)
            print("--- END GENERATED CHANGELOG ---\n")
            
            new_entries.append(entry)

        # Update CHANGELOG.md
        if new_entries:
            if not os.path.exists(CHANGELOG_FILE):
                content = "# Changelog\n\nAll notable changes to this project will be documented in this file.\n\n"
                # Reverse for bootstrap: latest at the top
                content += "\n\n".join(reversed(new_entries))
                with open(CHANGELOG_FILE, "w") as f:
                    f.write(content)
            else:
                with open(CHANGELOG_FILE, "r") as f:
                    current_content = f.read()
                
                # Find the position after the header
                header_end = current_content.find("\n\n") + 2
                if header_end < 2: header_end = 0
                
                # Insert new entries after header
                updated_content = (
                    current_content[:header_end] + 
                    "\n\n".join(reversed(new_entries)) + 
                    "\n\n" + 
                    current_content[header_end:]
                )
                with open(CHANGELOG_FILE, "w") as f:
                    f.write(updated_content)
            
            print(f"Updated {CHANGELOG_FILE}")

    # Always generate release_notes.md for the current run (GitHub Release body)
    # We take the latest section from CHANGELOG.md or the one we just generated
    current_tag = os.getenv('GITHUB_REF_NAME')
    if current_tag and os.path.exists(CHANGELOG_FILE):
        with open(CHANGELOG_FILE, "r") as f:
            content = f.read()
        
        # Extract the first ## section matching the current tag
        match = re.search(r"(##\s*\[?" + re.escape(current_tag) + r".*?)(?=\n##|$)", content, re.DOTALL)
        if match:
            latest_notes = match.group(1).strip()
            # Append footer
            with open(RELEASE_NOTES_FILE, "w") as f:
                f.write(latest_notes + "\n" + STATIC_FOOTER)
            print(f"Latest notes written to {RELEASE_NOTES_FILE}")

if __name__ == "__main__":
    main()
