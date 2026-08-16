# FAQ — Frequently Asked Questions

## General

### What is LrGeniusAI?

LrGeniusAI is an AI extension for Adobe Lightroom Classic. It adds AI-powered metadata generation (keywords, titles, captions), semantic free-text search, automatic develop edits, image culling, face/person management, and more — all running locally as a background server without freezing Lightroom.

### Does it send my photos to the cloud?

Only if you choose a cloud provider (ChatGPT/OpenAI, Google Gemini; Vertex AI was removed in August 2026). With a local provider — the built-in llama.cpp and MLX engines, or an external Ollama / LM Studio server — your photos never leave your machine. For cloud providers, images are sent to the respective API for analysis. Embeddings and all generated metadata are always stored locally.

### Which Lightroom version is supported?

Adobe **Lightroom Classic** only. Lightroom CC (cloud) and other Lightroom versions are not supported, as the plugin relies on the Lightroom Classic SDK.

### Is it free?

Yes, LrGeniusAI is open source (AGPL-3.0). Cloud API usage (Gemini, OpenAI) incurs costs at the respective provider's standard rates. Local models are completely free to run — including the ones the backend downloads and runs itself, see [Local AI Models](Help-Local-AI-Models).

### Is there a more minimalist version?

Yes. **[LrGeniusTagAI](https://github.com/LrGenius/LrGeniusTagAI)** is our earlier, lightweight Lightroom Classic plugin that focuses solely on AI-generated tags and descriptions, without the full server, semantic search, culling, or editing features of LrGeniusAI. It's a good fit if you just want quick AI keywording with a simpler setup.

---

## Installation

### The installer is blocked by Windows or macOS — is it safe?

Yes. The installers are not code-signed (cost and complexity for an open-source project), which causes operating system warnings. This is expected behavior:

- **Windows (SmartScreen):** Click *More info* → *Run anyway*.
- **macOS (Gatekeeper):** Right-click the `.pkg` → *Open* → *Open anyway*. Or go to *System Settings → Privacy & Security → Open Anyway*.

### The backend server won't start

1. Check that you completed the installer for the backend (separate from the plugin `.lrdevplugin`).
2. On macOS, make sure you allowed the binary in *System Settings → Privacy & Security*.
3. Try starting the server manually: `lrgenius-server/lrgenius-server` (macOS) or `lrgenius-server/lrgenius-server.cmd` (Windows).
4. Check the **Plugin Manager → Backend Server** section — it shows the server status and URL.

### Can I run the backend on a different machine or in Docker?

Yes. Change the **Backend Server URL** in *Plug-in Manager → LrGeniusAI* to point to the remote host, e.g. `http://192.168.1.100:19819`. Docker Compose files are included in the repository.

---

## AI Model Setup

### Which AI model should I use?

See [Help: Choosing AI Model](Help-Choosing-AI-Model) for a full breakdown. Quick summary:

| Goal | Recommendation |
|---|---|
| Cheap bulk keywording | `gemini-2.5-flash-lite` or `gpt-5-nano` |
| Balanced default | `gemini-2.5-flash` |
| Best quality | `gemini-2.5-pro` or `gpt-5.4-pro` |
| Privacy / no API cost | Built-in `llamacpp` with Gemma 4 E4B |
| Apple Silicon, local | Built-in `mlx` with Gemma 4 E4B |

### Can I run AI analysis without installing Ollama or LM Studio?

Yes. The backend has two built-in engines — **llama.cpp** (GGUF; macOS, Windows, Linux) and **MLX** (Apple silicon). In *Plug-in Manager → LrGeniusAI* find the **Local AI Model** sections, pick a model, and click **Download**. Nothing else to install or keep running. See [Local AI Models](Help-Local-AI-Models).

### I don't see any models in the dropdown

The model list is loaded from the backend at runtime. If it's empty:
1. Make sure the backend server is running and reachable (*Plugin Manager → Status*).
2. Check that the relevant API key or local server URL is configured.
3. For Ollama/LM Studio: the local server must be started before Lightroom is opened (or before running a task).
4. For the built-in local engines: a model must be downloaded first — the **Installed** line in the Plug-in Manager tells you whether one is present.

### Why is the MLX section greyed out?

MLX needs the `lrgenius-mlx` helper next to the server binary. It ships in the official macOS
installer, so a greyed-out section usually means a source build without the `xcodebuild` step —
the status line names the exact reason. On Windows the section is llama.cpp rather than MLX.

### Where do I enter my API key?

*File → Plug-in Manager → LrGeniusAI* → scroll to the **API Keys** section. Enter your Gemini or OpenAI key there.

---

## Analyze & Index

### What does "Analyze & Index" actually do?

It does two things in one pass:
1. Sends each photo to the configured AI model to generate keywords, title, caption, and alt text.
2. Creates a semantic embedding (using SigLIP2 locally) so the photo can be found by [Advanced Search](Help-Advanced-Search).

Both steps are optional — you can run only embeddings, only metadata, or both.

### Do I have to index all photos before I can search?

Yes. Advanced Search only works on photos that have been indexed (embeddings created). Unindexed photos will not appear in search results.

### The AI generated wrong or low-quality keywords

- Try a more capable model (e.g. move from a small local model to `gemini-2.5-flash`).
- With a local model, turn **off** keyword aliases and bilingual keywords — both make the model emit structured keyword objects, which small models handle badly (often returning no keywords at all).
- Add **Photo Context** (folder names, capture date, GPS coordinates) to give the AI more information.
- Write a custom **System Prompt** in *Plug-in Manager → Prompts* to guide the output style.
- Adjust the **Temperature** slider — lower values produce more consistent output.

### How do I re-index photos that have already been indexed?

Enable **Regenerate all data (overwrite existing)** in the Analyze & Index dialog.

---

## Advanced Search

### Search returns no results

1. Make sure photos were indexed with **Create search embeddings** enabled.
2. Set the Lightroom collection sort order to **Custom Order** — otherwise results appear in random order and the best matches are not at the top.
3. Try a broader query — semantic search understands concepts, not just exact keywords.

### How does search ranking work?

Results are ranked by combining visual semantic embeddings with a text search over AI-generated metadata (keywords, caption, title, alt text). The final score reflects both visual and textual similarity to your query.

---

## AI Edit Photos

### What does AI Edit generate?

A structured Lightroom develop recipe: global adjustments (exposure, white balance, tone curve, HSL, etc.) and optional local masks (subject, sky, background). The recipe is applied via the Lightroom SDK — no raw pixel editing happens outside Lightroom.

### I want to review edits before they are applied

Enable **Review each proposed edit before applying it** in the AI Edit dialog. You will see a before/after preview for each photo and can approve, skip, or adjust the edit.

### What are "Overall look" presets?

Presets define the editing style — for example *Natural Professional*, *Moody Dramatic*, *Portrait - Skin Safe*, *Wedding - Soft Airy*. They inject a style description into the AI prompt. Choose *Custom* to write your own style instruction.

### What does "Style strength" do?

Controls how aggressively the AI applies the style. At 0% the AI makes only technical corrections; at 100% it applies the full stylized look. Default is 50%.

### Can I train the AI on my own editing style?

Yes — use **Save Edits as AI Training Examples** (*Library → Plug-in Extras*). See [Help: Train from Edits](Help-Train-From-Edits) for details.

---

## Cull Photos

### Does culling delete photos?

No. Culling is completely non-destructive. It creates Lightroom collections (*Picks*, *Alternates*, *Reject Candidates*, optional *Duplicates*) — your photos are never moved or deleted.

### Which photos need to be indexed before culling?

Photos need to be indexed with **Analyze & Index Photos** first. Face-aware ranking (eye openness, sharpness) requires face detection to have been run during indexing.

---

## People & Faces

### What does the "People" workflow do?

It lists all detected face clusters (persons) from your indexed photos, lets you assign names, and creates Lightroom collections per person. See [Help: People & Faces](Help-People-Faces).

### How do I enable face detection?

In the **Analyze & Index Photos** dialog, make sure face detection is enabled. The backend uses InsightFace for detection and clustering.

---

## Keyword Management

### What is the difference between "Deduplicate Keyword Synonyms" and "Auto De-Clutter"?

- **Auto De-Clutter** runs automatically during indexing and prevents the AI from creating near-duplicate keywords of ones already in your catalog (e.g. if `Car` exists, `Automobile` becomes `Car`).
- **Deduplicate Keyword Synonyms** is an interactive workflow you run manually to clean up synonym sprawl that already exists in your catalog.

See [Help: Keyword Deduplication and De-Clutter](Help-Keyword-Dedup-and-Declutter).

---

## Troubleshooting

### Something failed — where do I see details?

- In Lightroom: after a batch task finishes, a **Task Completion Dialog** shows per-photo errors.
- In Plugin Manager: click **Show logfile** or **Copy logs to desktop**.
- On the server: check the terminal window where `geniusai-server` is running, or the log files in the server's working directory.

### Lightroom shows "Connection Refused" or "Cannot connect to server"

The backend is not reachable:
1. Check *Plug-in Manager → Backend Server* — the status indicator shows whether the server responded.
2. Verify the server URL (default `http://127.0.0.1:19819`).
3. Restart the backend manually if it crashed.

For more detailed solutions see [Troubleshooting](Troubleshooting).

### I upgraded from an old version and search / metadata is missing

Versions before file-based `photo_id` values stored Lightroom catalog UUIDs as primary IDs,
and the backend cannot match those against your photos any more.

There is no migration — the one the plugin used to offer never worked, and has been removed.
Run **Analyze & Index Photos** over the catalog again instead. Photos already indexed under
the current IDs are skipped, so only the ones that genuinely need it are re-processed.
