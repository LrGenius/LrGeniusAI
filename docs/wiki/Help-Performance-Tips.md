# Help: Performance Tips

LrGeniusAI's speed depends almost entirely on **how much LLM work a feature
does**. Some features call a vision LLM once per photo; others never call an
LLM at all and are limited only by local compute. This page explains which is
which, and what you can tune to make the slow ones faster.

---

## 1. What actually calls an LLM (and what doesn't)

| Feature | Calls an LLM? | What drives the time |
|---|---|---|
| **Analyze & Index — AI metadata** (keywords/title/caption/alt text) | **Yes, once per photo** | Model choice, export size, prompt options |
| **AI Edit Photos** | No — matches your saved edits locally | Export size, review dialog |
| **Deduplicate Keyword Synonyms** | Optional, once per cluster (not per photo) | Number of keyword clusters, not catalog size |
| Analyze & Index — **search embeddings** (SigLIP2) | No — local ONNX model, always | Photo count only; same speed regardless of LLM choice |
| Analyze & Index — **species identification** (BioCLIP 2) | No — local ONNX model, always | Photo count, and how many photos clear the organism pre-filter |
| **Cull Photos** | No | Runs on metrics already computed during indexing |
| **People / Find Similar Faces** | No | Vector lookup against already-indexed face embeddings |
| **Find Similar Images** | No | Vector lookup (CLIP) or perceptual hash (phash) |
| **Advanced Search** | No | Vector lookup against existing embeddings |

The practical upshot: **Analyze & Index is the only per-photo feature
where your LLM choice matters for speed.** Everything else is bounded by
local compute on the backend machine and is unaffected by whether you
picked a cheap cloud model or a big local one — but those features all
*depend on* having run Analyze & Index first (embeddings for search/cull/find
similar, face detection for people/faces).

### The local pass that is not free: species identification

"No LLM" does not mean "no cost". BioCLIP 2 is a ViT-L/14, several times the
work of the SigLIP2 embedding, and on CPU it takes a few hundred milliseconds
per photo on top of everything else in the run. Over a five-figure catalog that
is the difference between an overnight job and a much longer one.

Which is why **Only where an animal or plant is detected** is on by default.
The gate costs nothing — it scores the SigLIP2 embedding the same run already
computed — and in a general library of portraits, architecture and landscapes
it rejects the large majority of photos before the expensive model ever runs.
Leave it on unless you know the selection is all wildlife, in which case
turning it off removes a small chance of a false negative.

Two consequences worth knowing:

- **The gate needs an embedding to score.** A stored one from an earlier run
  counts, so running species on its own over an already-indexed selection gates
  normally. But a photo that has no embedding at all is passed through and
  classified anyway — the safe direction, and the expensive one. If you are
  starting from scratch, tick **Enable smart photo search** in the same run
  rather than running species first.
- **Photos the gate rejects still count as done.** They get
  `Species rank = none` and are not re-examined on the next run, so a second
  pass over the same selection is cheap.

---

## 2. The single biggest lever: which model you pick

Cloud frontier models (`gemini-2.5-pro`, `gpt-5.4-pro`) and large local models
are both slow — for different reasons (cloud: request latency and
provider-side reasoning; local: raw compute on your machine). If you're
processing a large catalog:

- **Bulk keywording** → cheapest/fastest cloud tier: `gemini-2.5-flash-lite`,
  `gpt-5-nano`. Seconds per photo, low per-image cost.
- **Balanced default** → `gemini-2.5-flash`, `gpt-5-mini`.
- **Best quality, small batches only** → `gemini-2.5-pro`, `gpt-5.4-pro`. Use
  for a reshoot's hero images, not 5,000 photos.
- **Local (privacy-first)** → expect **seconds to tens of seconds per photo**,
  much slower than cloud. Start with a small batch (10–20 photos) to gauge
  throughput on your hardware before queueing thousands. Smaller models
  (Gemma 4 E2B, Qwen2.5-VL 3B) trade quality for speed; the first photo in a
  session is always the slowest because the model has to load into memory.

Full comparison: [Help: Choosing AI Model](Help-Choosing-AI-Model) ·
[Help: Local AI Models](Help-Local-AI-Models).

---

## 3. Export size and JPEG quality

Every photo sent to a vision LLM is first exported to a temporary JPEG
(unless you use the originals fast path below). Two settings in *Plug-in
Manager → LrGeniusAI* control that export:

- **Export size in pixels (long edge)** — default **1024px**. Options: 512,
  1024, 2048, 3072, 4096.
- **Export JPEG quality in percent** — default **60%**.

Larger/higher-quality exports mean bigger uploads (cloud) and more pixels for
a local model to decode (local) — both cost time, and for cloud providers
also cost more tokens/money. **1024px / 60%** is a reasonable default for
keywording and descriptions. Drop to **512px** for a fast first-pass bulk run
where you mainly want keywords, not fine detail; go up to **2048px+** only if
you need the model to read small text or catch fine detail, and only on
smaller batches.

---

## 4. "Submit original files for fastest indexing" (experimental)

In *Plug-in Manager*, this option skips the export step entirely — the
backend reads the RAW/JPEG/TIFF/PNG file directly (local backend) or uploads
it raw (remote provider), instead of Lightroom rendering a JPEG first. It's
the fastest indexing path, with tradeoffs:

- Uses the file's embedded camera preview, so **crops and develop edits are
  not reflected** in what the AI sees.
- **Location keywords from Lightroom's address lookup are unavailable.**
- **HEIC is not among the formats the backend can decode.** Those photos fall
  back to the export path automatically, so a mixed library still indexes —
  just without the speedup for the HEIC part of it.
- Results may differ slightly from the classic export-based path.

If you use it, re-index later the classic way if you want search results
consistent with your final edited/cropped images.

---

## 5. Batching several photos into one request

Batching only happens when **both** are true:

1. **Submit original files** (above) is enabled, and
2. the backend can read the files locally (i.e. it's running on the same
   machine as Lightroom, or has access to the same file paths).

Given those, what gets batched depends on whether the run generates AI
metadata:

- **Runs without AI metadata batch on any provider.** A pass that only
  computes embeddings, faces or species — and the preparation step of *Cull
  Photos*, which computes pHash and image metrics — never talks to a language
  model at all, so there is no per-request billing or context window to
  respect. The backend reads, decodes and measures a group of photos across
  several CPU cores instead of one at a time, which is where most of the time
  in such a run goes. Uncheck **AI metadata** in the indexing dialog to get
  this.
- **Runs with AI metadata batch only on the built-in `llamacpp` engine**,
  which shares one pinned prompt prefix across the group and decodes the
  photos in parallel. Every cloud provider stays at **one photo per request**,
  because remote APIs are billed and rate-limited per call and gain nothing
  from grouping.

Two advanced llama.cpp settings trade against each other:

- **Context size (tokens)** — how much prompt+photo the model can hold at once.
- **Photos in parallel** — how many photos decode concurrently.

The whole group has to fit in the context window alongside the shared
prefix; the backend automatically reduces the parallel count (with a
warning) rather than overrunning it. Leave both empty for sensible defaults
unless you've hit that warning.

**MLX (Apple silicon) never batches the language model** — it allocates a
fresh cache per request and always processes one photo at a time, regardless
of these settings. A run without AI metadata is unaffected by this, since it
never reaches the language model. This is a deliberate limitation of the MLX decoder, not a bug. On
Apple silicon, run the same 10–20 photos through both `llamacpp` and `mlx`
to see which wins for your batch size before committing to one for a large run.

---

## 6. The plugin always processes one photo at a time

Regardless of provider, the plugin runs a **single worker** — it never has
two requests to the backend in flight at once. This is intentional: earlier
attempts at multi-threaded indexing caused crashes on Windows, so it's
hardcoded off on every platform, and batching (§5) does not change it. The
parallelism in a batched run is entirely on the backend's side, inside one
request: it is free to use several cores, while the plugin still waits for a
single answer.

On macOS, when you're *not* using "submit originals," the plugin pipelines
export and upload — the next photo's JPEG export overlaps with the previous
photo's backend round-trip, so you're not paying export time and network
time back-to-back. This pipelining isn't available on Windows, so on Windows
the "submit originals" fast path (§4) or a smaller export size (§3) buys you
more.

---

## 7. Other things that add prompt size (and a little time/cost)

These are secondary compared to §2–4, but worth knowing:

- **Keyword aliases / Bilingual keywords** turn every keyword into a
  structured object instead of a plain string, which costs more output
  tokens on every model — and small local models handle the structure badly
  enough that it's not worth using them together at all (measured: the same
  photo produced 19 keywords as plain strings and *zero* as objects on Gemma
  4 E4B). See [Help: Local AI Models](Help-Local-AI-Models).
- **"Use keyword structure from Lightroom catalog"** feeds your existing
  keyword hierarchy into the prompt as category context — fine for a modest
  taxonomy, but a large/complex hierarchy inflates the prompt on every call.
  Use carefully on catalogs with deep keyword trees.
- **Extra context toggles** (folder names, capture date, existing keywords,
  GPS) add a small amount to every prompt. Harmless individually; skip the
  ones you don't need if you're optimizing a very large bulk run.
- **Face detection during indexing** is local compute (YuNet + FaceNet), not
  an LLM call, so it doesn't affect cloud cost — but it does add per-photo
  processing time on the backend machine. Worth it if you'll use
  People/Faces or want face-aware Cull scoring; skip it for a pure
  keywording pass on a non-portrait shoot.

---

## 8. Practical workflow for a large catalog

1. **Test on 10–20 representative photos first**, comparing 1–2 model
   candidates for quality, runtime per image, and (for local) system load —
   before committing to a model for the whole catalog. See the comparison
   checklist in [Help: Choosing AI Model](Help-Choosing-AI-Model#practical-recommendation).
2. **Scope tightly.** Use `New or unprocessed photos` rather than
   `Regenerate all data` unless you actually need to overwrite existing
   results — regenerating re-runs the LLM call for photos that already have
   metadata.
3. **Pick export size/quality for the job**: 512–1024px/60% for a fast bulk
   keywording pass; go bigger only for smaller, quality-critical batches.
4. **Cloud for bulk, local for privacy-sensitive or offline work.** If going
   local, prefer llama.cpp with "submit originals" enabled if the backend and
   Lightroom share a filesystem — it's the only path that batches.
5. **Run search/cull/people/find-similar freely** — none of them touch an
   LLM at request time, so they're cheap to re-run as often as you like once
   indexing is done.
6. **Leave species identification off for a first bulk pass**, or leave its
   pre-filter on if you want it. It adds a second local model to every photo
   that clears the gate, and unlike the embedding it is not needed by any other
   feature — you can always come back and run it over just the wildlife folders,
   because photos already indexed for search keep their embedding for the gate
   to use.
7. **For AI Edit**, keep **Review each proposed edit** on until you've
   validated the results against your shooting style, then turn it off for
   large batches — it doesn't change compute cost, but it does gate throughput
   on how fast you click through the review dialog. The generation itself is
   local and needs no LLM at all.

---

## See also

- [Help: Choosing AI Model](Help-Choosing-AI-Model)
- [Help: Local AI Models](Help-Local-AI-Models)
- [Help: Analyze and Index](Help-Analyze-and-Index)
- [Help: AI Edit Photos](Help-AI-Edit)
- [Troubleshooting](Troubleshooting#9-local-model-timeout)
