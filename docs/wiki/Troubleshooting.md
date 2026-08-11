# Troubleshooting

> Tips and solutions for common issues when running LrGeniusAI.

## GUI Error Reporting

LrGeniusAI surfaces errors directly in Lightroom rather than hiding them in log files. If something fails during a batch task, a **Task Completion Dialog** shows which photos succeeded, which failed, and why. For more persistent errors, check the log file via *Plug-in Manager → Show logfile* or *Copy logs to desktop*.

---

## Common Issues

### 1. Installer & Security Warnings (SmartScreen / Gatekeeper)

Since LrGeniusAI installers are not currently code-signed, your OS may block them.

- **Symptom:** "Windows protected your PC" or "LrGeniusAI.pkg cannot be opened because it is from an unidentified developer".
- **Resolution:** Refer to the [Getting Started](Getting-Started#%E2%9A%A0%EF%B8%8F-bypassing-security-warnings-unsigned-installers) guide for OS-specific bypass steps. This is a one-time requirement during installation.

---

### 2. Plugin/Backend Version Mismatch

Using a newer plugin with an older backend (or vice versa) causes API errors.

- **Symptom:** Errors like "Failed to initialize database", "API request failed", or unexpected 500 errors immediately after an update.
- **Resolution:** Make sure **both** the plugin (`.lrdevplugin`) and the backend server are updated to the same release version. Download both from the same GitHub release. If you updated the plugin only, re-run the backend installer from the same release.

---

### 3. Missing or Not Downloading OpenCLIP Model

The backend downloads the SigLIP2 embedding model on first use (several GB). If this fails, semantic search won't work.

- **Symptom:** "OpenCLIP model is missing" warning in Lightroom, or indexing fails immediately even though the backend is running.
- **Resolution:**
  1. Check your internet connection on the machine running the backend.
  2. Make sure the backend process has write permission to its cache directory (typically `~/.cache/huggingface` or the configured data path).
  3. Wait for the download to complete — the `/status` endpoint shows `is_model_cached`. Progress is visible in the backend terminal.
  4. If the warning persists after a successful download, restart the backend and try again.
  5. If the backend server and Lightroom are on different machines, the model must be downloaded on the server machine, not your Lightroom machine.

---

### 4. OpenCLIP "Missing" Even After Installation

- **Symptom:** The plugin reports the CLIP model is missing, but it was previously working or you can see the model files on disk.
- **Resolution:** This can happen if the backend was updated and the model cache path changed, or if the model files were partially downloaded. Delete the model cache folder and let the backend re-download:
  - macOS/Linux: `~/.cache/huggingface/hub/`
  - Windows: `%USERPROFILE%\.cache\huggingface\hub\`
  
  After deleting, restart the backend. It will re-download the model on next use.

---

### 5. Invalid or Missing API Keys

If you are using cloud providers (Gemini, ChatGPT), authentication failures block all analysis.

- **Symptom:** "Unauthorized", 401/403 HTTP errors, or "Gemini API not configured" despite entering a key.
- **Resolution:**
  1. Double-check the key in *Plug-in Manager → API Keys*. Copy-paste directly from the provider dashboard to avoid whitespace.
  2. Ensure the key has not expired and has sufficient billing quota attached.
  3. For Gemini: make sure the Gemini API is enabled in your Google Cloud project.
  4. After saving a new key, restart the backend (the backend reads keys from configuration at startup).

---

### 6. Built-In Local Model (llama.cpp / MLX) Not Available

The backend can run models itself, in two independent engines: `llamacpp`
(GGUF; macOS/Windows/Linux) and `mlx` (Apple silicon only). See
[Local AI Models](Help-Local-AI-Models) for the full guide.

- **Symptom:** The **Local AI Model** section says *"This backend does not
  support MLX"* or *"MLX runs only on Apple silicon Macs"*.
  - **Resolution:** Expected on Windows, Linux, and Intel Macs. Use the
    llama.cpp section instead — it offers the same model families as GGUF.
- **Symptom:** MLX is unavailable on an Apple silicon Mac, with a reason naming
  a missing helper.
  - **Resolution:** The `lrgenius-mlx` helper ships next to the server binary in
    the macOS `.pkg`. Reinstall the backend with the official installer; if you
    build from source, the helper needs `xcodebuild` plus the Metal toolchain
    (see [Server README](Dev-Server-README)).
- **Symptom:** *"This backend build has no local-model support"*.
  - **Resolution:** The binary was compiled without the `llamacpp` feature.
    Official releases include it — reinstall from the release page, or rebuild
    with `cargo build --release -p lrg-server --features llamacpp`.
- **Symptom:** A downloaded model does not appear in the model dropdown.
  - **Resolution:** Check the **Installed** line in the Plug-in Manager. An
    interrupted download is discarded rather than offered as a broken model, so
    simply run the download again.

---

### 7. Ollama Not Responding / Not Invoked

- **Symptom:** Analysis runs but Ollama shows no activity (`ollama ps` is empty), or connection errors to `localhost:11434`.
- **Resolution:**
  1. Make sure Ollama is running before starting the task. Start it with `ollama serve` if needed.
  2. Verify the Ollama Base URL in *Plug-in Manager* matches where Ollama is listening (default: `http://localhost:11434`).
  3. Make sure at least one vision-capable model is pulled: `ollama pull qwen3-vl:4b-instruct-q4_K_M`.
  4. If Ollama runs on a different machine, use its network address instead of `localhost`.
  5. Check that no firewall or VPN is blocking the connection between the backend and Ollama.

---

### 8. LM Studio Not Responding

- **Symptom:** Errors connecting to LM Studio, or model list shows no LM Studio models.
- **Resolution:**
  1. Open LM Studio and confirm the **local server is running** (green status in the "Local Server" tab).
  2. Ensure a vision model is loaded (or enable "on-demand model loading").
  3. Verify the LM Studio Base URL in *Plug-in Manager* (default: `http://localhost:1234`).
  4. On Apple Silicon, the MLX variants of models perform significantly better than GGUF builds — try the MLX version if performance is poor.

---

### 9. Local Model Timeout

Local AI models can take significantly longer than cloud APIs, especially without a dedicated GPU.

- **Symptom:** Lightroom displays a timeout error during image analysis.
- **Resolution:**
  1. Ensure the model is fully loaded into memory before starting the batch. With Ollama/LM Studio, run a test prompt first; with the built-in engines, the first request of a session loads a multi-gigabyte model and is always the slowest.
  2. Process smaller batches (10–20 photos at a time) to avoid cumulative timeouts.
  3. Switch to a smaller model (4B instead of 12B) if your hardware can't sustain the load.
  4. For the built-in llama.cpp engine: lower **Photos in parallel** to 1, and check **Layers on the GPU** — a model that does not fit in VRAM partially runs on the CPU and is far slower.

---

### 10. No Metadata Generated

- **Symptom:** Analysis completes without errors but no keywords/title/caption appear in Lightroom.
- **Resolution:**
  1. In the Analyze & Index dialog, confirm **Generate AI metadata** is enabled.
  2. If **Regenerate all data** is disabled, photos that already have metadata in the backend won't be re-generated. Enable regeneration to force a fresh run.
  3. Check that **Apply to Lightroom catalog** (or equivalent write option) is enabled — without it, metadata is generated on the backend but never written to Lightroom.
  4. After generation, you can always use *Retrieve Metadata from Backend* to pull generated data back into Lightroom manually.

---

### 11. Search Returns No Results

- **Symptom:** Advanced Search runs but the results collection is empty or sparse.
- **Resolution:**
  1. Make sure photos were indexed with **Create search embeddings** enabled.
  2. Set the Lightroom collection sort order to **Custom Order** — without this, best matches are not sorted to the top.
  3. Try a broader or more descriptive query.
  4. Confirm the backend is running and returning a valid response (check *Plugin Manager → Status*).

---

### 12. macOS Filesystem Permission Warning After Update

- **Symptom:** Lightroom shows a filesystem permissions warning on startup after installing or updating LrGeniusAI.
- **Resolution:** Run Adobe's permission repair script:
  [Adobe: Fix Lightroom user account permissions (macOS)](https://helpx.adobe.com/lightroom-classic/kb/lightroom-basic-troubleshooting-fix-most-issues.html#Troubleshootuseraccountpermissions)
  
  This is a known macOS quirk when a third-party plugin installer writes to certain directories. Running Adobe's script resolves it without any data loss.

---

### 13. AI Edit Produces 500 Internal Server Error

- **Symptom:** AI Edit fails with "HTTP status: 500" from the backend.
- **Resolution:**
  1. Check the backend terminal log for the full error message.
  2. Common cause: the selected model returned a response that doesn't match the expected edit schema. Try a different (more capable) model — `gemini-2.5-flash` or `gpt-5-mini` are reliable choices.
  3. If you are using a local model, the 4B models sometimes produce malformed JSON. Try the 8B variant or switch to a cloud model.
  4. If the error mentions "schema validation", it usually means the model returned prose instead of structured JSON. Custom system prompts that break the format can cause this — reset to the default system prompt and try again.

---

### 14. Database Initialization Failed After Update

- **Symptom:** "Failed to initialize database at the selected path" error when starting the backend or when Lightroom connects.
- **Resolution:**
  1. Ensure the plugin and backend are the **same version** (see [Version Mismatch](#pluginbackend-version-mismatch)).
  2. Check that the backend has write access to the configured data directory.
  3. If the path was changed in a previous version, verify the new path in *Plug-in Manager → Backend Server*.
  4. As a last resort, create a **DB backup** first (*Plug-in Manager → Download DB backup*), then let the backend re-initialize with a fresh database.

---

### 15. Windows: Restart/Update Endpoint Not Working

- **Symptom:** "Check for Updates" or backend restart from within Lightroom fails on Windows.
- **Resolution:** This is a known issue on Windows with the `/restart` endpoint. Restart the backend manually:
  1. Close Lightroom or stop the backend process.
  2. Run `lrgenius-server/lrgenius-server.cmd` again.

---

## Collecting Logs for Bug Reports

If you need to report a bug on GitHub, include the following:

1. **Plugin log:** *Plug-in Manager → Copy logs to desktop* (or *Show logfile*)
2. **Backend log:** The terminal output of `geniusai-server`, or the log files in the backend's working directory
3. **Environment:** OS version, Lightroom Classic version, LrGeniusAI version (plugin + backend)
4. **Steps to reproduce:** What you did before the error occurred

Submit bugs at: [https://github.com/LrGenius/LrGeniusAI/issues](https://github.com/LrGenius/LrGeniusAI/issues)
