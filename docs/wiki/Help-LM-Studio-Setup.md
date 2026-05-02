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

Recommended starting points:

- `qwen/qwen3-vl-4b` — fast baseline.
- `qwen/qwen3-vl-8b` — better description quality at moderate cost.
- `google/gemma3-4b` — solid general-purpose default.
- `google/gemma3-12b` — higher quality if your hardware can host it.

## 4. Performance guidance

- Prefer the largest model that still fits comfortably in VRAM/unified memory.
- On Apple Silicon, prefer the **MLX** variant of the same model — it runs
  noticeably faster than the GGUF build for vision workloads.
- For batch indexing on a laptop, a 4B model usually beats waiting on a
  thrashing 12B model.

See [Help: Choosing AI Model](Help-Choosing-AI-Model) for a side-by-side
comparison with cloud providers.

## 5. Configure plugin/backend

- Point backend/plugin to the LM Studio server endpoint
- Verify model availability from plugin model list
