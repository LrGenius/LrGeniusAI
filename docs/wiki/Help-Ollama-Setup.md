# Help: Ollama Setup

> Migrated from `lrgenius.com/help/ollama-setup` and curated for repo docs.  
> Screenshot references were intentionally removed.

## 1. Install Ollama

- Download from: [https://ollama.com/](https://ollama.com/)
- Install for your platform (Windows/macOS/Linux as available)

## 2. Pull at least one vision-capable model

Suggested starting points (smaller variant first if unsure about VRAM):

```bash
ollama pull qwen3-vl:4b-instruct-q4_K_M    # fast, ~6 GB VRAM
ollama pull qwen3-vl:8b-instruct-q4_K_M    # better quality, ~10 GB VRAM
ollama pull gemma3:4b-it-q4_K_M            # solid general default
ollama pull gemma3:12b-it-q4_K_M           # higher quality if VRAM allows
ollama pull llava                           # legacy fallback
```

You can browse all vision models here:

- [https://ollama.com/search?c=vision](https://ollama.com/search?c=vision)

See [Help: Choosing AI Model](Help-Choosing-AI-Model) for guidance on
picking between local and cloud options.

## 3. Configure plugin/backend

- Set `Ollama Base URL` in plugin settings
- Keep default when Ollama runs locally
- Use explicit host URL when Ollama runs on another machine

## Notes

- Larger models generally improve quality but need more VRAM/RAM.
- First pull can take significant time due to model size.
