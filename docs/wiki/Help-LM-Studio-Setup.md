# Help: LM Studio Setup

> Migrated from `lrgenius.com/help/lmstudio-setup` and curated for repo docs.  
> Screenshot references were intentionally removed.

## 1. Install LM Studio

- Download from: [https://lmstudio.ai/download](https://lmstudio.ai/download)

## 2. Configure LM Studio for LrGeniusAI

- Enable server mode in LM Studio
- Ensure server status is running
- Enable on-demand model loading if preferred

## 3. Download vision model(s)

For current model recommendations and hardware sizing guidance, see
[Help: Choosing AI Model](Help-Choosing-AI-Model).

LM Studio's built-in model browser shows estimated RAM/VRAM usage and flags
models that exceed your system memory — use it to find models that fit your
hardware.

## 4. Performance guidance

- Prefer the largest model that still fits comfortably in VRAM/unified memory.
- On Apple Silicon, prefer the **MLX** variant of the same model — it runs
  noticeably faster than the GGUF build for vision workloads.
- For batch indexing on a laptop, a smaller/faster model usually beats waiting
  on a thrashing large one.

## 5. Nothing to configure in the plugin

LrGeniusAI finds LM Studio on this computer by itself, at its default address
(`localhost:1234`). Once the server is running, its models appear in every
task's **AI Model** list as `LM Studio · <model>`. With LM Studio's
just-in-time model loading (on by default), every downloaded model is listed,
not only the loaded ones.

- **LM Studio on another computer** (or on a changed port): enter its address,
  e.g. `http://192.168.1.20:1234`, as the **Other AI server** — see
  [Other AI Server](Help-Other-AI-Server).
- **LM Studio's "Require API token" setting** is not supported for the
  automatic connection. Either leave it off, or enter `localhost:1234` as the
  Other AI server with the token as its API key.
