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

Configure in *Plug-in Manager → Optional AI providers → Google Gemini key*. Models exposed today:

- `gemini-2.5-flash-lite` — cheapest and fastest; good for bulk keywording.
- `gemini-2.5-flash` — balanced default for analyze-and-index runs.
- `gemini-2.5-pro` — highest 2.5-tier quality; use for tricky scenes or when
  description quality matters more than throughput.
- `gemini-3-flash-preview`, `gemini-3.1-flash-lite-preview`,
  `gemini-3.1-pro-preview` — latest preview tier. Expect higher quality and
  better instruction following, but preview pricing/quotas can change.

How much a Gemini model thinks follows **Analysis depth** in the Analyze &
Index dialog: every Gemini 3 model (`gemini-3-*`, `gemini-3.x-*`) gets the
matching thinking level (*Fast* = `low`, *Balanced* = `medium`, *Thorough* =
`high`), and `gemini-2.5-*` a thinking budget (the smallest the model takes at
*Fast*, 2048 tokens at *Balanced*, the model's own choice at *Thorough*).
Thinking tokens count against the token limit, so the backend adds room for
them on top of **Max Tokens** (4096 / 8192 / 16384 tokens by depth) whenever
the model thinks; they are billed as output and included in the token totals
the server logs. A Gemini 3 model that does not offer *Balanced* is asked again
at *Thorough*, and the run says so.

Gemini 3 is sent no temperature: Google asks for its default there, and lower
values can make it repeat itself. The **Temperature** slider still applies to
`gemini-2.5-*`.

If a Gemini model reports that it stopped because the token limit was reached,
raise Max Tokens to 4096 or higher. If the same photo keeps failing at a higher
limit, the model is looping rather than running out of room; switch to a
different Gemini model instead of raising the limit further.

### OpenAI / ChatGPT

Configure in *Plug-in Manager → Optional AI providers → OpenAI key*. Models exposed:

- `gpt-4.1` — proven vision quality; the safe baseline.
- `gpt-5-nano`, `gpt-5-mini`, `gpt-5` — current GPT-5 tier; pick `nano`/`mini`
  for batch jobs and `gpt-5` for higher-fidelity descriptions.
- `gpt-5.4-nano`, `gpt-5.4-mini`, `gpt-5.4`, `gpt-5.4-pro` — newest GPT-5.4
  tier; `gpt-5.4-pro` is the highest-quality option but the most expensive.

Note: GPT-5 and GPT-5.4 models ignore the **Temperature** slider. Their
`reasoning_effort` follows **Analysis depth** instead (*Fast* = `low`,
*Balanced* = `medium`, *Thorough* = `high`). `gpt-4.1` does not reason: it
uses the temperature and ignores Analysis depth.

GPT-5 models spend part of the token limit on internal reasoning before
writing a single word of the answer, so the backend adds room for it on top of
**Max Tokens** (4096 / 8192 / 16384 tokens by depth). If a GPT-5 model still
reports that it stopped because the token limit was reached, lower Analysis
depth or raise Max Tokens.

### Anthropic Claude

Configure in *Plug-in Manager → Optional AI providers → Anthropic key* (create
one at [platform.claude.com](https://platform.claude.com/settings/keys)). The
list is not curated: it shows every Claude model your key can use that reads
photos and supports structured outputs, as Anthropic's own model list reports
them, so new Claude models appear without a plugin update. As a starting point:

- `claude-haiku-4-5` — fastest and cheapest; good for bulk keywording.
- `claude-sonnet-5-5` — balanced default for analyze-and-index runs.
- `claude-opus-5-5` — highest description quality of the three.

This uses Anthropic's own API, not its OpenAI-compatible endpoint. Entering
`https://api.anthropic.com/v1` as the *Other AI server* does not work: that
endpoint cannot list models (it answers *"anthropic-version: header is
required"*) and ignores the answer format. Use the Anthropic key field instead.

What the backend does for you:

- **Answer format.** Every answer is held to the requested fields by
  Anthropic's structured outputs, so keywords, caption and title arrive in the
  expected shape.
- **Thinking.** Newer Claude models think before answering, and some cannot
  turn it off. The backend sends **Analysis depth** as the model's `effort`
  (*Fast* = `low`, *Balanced* = `medium`, *Thorough* = `high`) where the model
  lists that level, and otherwise leaves the model at its own default. It adds
  room for thinking on top of **Max Tokens** (4096 / 8192 / 16384 tokens by
  depth), so a photo is not cut off by reasoning you never see. Thinking
  tokens are billed as output.
- **Temperature.** Current Claude models reject a temperature setting, so the
  plugin's temperature slider does not apply to them.
- **Prompt caching.** The part of the prompt that is the same for every photo of
  a run (instructions, keyword vocabulary, taxonomy) is cached on Anthropic's
  side, so from the second photo on it costs a fraction of the normal input
  price. This only kicks in once that part is long enough (roughly 500–4,000
  tokens, depending on the model).
- **Safety filter.** If a model declines a photo, the task reports it with
  Anthropic's reason. For the newest models (Opus 5.5, Opus 5, Sonnet 5.5,
  Fable 5/5.1) the request lets Anthropic retry a declined photo on the model
  it recommends; the photo is then indexed and the task tells you which model
  answered.

If Claude reports that it stopped because the token limit was reached, raise
Max Tokens to 4096 or higher. If the same photo keeps failing, the model is
repeating itself; pick a different Claude model.

## Local models

Local providers run on your own machine, so privacy is the strongest argument
for using them. Quality of small open-weights vision models has improved
significantly, but cloud frontier models still lead on tricky scenes.

There are two kinds of local option: the engines **built into the backend**
(nothing else to install), and **external servers** you run yourself (Ollama,
LM Studio, or any OpenAI-compatible server as the *Other AI server*).

### Built-in: llama.cpp and MLX (no external app)

The backend runs vision models itself. Open *Plug-in Manager → LrGeniusAI*,
find the **Local AI Model** sections, pick a model, and click **Download** —
it then appears in the model dropdown as `On this Mac · <model>` or
`On this PC · <model>`.

- **`llamacpp`** — llama.cpp compiled into the backend, using GGUF models.
  Shipped in the Windows release (Vulkan, any GPU vendor). Recommended entries: **Gemma 4 E4B** as the default, **Gemma 4 12B
  (QAT)** if you have 24 GB of RAM, **Ministral 3 8B** or **Qwen3.5 9B** as
  alternatives, **Qwen2.5-VL 3B** on modest hardware.
- **`mlx`** — Apple's MLX stack via a small Metal helper process, **Apple
  silicon only**. Recommended entries: **Gemma 4 E4B** as the default,
  **Gemma 4 E2B** for speed, **Ministral 3 8B** or **Qwen3-VL 4B** as
  alternatives.

The macOS release ships MLX only. A backend built from source with the
`llamacpp` feature offers both on a Mac, which is worth a side-by-side run on
the same 10–20 photos: MLX is Apple's native inference stack, while llama.cpp
reuses the shared prompt prefix across the photos in a batch (MLX re-processes
it per photo), which matters more the larger the batch.

Either engine can also download a model that is not in the list — pick
**Other model from Hugging Face…**; it is checked before anything is
downloaded. See [Local AI Models](Help-Local-AI-Models).

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
Nothing to configure in the plugin: Ollama on this computer is found
automatically. See [Ollama Setup](Help-Ollama-Setup).

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
significantly faster than the GGUF builds. Like Ollama, LM Studio on this
computer is found automatically. See [LM Studio Setup](Help-LM-Studio-Setup).

### Other AI server (OpenRouter, llama.cpp server, LiteLLM, vLLM)

Any server that speaks the OpenAI chat API can be entered as the **Other AI
server** in *Plug-in Manager → Optional AI providers*, with an API key if it
needs one. That covers [OpenRouter](https://openrouter.ai) (hundreds of cloud
models behind one key, some free), llama.cpp's `llama-server`, LiteLLM, vLLM,
and Ollama or LM Studio running on another computer. Its models appear under
the server's name, e.g. `OpenRouter · google/gemini-2.5-flash`. See
[Other AI Server](Help-Other-AI-Server).

## Quick recommendations

| Workflow                              | Suggested first try                              |
| ------------------------------------- | ------------------------------------------------ |
| Cheap bulk keywording (cloud)         | `gemini-2.5-flash-lite`, `gpt-5-nano` or `claude-haiku-4-5` |
| Balanced default (cloud)              | `gemini-2.5-flash`, `gpt-5-mini` or `claude-sonnet-5-5`     |
| Best description quality (cloud)      | `gemini-2.5-pro`, `gpt-5.4`, `gpt-5.4-pro` or `claude-opus-5-5` |
| Privacy-first, simplest setup         | Built-in `llamacpp` with Gemma 4 E4B             |
| Apple Silicon, local                  | Built-in `mlx` with Gemma 4 E4B (E2B if 8–16 GB) |
| Windows with any discrete GPU, local  | Built-in `llamacpp` (Vulkan) with Gemma 4 E4B    |
| Modest hardware / 8 GB RAM            | `mlx` Gemma 4 E2B or `llamacpp` Qwen2.5-VL 3B    |
| Already running Ollama / LM Studio    | Ollama `qwen3-vl:8b` or LM Studio `qwen3-vl-8b`  |
| Many cloud models, one key            | Other AI server: OpenRouter                      |
| A GPU server on your network          | Other AI server: `llama-server`, vLLM or LiteLLM |

## Practical recommendation

The dropdown in *Analyze & Index* always reflects what the
backend currently advertises — newer models that ship with future backend
updates will appear automatically. If a model you expect is missing, check
that the corresponding API key or server is configured and reachable from the
backend. The *Plug-in Manager → Status* section reports whether any provider is
available, and the line under **Other AI server** whether that server answers.
A saved model that is not offered right now stays selected, marked
*(not available now)* — the task then tells you what to start or fix instead of
quietly switching to another provider.

When evaluating, run the same batch of 10–20 representative photos through
two candidates and compare:

- keyword coverage and accuracy
- description quality and language correctness
- runtime per image and end-to-end batch time
- system load (local) or token cost (cloud)

---

## Throughput on cloud providers

Cloud runs are dominated by network round trips, not by the model. The backend
overlaps up to four requests at a time for OpenAI, Gemini and Anthropic, which hides most
of that latency without looking like a burst to their rate limiters.

Ollama, LM Studio and the Other AI server deliberately stay at one request at
a time: the server on the other end is usually a single model that serialises
the work anyway, and piling requests on it only competes with Lightroom for the
same machine.
