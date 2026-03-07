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

Examples mentioned in help content:

- `qwen/qwen3-vl-4b`
- `qwen/qwen3-vl-8b`
- `google/gemma3-4b`
- `google/gemma3-12b`

## 4. Performance guidance

- Prefer the largest model that still fits your hardware
- On Apple Silicon, use MLX-optimized variants when available

## 5. Configure plugin/backend

- Point backend/plugin to the LM Studio server endpoint
- Verify model availability from plugin model list
