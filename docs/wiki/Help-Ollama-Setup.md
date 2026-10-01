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

## 3. Nothing to configure in the plugin

LrGeniusAI finds Ollama on this computer by itself, at its default address
(`http://localhost:11434`). Once Ollama is running, its models appear in every
task's **AI Model** list as `Ollama · <model>`.

**Ollama on another computer** (or on a changed port): enter its address, e.g.
`http://nas.local:11434`, as the **Other AI server** — see
[Other AI Server](Help-Other-AI-Server). That connection goes through Ollama's
OpenAI-compatible API, which cannot set the context size per request; raise it
on that computer if long prompts get cut off
(`OLLAMA_CONTEXT_LENGTH=8192 ollama serve`).

## Notes

- Larger models generally improve quality but need more VRAM/RAM.
- First pull can take significant time due to model size.
