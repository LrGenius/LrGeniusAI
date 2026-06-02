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

## 5. Configure plugin/backend

- Point backend/plugin to the LM Studio server endpoint
- Verify model availability from plugin model list
