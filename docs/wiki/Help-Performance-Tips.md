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
| **AI Edit Photos** | **Yes, once per photo** | Model choice, export size, review dialog |
| **Deduplicate Keyword Synonyms** | Optional, once per cluster (not per photo) | Number of keyword clusters, not catalog size |
| Analyze & Index — **search embeddings** (SigLIP2) | No — local ONNX model, always | Photo count only; same speed regardless of LLM choice |
| **Cull Photos** | No | Runs on metrics already computed during indexing |
| **People / Find Similar Faces** | No | Vector lookup against already-indexed face embeddings |
| **Find Similar Images** | No | Vector lookup (CLIP) or perceptual hash (phash) |
| **Advanced Search** | No | Vector lookup against existing embeddings |

The practical upshot: **Analyze & Index and AI Edit are the only features
where your LLM choice matters for speed.** Everything else is bounded by
local compute on the backend machine and is fast regardless of whether you
picked a cheap cloud model or a big local one — but those features all
*depend on* having run Analyze & Index first (embeddings for search/cull/find
similar, face detection for people/faces).

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
backend reads the RAW/JPEG/HEIC file directly (local backend) or uploads it
raw (remote provider), instead of Lightroom rendering a JPEG first. It's the
fastest indexing path, with tradeoffs:

- Uses the file's embedded camera preview, so **crops and develop edits are
  not reflected** in what the AI sees.
- **Location keywords from Lightroom's address lookup are unavailable.**
- Results may differ slightly from the classic export-based path.

If you use it, re-index later the classic way if you want search results
consistent with your final edited/cropped images.

---

## 5. llama.cpp batching (built-in local engine only)

If you're on the built-in **llamacpp** engine, batching multiple photos into
one request only happens when **both** are true:

1. **Submit original files** (above) is enabled, and
2. the backend can read the files locally (i.e. it's running on the same
   machine as Lightroom, or has access to the same file paths).

Under those conditions, llama.cpp shares one pinned prompt prefix across the
photos in a group and decodes them in parallel — noticeably faster than one
request per photo. Every other path (any cloud provider, or llama.cpp without
originals) sends **one photo per request**, because remote APIs are billed
and rate-limited per call and gain nothing from grouping.

Two advanced llama.cpp settings trade against each other:

- **Context size (tokens)** — how much prompt+photo the model can hold at once.
- **Photos in parallel** — how many photos decode concurrently.

The whole group has to fit in the context window alongside the shared
prefix; the backend automatically reduces the parallel count (with a
warning) rather than overrunning it. Leave both empty for sensible defaults
unless you've hit that warning.

**MLX (Apple silicon) never batches** — it allocates a fresh cache per
request and always processes one photo at a time, regardless of these
settings. This is a deliberate limitation of the MLX decoder, not a bug. On
Apple silicon, run the same 10–20 photos through both `llamacpp` and `mlx`
to see which wins for your batch size before committing to one for a large run.

---

## 6. The plugin always processes one photo at a time

Regardless of provider, the plugin runs a **single worker** — it does not
send several photos to the LLM concurrently. This is intentional: earlier
attempts at multi-threaded indexing caused crashes on Windows, so it's
hardcoded off on every platform. The one exception is llama.cpp's own
server-side batching (§5), which is not the same as the plugin issuing
concurrent requests.

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
- **Face detection during indexing** is local compute (SCRFD + ArcFace), not
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
6. **For AI Edit**, keep **Review each proposed edit** on until you've
   validated a model+preset combination on your shooting style, then turn it
   off for large batches — it doesn't change compute cost, but it does gate
   throughput on how fast you click through the review dialog.

---

## See also

- [Help: Choosing AI Model](Help-Choosing-AI-Model)
- [Help: Local AI Models](Help-Local-AI-Models)
- [Help: Analyze and Index](Help-Analyze-and-Index)
- [Help: AI Edit Photos](Help-AI-Edit)
- [Troubleshooting](Troubleshooting#9-local-model-timeout)
