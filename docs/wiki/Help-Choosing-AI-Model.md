# Help: Choosing an AI Model

> The exact model lists exposed by the plugin come from the backend at runtime.
> The names below reflect the curated lists shipped with the current backend
> (`server-rs/crates/lrg-providers/` for cloud providers,
> `server-rs/crates/lrg-api/src/{llm_models,mlx_models}.rs` for the built-in
> local ones). Pricing and availability change over time — verify with each
> provider before relying on production cost estimates.

## Decision factors

Choose based on:

- privacy requirements (cloud vs. local)
- quality expectations (description detail, keyword accuracy, edit recipe sanity)
- runtime per image and batch throughput
- per-image cost (cloud) or hardware cost (local)
- available local hardware (VRAM/RAM, Apple Silicon vs. discrete GPU)

## Cloud models

### Google Gemini

Configure in *Plug-in Manager → API Keys → Gemini API key*. Models exposed today:

- `gemini-2.5-flash-lite` — cheapest and fastest; good for bulk keywording.
- `gemini-2.5-flash` — balanced default for analyze-and-index runs.
- `gemini-2.5-pro` — highest 2.5-tier quality; use for tricky scenes or when
  description quality matters more than throughput.
- `gemini-3-flash-preview`, `gemini-3.1-flash-lite-preview`,
  `gemini-3.1-pro-preview` — latest preview tier. Expect higher quality and
  better instruction following, but preview pricing/quotas can change.

The backend automatically tunes a thinking budget for `gemini-2.5-*` and
`gemini-3-pro-preview`, so you don't need to configure that yourself.

### OpenAI / ChatGPT

Configure in *Plug-in Manager → API Keys → OpenAI API key*. Models exposed:

- `gpt-4.1` — proven vision quality; the safe baseline.
- `gpt-5-nano`, `gpt-5-mini`, `gpt-5` — current GPT-5 tier; pick `nano`/`mini`
  for batch jobs and `gpt-5` for higher-fidelity descriptions.
- `gpt-5.4-nano`, `gpt-5.4-mini`, `gpt-5.4`, `gpt-5.4-pro` — newest GPT-5.4
  tier; `gpt-5.4-pro` is the highest-quality option but the most expensive.

Note: GPT-5 and GPT-5.4 models ignore the `temperature` slider and use a
fixed reasoning effort — small differences in plugin temperature settings
will not affect output for these models.

### ~~Vertex AI (embeddings only)~~ — REMOVED

> **⚠️ Removed in August 2026.** The plugin no longer offers Vertex AI anywhere: no
> project/location settings, no Vertex embeddings during *Analyze & Index*, no
> *Semantic (Vertex AI)* search option. Semantic search now runs on the built-in SigLIP2
> embeddings only. Existing Vertex embeddings stay in the database but are not updated or
> queried. Description below kept for reference.

Vertex AI is used for the `multimodalembedding@001` model that powers the
`image_embeddings_vertex` semantic-search collection. It is **not** an
alternative LLM for keywords/descriptions — pair it with a Gemini, ChatGPT,
or local provider for metadata generation. See
[Google Vertex AI Login](Google-Vertex-AI-Login).

## Local models

Local providers run on your own machine, so privacy is the strongest argument
for using them. Quality of small open-weights vision models has improved
significantly, but cloud frontier models still lead on tricky scenes.

There are two kinds of local option: the engines **built into the backend**
(nothing else to install), and **external servers** you run yourself (Ollama,
LM Studio).

### Built-in: llama.cpp and MLX (no external app)

The backend runs vision models itself. Open *Plug-in Manager → LrGeniusAI*,
find the **Local AI Model** sections, pick a model, and click **Download** —
it then appears in the model dropdown as `llamacpp: <model>` or `mlx: <model>`.

- **`llamacpp`** — llama.cpp compiled into the backend, using GGUF models.
  Available on macOS (Metal), Windows (Vulkan, any GPU vendor), and Linux
  (CPU). Recommended entries: **Gemma 4 E4B** as the default, **Gemma 4 12B
  (QAT)** if you have 24 GB of RAM, **Ministral 3 8B** or **Qwen3.5 9B** as
  alternatives, **Qwen2.5-VL 3B** on modest hardware.
- **`mlx`** — Apple's MLX stack via a small Metal helper process, **Apple
  silicon only**. Recommended entries: **Gemma 4 E4B** as the default,
  **Gemma 4 E2B** for speed, **Ministral 3 8B** or **Qwen3-VL 4B** as
  alternatives.

On an Apple silicon Mac both are available and worth a side-by-side run on the
same 10–20 photos: MLX is Apple's native inference stack, while llama.cpp reuses
the shared prompt prefix across the photos in a batch (MLX re-processes it per
photo), which matters more the larger the batch.

Both reuse models you already have: llama.cpp picks up GGUFs under
`~/.lmstudio/models`, and MLX picks up LM Studio's MLX models and the
`huggingface-cli` cache. Full guide: [Local AI Models](Help-Local-AI-Models).

One caveat that applies to both: **do not enable keyword aliases or bilingual
keywords with a local model.** They turn every keyword into a structured object,
which small models handle badly — the Analyze & Index dialog warns about it.

### Ollama

Install and start Ollama from [ollama.com](https://ollama.com/), then pull at
least one vision-capable model. Recommended starting points:

```bash
ollama pull qwen3-vl:4b-instruct-q4_K_M     # fast, ~6 GB VRAM
ollama pull qwen3-vl:8b-instruct-q4_K_M     # better quality, ~10 GB VRAM
ollama pull gemma3:4b-it-q4_K_M             # good general default
ollama pull gemma3:12b-it-q4_K_M            # higher quality if you have VRAM
ollama pull llava                            # legacy fallback
```

Browse all vision models: [ollama.com/search?c=vision](https://ollama.com/search?c=vision).
See [Ollama Setup](Help-Ollama-Setup).

### LM Studio

Worth running as an external server mainly if you already use it for other
things, or want its model browser and per-model tuning; otherwise the built-in
engines above cover the same ground with one fewer app running.

Download from [lmstudio.ai](https://lmstudio.ai/download), enable server mode,
and download one or more vision models from inside the app. Recommended:

- `qwen/qwen3-vl-4b` — fast baseline.
- `qwen/qwen3-vl-8b` — better description quality.`
- `gemma-4-e4b` / `google/gemma3-12b` — strong general-purpose options.

On Apple Silicon prefer the **MLX** variants of the same model — they run
significantly faster than the GGUF builds. See [LM Studio Setup](Help-LM-Studio-Setup).

## Quick recommendations

| Workflow                              | Suggested first try                              |
| ------------------------------------- | ------------------------------------------------ |
| Cheap bulk keywording (cloud)         | `gemini-2.5-flash-lite` or `gpt-5-nano`          |
| Balanced default (cloud)              | `gemini-2.5-flash` or `gpt-5-mini`               |
| Best description quality (cloud)      | `gemini-2.5-pro`, `gpt-5.4`, or `gpt-5.4-pro`    |
| Privacy-first, simplest setup         | Built-in `llamacpp` with Gemma 4 E4B             |
| Apple Silicon, local                  | Built-in `mlx` with Gemma 4 E4B (E2B if 8–16 GB) |
| Windows with any discrete GPU, local  | Built-in `llamacpp` (Vulkan) with Gemma 4 E4B    |
| Modest hardware / 8 GB RAM            | `mlx` Gemma 4 E2B or `llamacpp` Qwen2.5-VL 3B    |
| Already running Ollama / LM Studio    | Ollama `qwen3-vl:8b` or LM Studio `qwen3-vl-8b`  |

## Practical recommendation

The dropdown in *Analyze & Index* and *AI Edit* always reflects what the
backend currently advertises — newer models that ship with future backend
updates will appear automatically. If a model you expect is missing, check
that the corresponding API key or local server is configured and reachable
from the backend (the *Plugin Manager → Status* section reports availability
per provider).

When evaluating, run the same batch of 10–20 representative photos through
two candidates and compare:

- keyword coverage and accuracy
- description quality and language correctness
- runtime per image and end-to-end batch time
- system load (local) or token cost (cloud)
