# Help: Ollama Setup

> Migrated from `lrgenius.com/help/ollama-setup` and curated for repo docs.  
> Screenshot references were intentionally removed.

## 1. Install Ollama

- Download from: [https://ollama.com/](https://ollama.com/)
- Install for your platform (Windows/macOS/Linux as available)

## 2. Pull at least one vision-capable model

For current model recommendations and hardware sizing guidance, see
[Help: Choosing AI Model](Help-Choosing-AI-Model).

You can browse all available vision models here:

- [https://ollama.com/search?c=vision](https://ollama.com/search?c=vision)
  — each model page shows the required VRAM per quantisation variant.

## 3. Configure plugin/backend

- Set `Ollama Base URL` in plugin settings
- Keep default when Ollama runs locally
- Use explicit host URL when Ollama runs on another machine

## Notes

- Larger models generally improve quality but need more VRAM/RAM.
- First pull can take significant time due to model size.
