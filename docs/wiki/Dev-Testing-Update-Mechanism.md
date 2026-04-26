# Testing the Update Mechanism Locally

This page documents how to test the full in-place update flow without a real GitHub release. The mechanism has three distinct layers — the manifest, the updater script, and the Lua plugin — which can each be exercised independently.

## Overview of the flow

```
Lightroom (Lua)
  └─ UpdateCheck.getLatestReleaseInfo()   → GitHub API (or stub)
  └─ UpdateCheck.fetchManifest()          → manifest JSON URL
  └─ SearchIndexAPI.applyUpdate(manifest) → POST /update/apply
        └─ services/update.py             → spawns updater.py + shuts down backend
              └─ scripts/updater.py       → downloads files, applies, restarts backend
```

The test scaffolding lives in `test_update_env/source_files/`. It contains a small set of replacement files and a pre-built `update-manifest-test.json` pointing at `localhost:8080`.

---

## Layer 1 — Updater script in isolation

Tests `server/src/scripts/updater.py` directly: download, SHA verification, backup, apply, and backend restart.

### 1. Serve the test replacement files

```bash
cd test_update_env/source_files
python3 -m http.server 8080
```

The directory layout must match the URL paths in the manifest:
- `plugin/<file>` → plugin files
- `backend_src/<file>` → backend source files

### 2. Generate a manifest with correct SHA256 hashes

Run this from the repo root (the existing `update-manifest-test.json` may have stale hashes):

```bash
python3 - <<'EOF'
import hashlib, json
from pathlib import Path

def sha(p):
    return hashlib.sha256(Path(p).read_bytes()).hexdigest()

manifest = {
    "version": "9.9.9",
    "tag": "v9.9.9",
    "update_type": "code_only",
    "breaking_changes": False,
    "requires_restart": True,
    "total_size_bytes": 0,
    "file_counts": {"plugin": 1, "backend_src": 1},
    "files": {
        "plugin": [
            {
                "path": "Init.lua",
                "url": "http://localhost:8080/plugin/Init.lua",
                "sha256": sha("test_update_env/source_files/plugin/Init.lua"),
            }
        ],
        "backend_src": [
            {
                "path": "geniusai_server.py",
                "url": "http://localhost:8080/backend_src/geniusai_server.py",
                "sha256": sha("test_update_env/source_files/backend_src/geniusai_server.py"),
            }
        ]
    }
}
Path("/tmp/test-manifest.json").write_text(json.dumps(manifest, indent=2))
print("Written to /tmp/test-manifest.json")
EOF
```

### 3. Prepare throwaway target directories

Do **not** point the updater at your working checkout — it will overwrite files.

```bash
cp -r plugin/LrGeniusAI.lrdevplugin /tmp/test-plugin
cp -r server /tmp/test-server
```

### 4. Run the updater GUI

```bash
cd server
uv run python src/scripts/updater.py \
    /tmp/test-manifest.json \
    /tmp/test-plugin \
    /tmp/test-server
```

The Tkinter progress window appears. After completion, verify:

```bash
# Replacement file should now be in place
diff /tmp/test-plugin/Init.lua test_update_env/source_files/plugin/Init.lua

# No leftover .bak files
find /tmp/test-plugin /tmp/test-server -name "*.bak"

# No leftover temp dir
ls ~/.lrgeniusai/update_tmp 2>/dev/null || echo "cleaned up OK"
```

---

## Layer 2 — Backend API endpoint

Tests `services/update.py` and the `/update/apply` route end-to-end, including the subprocess spawn and backend shutdown.

### 1. Start the backend

```bash
cd server && uv run python src/geniusai_server.py
```

### 2. POST the manifest

```bash
curl -s -X POST http://localhost:57430/update/apply \
  -H "Content-Type: application/json" \
  -d @/tmp/test-manifest.json | jq .
```

Expected response: `{"results": "Update started", "error": null, "warning": null}`

The backend logs `Spawning updater GUI` then `Requesting backend shutdown`. The updater GUI window appears and the backend process exits after ~2 seconds.

### 3. Test the in-progress guard

Send two POST requests simultaneously — the second should return an error:

```bash
curl -s -X POST http://localhost:57430/update/apply \
  -H "Content-Type: application/json" \
  -d @/tmp/test-manifest.json &
curl -s -X POST http://localhost:57430/update/apply \
  -H "Content-Type: application/json" \
  -d @/tmp/test-manifest.json
```

Expected: the second call returns `"An update is already in progress"`.

---

## Layer 3 — Breaking-changes guard

Set `"breaking_changes": true` in `/tmp/test-manifest.json` and POST it to the backend as above. The backend will still return success (it passes the manifest straight through), but when triggered from Lightroom the Lua layer (`TaskUpdate.lua:72–88`) will intercept this flag and show the "Full Installer Required" dialog instead of applying.

To test the Lua path specifically, use the Lightroom stub approach in Layer 4.

---

## Layer 4 — Full Lua → backend → updater chain in Lightroom

Exercises the complete path including `UpdateCheck.lua`, `TaskUpdate.lua`, and the backend.

### 1. Serve the manifest

Use the HTTP server from Layer 1, or drop the manifest behind any URL reachable from the machine running Lightroom.

### 2. Stub `getLatestReleaseInfo` temporarily

Add this override in `UpdateCheck.lua` (revert before committing):

```lua
function UpdateCheck.getLatestReleaseInfo()
    return {
        tag_name        = "v9.9.9",
        release_url     = "http://localhost:8080",
        manifest_url    = "http://localhost:8080/update-manifest-test.json",
        is_code_only    = true,
        is_newer        = true,
    }
end
```

### 3. Reload the plugin and trigger

In Lightroom: **Library → Plug-in Extras → Check for Updates**

The confirmation dialog should show version 9.9.9 with the file counts from the manifest. Clicking **Install** fires the full chain.

---

## Edge case checklist

| Scenario | How to trigger |
|---|---|
| SHA mismatch rejected | Edit a sha256 value in the manifest to a wrong hash |
| Stale temp dir cleaned | Pre-create `~/.lrgeniusai/update_tmp/` with junk files before running |
| Backup created and cleaned on success | Run with a real existing file at the target path; confirm no `.bak` left behind |
| Backup survives a mid-apply failure | Kill the updater process during the apply phase; confirm `.bak` file still exists |
| Backend restart fails gracefully | Remove the `geniusai_server.py` entry point from the throwaway directory |
| `breaking_changes: true` blocks apply | Set the flag in the manifest and trigger from Lightroom |
| Inline `content` field (version_info.py) | Generate a manifest with `generate_update_manifest.py` and confirm no URL is fetched for that entry |
