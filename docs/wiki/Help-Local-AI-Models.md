# Help: Built-In Local AI Models (llama.cpp & MLX)

The backend can run vision models **itself** — no Ollama, no LM Studio, no
other app to install or keep running. You pick a model in the Plug-in Manager,
click *Download*, and analysis happens entirely on your machine.

**There is nothing to choose.** Each platform ships one built-in engine, and the
Plug-in Manager shows that one:

| Platform | Engine | Provider name | GPU acceleration |
|---|---|---|---|
| macOS (Apple silicon) | MLX (helper process) | `mlx` | Metal |
| Windows | llama.cpp (in-process) | `llamacpp` | Vulkan, CPU fallback |

macOS ships MLX because it is Apple's native inference stack and the faster of
the two on Apple silicon. The two engines use **different model files**, so a
model downloaded for one is not usable by the other — worth knowing if you move
a catalog between a Mac and a PC, since each machine needs its own download.

If you are on an Intel Mac, there is no built-in engine; use **Ollama** or **LM
Studio**, or a cloud provider.

---

## 1. Download a model

1. Open `File → Plug-in Manager → LrGeniusAI`.
2. Scroll to **AI model (on this computer)**. It carries one box: **Local AI
   Model — MLX (Apple silicon)** on macOS, **Local AI Model — llama.cpp** on
   Windows.
3. Pick an entry from the dropdown and click **Download**. The list shows the
   approximate download size; the models are several gigabytes, so this takes a
   while on a slow connection.
4. When the download finishes, the **Installed** line lists the model.
5. In *Analyze & Index Photos* (or *AI Edit*), choose the model from the **AI
   Model** dropdown — it appears as `mlx: <model>` or `llamacpp: <model>`.

The curated lists are short on purpose: every entry is an ungated repository
(no Hugging Face token needed) and a **vision** model, since a text-only model
would look selectable and then fail on the first photo.

### llama.cpp (GGUF)

| Model | Download | Comfortable RAM |
|---|---|---|
| Gemma 4 E4B *(recommended)* | ~6.3 GB | 16 GB |
| Gemma 4 12B (QAT, highest quality) | ~7.2 GB | 24 GB |
| Ministral 3 8B (balanced alternative) | ~6.1 GB | 16 GB |
| Qwen3.5 9B (balanced alternative) | ~6.5 GB | 16 GB |
| Qwen2.5-VL 3B / 7B | ~3.4 / 6.5 GB | 8 / 16 GB |
| SmolVLM 500M (testing only) | ~0.7 GB | 4 GB |

A GGUF model is always a **pair** of files: the weights plus an `mmproj` vision
projector, which is what lets the model see the photo. The download fetches
both.

### MLX (Apple silicon)

| Model | Download | Comfortable RAM |
|---|---|---|
| Gemma 4 E4B *(recommended)* | ~5.2 GB | 16 GB |
| Gemma 4 E2B (faster, lower quality) | ~3.6 GB | 8 GB |
| Gemma 4 12B (QAT, highest quality) | ~11.0 GB | 32 GB |
| Ministral 3 8B (balanced alternative) | ~5.6 GB | 16 GB |
| Qwen3-VL 4B (balanced alternative) | ~3.1 GB | 8 GB |

An MLX model is a **directory** (config, safetensors shards, tokenizer), not a
single file — that difference is only visible if you go looking on disk.

---

## 2. Reusing models you already have

Neither engine forces a second copy of a model you already downloaded
elsewhere:

- **llama.cpp** scans its own model directory plus any GGUFs under
  `~/.lmstudio/models`.
- **MLX** scans its own model directory, `~/.lmstudio/models` (LM Studio ships
  an MLX engine on Apple silicon), and the `huggingface-cli` cache at
  `~/.cache/huggingface/hub`.

Anything found there shows up in the **Installed** list and the model dropdown.

---

## 3. Advanced settings (Windows / llama.cpp only)

Under the llama.cpp section:

- **Context size (tokens)** — how much prompt+photo the model can hold at once.
- **Photos in parallel** — how many photos are decoded concurrently.
- **Layers on the GPU** — `0` runs entirely on the CPU; anything that does not
  fit in VRAM stays on the CPU anyway.

Context size and photos-in-parallel trade against each other: the whole group
of photos has to fit the context window alongside the shared prompt prefix, and
the backend reduces the parallel count with a warning rather than overrunning
it. Leave the fields empty for sensible defaults. Changing any of them reloads
the model on the next request.

**MLX has no equivalent knobs**, and that is a deliberate limitation rather than
an oversight: the MLX decoder allocates a fresh cache per request, so there is
no shared prompt prefix to size a context window around, and it processes one
photo at a time.

---

## 4. Things worth knowing

- **Local is slower than cloud.** Expect seconds to tens of seconds per photo
  depending on model size and hardware. Start with a small batch to get a feel
  for the throughput before queueing thousands of photos.
- **First request loads the model**, which can take a while for a multi-gigabyte
  file. Subsequent photos are much faster.
- **Avoid keyword aliases and bilingual keywords with local models.** Both turn
  every keyword into an object rather than a plain string, and small local
  models handle that structure badly — measured on Gemma 4 E4B, the same photo
  produced 19 keywords with plain strings and *zero* with the object form. The
  Analyze & Index dialog warns when you combine them.
- **Quality still trails frontier cloud models** on tricky scenes. If a batch
  comes back weak, compare against `gemini-2.5-flash` on the same 10–20 photos
  before tuning anything else.

---

## 5. Troubleshooting

**"MLX runs only on Apple silicon Macs"**
Expected on an Intel Mac. There is no built-in engine there — use Ollama, LM
Studio, or a cloud provider.

**MLX says the helper is missing**
The MLX engine needs the `lrgenius-mlx` helper next to the server binary. Ship
it by reinstalling the backend with the official macOS `.pkg`; if you build
from source, see [Server README](Dev-Server-README) for the `xcodebuild` step.
Since MLX is the only built-in engine on macOS, this is worth fixing rather
than working around.

**"This backend build has no local-model support"** (Windows)
The backend was compiled without the `llamacpp` feature. Official Windows
release builds have it; a self-built binary needs
`cargo build --release -p lrg-server --features llamacpp`. On macOS this
message means the backend is a Windows-style build — the macOS release ships
MLX instead and never reports llama.cpp support.

**The model dropdown shows no local models**
Nothing is installed yet, or the download did not finish. Check the
**Installed** line in the Plug-in Manager, and re-run the download if needed —
an interrupted download is discarded rather than offered as a broken model.

**Analysis is very slow or the machine swaps**
The model is too large for your RAM. Move down a size (Gemma 4 E2B or
Qwen2.5-VL 3B), or reduce **Photos in parallel** to 1.

---

## See also

- [Help: Choosing AI Model](Help-Choosing-AI-Model) — cloud vs local comparison
- [Help: Ollama Setup](Help-Ollama-Setup) — external local server alternative
- [Help: LM Studio Setup](Help-LM-Studio-Setup) — external local server alternative
- [Troubleshooting](Troubleshooting)
