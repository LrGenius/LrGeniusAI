# Help: Choosing an AI Model

> The exact model lists exposed by the plugin come from the backend at runtime.
> The names below reflect the curated lists shipped with the current backend
> (`server/src/providers/`). Pricing and availability change over time — verify
> with each provider before relying on production cost estimates.

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

### Vertex AI (embeddings only)

Vertex AI is used for the `multimodalembedding@001` model that powers the
`image_embeddings_vertex` semantic-search collection. It is **not** an
alternative LLM for keywords/descriptions — pair it with a Gemini, ChatGPT,
or local provider for metadata generation. See
[Google Vertex AI Login](Google-Vertex-AI-Login).

## Local models

Local providers run on your own machine, so privacy is the strongest argument
for using them. Quality of small open-weights vision models has improved
significantly, but cloud frontier models still lead on tricky scenes.

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

Download from [lmstudio.ai](https://lmstudio.ai/download), enable server mode,
and download one or more vision models from inside the app. Recommended:

- `qwen/qwen3-vl-4b` — fast baseline.
- `qwen/qwen3-vl-8b` — better description quality.
- `google/gemma3-4b` / `google/gemma3-12b` — strong general-purpose options.

On Apple Silicon prefer the **MLX** variants of the same model — they run
significantly faster than the GGUF builds. See [LM Studio Setup](Help-LM-Studio-Setup).

## Quick recommendations

| Workflow                              | Suggested first try                              |
| ------------------------------------- | ------------------------------------------------ |
| Cheap bulk keywording (cloud)         | `gemini-2.5-flash-lite` or `gpt-5-nano`          |
| Balanced default (cloud)              | `gemini-2.5-flash` or `gpt-5-mini`               |
| Best description quality (cloud)      | `gemini-2.5-pro`, `gpt-5.4`, or `gpt-5.4-pro`    |
| Privacy-first / no API billing        | Ollama `qwen3-vl:8b` or LM Studio `qwen3-vl-8b`  |
| Apple Silicon, local                  | LM Studio MLX build of `qwen3-vl` or `gemma3`    |

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
