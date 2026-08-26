//! The worker thread: owns the model, the mtmd projector and the context, and
//! is the only place llama.cpp is ever touched.
//!
//! # The prefix cache
//!
//! Sequence 0 holds the evaluated run-constant prefix (`system_prompt` +
//! `stable_prompt`, rendered through the model's chat template). Photo `i` gets
//! sequence `i`, so the *first* photo of a batch shares sequence 0 with the
//! prefix and the others are seeded from it with `kv_cache_seq_cp(0, i, ..)`
//! before anyone appends a tail. Afterwards each tail is dropped with
//! `kv_cache_seq_rm(i, prefix_len, ..)` while the prefix in sequence 0 survives
//! for the next batch.
//!
//! Sequence 0 is shared rather than reserved because a sequence is not free:
//! llama.cpp splits the KV cache per sequence unless `kv_unified` is set, which
//! `llama-cpp-2` does not expose, so every sequence costs each photo a share of
//! the context window (see [`ctx_per_seq`]). Reserving one for the prefix cost
//! the single-photo default half of its window.
//!
//! The copy is metadata in a unified cache and a stream copy otherwise; either
//! way it beats re-evaluating the prefix, and either way it has to happen while
//! sequence 0 still holds nothing but the prefix.
//!
//! The split is derived, not assumed: the prompt is rendered once through the
//! model's chat template and then cut immediately after the run-constant text
//! (see [`WorkerState::split_render`]). A template whose output does not
//! contain that text verbatim simply falls back to evaluating the whole prompt
//! per photo, so an unusual template costs speed but never correctness.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::num::NonZeroU32;
use std::sync::mpsc::{Receiver, Sender};

use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::context::LlamaContext;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::{AddBos, LlamaChatMessage, LlamaChatTemplate, LlamaModel};
use llama_cpp_2::mtmd::{
    mtmd_default_marker, MtmdBitmap, MtmdContext, MtmdContextParams, MtmdInputText,
};
use llama_cpp_2::sampling::LlamaSampler;
use llama_cpp_2::token::LlamaToken;
use tokio::sync::oneshot;

use crate::device;
use crate::engine::{EngineInfo, GenerationOutput, GenerationRequest, LlamaEngineConfig};
use crate::LlamaError;

pub(crate) enum Job {
    Generate {
        requests: Vec<GenerationRequest>,
        reply: oneshot::Sender<Vec<Result<GenerationOutput, LlamaError>>>,
    },
}

/// Sequence 0 holds the pinned prefix, and is also the first photo's sequence.
const PREFIX_SEQ: i32 = 0;

/// The smallest per-photo window worth loading with. Below this a run cannot
/// fit a realistic prompt plus its answer, so `n_parallel` is reduced instead —
/// slower, but it produces results rather than `NoKvCacheSlot`.
const MIN_CTX_PER_SEQ: u32 = 4096;

struct PinnedPrefix {
    hash: u64,
    len: i32,
}

/// The prefix a whole chunk shares, already evaluated into sequence 0 and
/// copied into the chunk's other sequences.
struct ChunkPrefix {
    text: String,
    len: i32,
}

/// The window one sequence actually gets.
///
/// llama.cpp splits the KV cache per sequence unless `kv_unified` is set, and
/// `llama-cpp-2` exposes no way to set it, so llama.cpp's default (off) applies:
///
/// ```text
/// n_ctx_seq = kv_unified ? n_ctx : GGML_PAD(n_ctx / n_seq_max, 256)
/// ```
///
/// llama.cpp pads *up*; this rounds *down* to the same boundary, so the budget
/// handed to a photo can never exceed what the cache holds. Budgeting against
/// the undivided `n_ctx` is what produced `Decode Error 1: NoKvCacheSlot`
/// minutes into a run instead of an actionable error before it.
fn ctx_per_seq(n_ctx: u32, n_seq_max: u32) -> u32 {
    if n_seq_max <= 1 {
        return n_ctx;
    }
    (n_ctx / n_seq_max) & !255
}

/// Reduce `n_parallel` until each sequence still gets a usable window.
///
/// This is the behaviour the plugin's settings dialog and the wiki have always
/// described; it did not exist, and the context was quietly overrun instead.
fn parallel_that_fits(n_ctx: u32, requested: u32) -> u32 {
    requested.max(1).min((n_ctx / MIN_CTX_PER_SEQ).max(1))
}

pub(crate) fn run(
    config: LlamaEngineConfig,
    rx: Receiver<Job>,
    ready: Sender<Result<EngineInfo, LlamaError>>,
) {
    let backend = match LlamaBackend::init() {
        Ok(b) => b,
        Err(e) => {
            let _ = ready.send(Err(LlamaError::Load(format!(
                "llama backend init failed: {e}"
            ))));
            return;
        }
    };

    // Pick one GPU explicitly rather than letting llama.cpp split the model
    // across every registered device — on a laptop that means splitting it
    // between the discrete card and the integrated one. See
    // `device::select_gpu`.
    let mut model_params = LlamaModelParams::default().with_n_gpu_layers(config.n_gpu_layers);
    let devices = device::available();
    if let Some(gpu) = device::select_gpu(&devices) {
        log::info!(
            "llama: using {} via {} ({} MiB VRAM), out of {} registered device(s)",
            gpu.description,
            gpu.backend,
            gpu.memory_total / (1024 * 1024),
            devices.len()
        );
        match model_params.with_devices(&[gpu.index]) {
            Ok(p) => model_params = p,
            Err(e) => {
                // Not fatal: llama.cpp's own device choice is still workable,
                // it just may be the slower duplicate.
                log::warn!("llama: could not pin device {}: {e}", gpu.index);
                model_params = LlamaModelParams::default().with_n_gpu_layers(config.n_gpu_layers);
            }
        }
    } else {
        log::info!("llama: no GPU backend registered, running on CPU");
    }

    let model = match LlamaModel::load_from_file(&backend, &config.model_path, &model_params) {
        Ok(m) => m,
        Err(e) => {
            let _ = ready.send(Err(LlamaError::Load(format!(
                "{}: {e}",
                config.model_path.display()
            ))));
            return;
        }
    };

    // One sequence per concurrently decoded photo; the first photo shares
    // sequence 0 with the pinned prefix rather than the prefix reserving one of
    // its own. See the module docs.
    let n_parallel = parallel_that_fits(config.n_ctx, config.n_parallel);
    if n_parallel < config.n_parallel.max(1) {
        log::warn!(
            "llama: {} photos in parallel would leave each one under {MIN_CTX_PER_SEQ} tokens of \
             the {}-token context; using {n_parallel} instead. Raise the context size to run more \
             photos at once.",
            config.n_parallel.max(1),
            config.n_ctx,
        );
    }
    let n_seq_max = n_parallel;
    let mut ctx_params = LlamaContextParams::default()
        .with_n_ctx(NonZeroU32::new(config.n_ctx))
        .with_n_batch(config.n_ctx.min(2048))
        .with_n_seq_max(n_seq_max);
    if let Some(threads) = config.n_threads {
        ctx_params = ctx_params
            .with_n_threads(threads)
            .with_n_threads_batch(threads);
    }

    let mut ctx = match model.new_context(&backend, ctx_params) {
        Ok(c) => c,
        Err(e) => {
            let _ = ready.send(Err(LlamaError::Load(format!(
                "context creation failed: {e}"
            ))));
            return;
        }
    };

    let mtmd = match &config.mmproj_path {
        Some(path) => {
            let params = MtmdContextParams {
                use_gpu: config.n_gpu_layers > 0,
                print_timings: false,
                n_threads: config.n_threads.unwrap_or(4),
                media_marker: std::ffi::CString::new(mtmd_default_marker())
                    .expect("the mtmd marker never contains NUL"),
                ..Default::default()
            };
            match MtmdContext::init_from_file(&path.to_string_lossy(), &model, &params) {
                Ok(m) => Some(m),
                Err(e) => {
                    let _ = ready.send(Err(LlamaError::Load(format!("{}: {e}", path.display()))));
                    return;
                }
            }
        }
        None => None,
    };

    let supports_vision = mtmd.as_ref().is_some_and(MtmdContext::support_vision);
    let chat_template = resolve_chat_template(&model);
    if chat_template.is_none() {
        log::warn!(
            "{} has no chat template llama.cpp can apply; falling back to plain concatenation, \
             which usually degrades instruction following",
            config.model_path.display()
        );
    }

    // Walking the whole vocabulary costs hundreds of milliseconds, so build it
    // once and reuse it for every grammar and every request.
    let tok_env = LlamaSampler::llguidance_tok_env(&model);

    // Ask the context, not the config: llama.cpp pads `n_ctx` up.
    let n_ctx_seq = ctx_per_seq(ctx.n_ctx(), n_seq_max);
    let info = EngineInfo {
        model_path: config.model_path.display().to_string(),
        mmproj_path: config.mmproj_path.as_ref().map(|p| p.display().to_string()),
        n_ctx: ctx.n_ctx(),
        n_ctx_seq,
        n_parallel,
        supports_vision,
    };
    if ready.send(Ok(info)).is_err() {
        return;
    }

    let mut state = WorkerState {
        model: &model,
        mtmd: mtmd.as_ref(),
        chat_template,
        tok_env,
        prefix: None,
        n_parallel,
        n_ctx_seq: i32::try_from(n_ctx_seq).unwrap_or(i32::MAX),
    };

    // `recv` fails once the engine handle is dropped: that is the shutdown signal.
    while let Ok(job) = rx.recv() {
        match job {
            Job::Generate { requests, reply } => {
                let results = state.generate_batch(&mut ctx, &requests);
                let _ = reply.send(results);
            }
        }
    }

    log::info!("llama worker shutting down, releasing model");
}

/// A prompt split into the half worth pinning and the half that must be
/// evaluated per photo. `prefix` is `None` when the chat template does not
/// produce a clean split, in which case `tail` is the entire prompt.
struct SplitRender {
    prefix: Option<String>,
    tail: String,
}

/// Pick a chat template llama.cpp will actually apply.
///
/// A GGUF carries its chat template as Jinja, but `llama_chat_apply_template`
/// is the legacy C path: it applies only templates it recognises, by built-in
/// name or by sniffing familiar markers, and answers anything else with a bare
/// `-1` that says nothing about why. Current models ship large Jinja templates
/// for tool calling and thinking — Gemma 4's is 18 KB of macros — so the
/// model's own template is refused outright and every generation fails.
///
/// The template text still identifies the family even when llama.cpp cannot
/// execute it, so fall back to the matching built-in and confirm by applying
/// it. Built-ins render the same control tokens the model was trained on
/// (`<start_of_turn>` for Gemma), which is what actually matters here; what is
/// lost is only the Jinja-only extras this crate does not use.
fn resolve_chat_template(model: &LlamaModel) -> Option<LlamaChatTemplate> {
    // Applying it is the only way to learn whether llama.cpp accepts it.
    let applies = |template: &LlamaChatTemplate| {
        LlamaChatMessage::new("user".to_string(), "probe".to_string())
            .ok()
            .and_then(|m| model.apply_chat_template(template, &[m], true).ok())
            .is_some()
    };

    let own = model.chat_template(None).ok();
    if let Some(template) = &own {
        if applies(template) {
            return own;
        }
    }

    // The architecture is the reliable signal. Sniffing the template text does
    // not work: Gemma 4's builds its turns through macros and contains none of
    // the control markers verbatim.
    let arch = model
        .meta_val_str("general.architecture")
        .unwrap_or_default()
        .to_lowercase();
    let family: &[&str] = if arch.starts_with("gemma") {
        // Every Gemma generation delimits turns the same way; the built-ins
        // differ only in extras. Newest first, and each is verified below.
        &["gemma3", "gemma2", "gemma"]
    } else if arch.starts_with("llama") {
        &["llama3", "llama2"]
    } else if arch.starts_with("mistral") || arch.starts_with("mixtral") {
        &["mistral-v7", "mistral-v1"]
    } else if arch.starts_with("qwen") || arch.starts_with("phi") || arch.starts_with("yi") {
        &["chatml"]
    } else {
        &[]
    };

    // Architecture match first; chatml last, because most instruct models
    // tolerate it and it still beats plain concatenation.
    for name in family.iter().copied().chain(std::iter::once("chatml")) {
        let Ok(template) = LlamaChatTemplate::new(name) else {
            continue;
        };
        if applies(&template) {
            log::info!(
                "this model's own chat template is Jinja, which llama.cpp cannot apply; using \
                 the built-in '{name}' template for architecture '{arch}' instead"
            );
            return Some(template);
        }
    }
    None
}

struct WorkerState<'a> {
    model: &'a LlamaModel,
    mtmd: Option<&'a MtmdContext>,
    chat_template: Option<LlamaChatTemplate>,
    tok_env: toktrie::TokEnv,
    prefix: Option<PinnedPrefix>,
    n_parallel: u32,
    /// The per-photo window, i.e. what every budget check has to measure
    /// against. Never `ctx.n_ctx()`, which is the whole cache.
    n_ctx_seq: i32,
}

impl WorkerState<'_> {
    /// Apply the chat template, optionally with a leading system turn.
    fn apply_template(
        &self,
        template: &LlamaChatTemplate,
        system: Option<&str>,
        user: &str,
        add_ass: bool,
    ) -> Result<String, LlamaError> {
        let mut messages = Vec::new();
        if let Some(system) = system {
            messages.push(
                LlamaChatMessage::new("system".to_string(), system.to_string())
                    .map_err(|e| LlamaError::Prompt(e.to_string()))?,
            );
        }
        messages.push(
            LlamaChatMessage::new("user".to_string(), user.to_string())
                .map_err(|e| LlamaError::Prompt(e.to_string()))?,
        );
        self.model
            .apply_chat_template(template, &messages, add_ass)
            .map_err(|e| LlamaError::Prompt(e.to_string()))
    }

    fn render(&self, system: &str, user: &str, add_ass: bool) -> Result<String, LlamaError> {
        let Some(template) = &self.chat_template else {
            return Ok(if system.is_empty() {
                user.to_string()
            } else {
                format!("{system}\n\n{user}")
            });
        };
        let system = (!system.is_empty()).then_some(system);
        self.apply_template(template, system, user, add_ass)
    }

    /// Render the complete prompt once, then cut it immediately after the
    /// run-constant text.
    ///
    /// Rendering the stable half as a *separate* prompt does not work: a chat
    /// template closes the user turn at the end of the message, so the closing
    /// tag lands in the middle of what should be the shared prefix and the two
    /// renders diverge. Locating the stable text inside the full render instead
    /// yields a genuine prefix — template header, system turn, opening user tag
    /// and the stable text — without assuming anything about the template's
    /// syntax. If the text cannot be located (a template that escapes or
    /// rewrites content), pinning is skipped rather than risking a bad split.
    ///
    /// Note that prefix and tail are tokenized separately, so the token
    /// sequence can differ by one token at the seam versus a single-pass
    /// tokenization. That is inherent to prefix caching and harmless here: the
    /// split point is identical on every photo, so the model always sees the
    /// same tokenization for the same prompt shape.
    fn split_render(&self, request: &GenerationRequest) -> Result<SplitRender, LlamaError> {
        // A newline right after the stable text keeps the cut on a boundary the
        // tokenizer already treats as a break.
        let mut user = request.stable_prompt.clone();
        if !user.is_empty() {
            user.push('\n');
        }
        if !request.per_photo_prompt.is_empty() {
            user.push('\n');
            user.push_str(&request.per_photo_prompt);
        }
        // The media marker is where mtmd splices in the image tokens. It goes
        // last so every text token stays ahead of the image and the prefix
        // remains a contiguous run.
        if request.image.is_some() {
            user.push('\n');
            user.push_str(mtmd_default_marker());
        }

        let full = self.render(&request.system_prompt, &user, true)?;

        if !request.stable_prompt.is_empty() {
            if let Some(start) = full.find(&request.stable_prompt) {
                // Include the trailing newline we added, so the tail always
                // begins at the same kind of boundary.
                let cut = start + request.stable_prompt.len() + 1;
                if cut <= full.len() && full.is_char_boundary(cut) {
                    return Ok(SplitRender {
                        prefix: Some(full[..cut].to_string()),
                        tail: full[cut..].to_string(),
                    });
                }
            }
            log::debug!(
                "llama prefix cache disabled: the stable prompt is not present verbatim in the \
                 rendered chat template"
            );
        }
        Ok(SplitRender {
            prefix: None,
            tail: full,
        })
    }

    /// Ensure sequence 0 holds the KV for `prefix_text`, re-evaluating only if
    /// the text changed. Returns the prefix length in tokens and whether the
    /// existing cache was reused.
    fn ensure_prefix(
        &mut self,
        ctx: &mut LlamaContext,
        prefix_text: &str,
    ) -> Result<(i32, bool), LlamaError> {
        let mut hasher = DefaultHasher::new();
        prefix_text.hash(&mut hasher);
        let hash = hasher.finish();

        if let Some(pinned) = &self.prefix {
            if pinned.hash == hash {
                // Logged because this is the whole point of the crate, and a
                // silent hit is indistinguishable from no prefix caching at
                // all when reading a debug log.
                log::debug!("llama prefix cache hit: reusing {} tokens", pinned.len);
                return Ok((pinned.len, true));
            }
        }

        // A changed prefix invalidates every sequence, since the per-photo ones
        // are copies of it.
        self.prefix = None;
        ctx.clear_kv_cache();

        let tokens = self
            .model
            .str_to_token(prefix_text, AddBos::Always)
            .map_err(|e| LlamaError::Prompt(format!("tokenizing the prompt failed: {e}")))?;
        let len = i32::try_from(tokens.len())
            .map_err(|_| LlamaError::Prompt("the prompt is implausibly long".to_string()))?;

        let n_ctx_seq = self.n_ctx_seq;
        if len >= n_ctx_seq {
            return Err(LlamaError::ContextOverflow(format!(
                "the fixed part of the prompt is {len} tokens but each photo only gets \
                 {n_ctx_seq} of the context window. Shrink the keyword taxonomy or catalog \
                 vocabulary, lower Photos in parallel, or raise the context size in the plugin \
                 settings."
            )));
        }

        // A catalog vocabulary of a few hundred keywords can outgrow `n_batch`,
        // and a batch larger than that is rejected outright, so feed the prefix
        // in slices. Only the very last token needs logits, matching what
        // `add_sequence` would have requested.
        let n_batch = usize::try_from(ctx.n_batch()).unwrap_or(512).max(1);
        let mut pos = 0i32;
        for slice in tokens.chunks(n_batch) {
            let mut batch = LlamaBatch::new(slice.len(), 1);
            for (offset, token) in slice.iter().enumerate() {
                let at = pos + i32::try_from(offset).unwrap_or(0);
                batch
                    .add(*token, at, &[PREFIX_SEQ], at + 1 == len)
                    .map_err(|e| {
                        LlamaError::Inference(format!("building the prompt batch failed: {e}"))
                    })?;
            }
            ctx.decode(&mut batch)
                .map_err(|e| LlamaError::Inference(format!("evaluating the prompt failed: {e}")))?;
            pos += i32::try_from(slice.len()).unwrap_or(0);
        }

        self.prefix = Some(PinnedPrefix { hash, len });
        log::debug!("llama prefix cache miss: evaluated {len} tokens");
        Ok((len, false))
    }

    fn generate_batch(
        &mut self,
        ctx: &mut LlamaContext,
        requests: &[GenerationRequest],
    ) -> Vec<Result<GenerationOutput, LlamaError>> {
        let mut results = Vec::with_capacity(requests.len());
        for chunk in requests.chunks(self.n_parallel as usize) {
            results.append(&mut self.generate_chunk(ctx, chunk));
        }
        results
    }

    fn generate_chunk(
        &mut self,
        ctx: &mut LlamaContext,
        requests: &[GenerationRequest],
    ) -> Vec<Result<GenerationOutput, LlamaError>> {
        let mut results: Vec<Result<GenerationOutput, LlamaError>> = Vec::new();
        let mut slots: Vec<Slot> = Vec::new();
        let mut prefix_reused = false;

        // Stage timing, batch-level because that is the unit that is real
        // here: every live sequence decodes together, one token per sequence
        // per step, so per-photo decode time does not exist to be measured.
        // `prefill` covers vision-encode, prompt evaluation, the grammar
        // setup in `build_sampler` and the first sampled token — everything
        // before the decode loop. Splitting the grammar out separately is
        // worth it on the MLX backend, where the constraint compile runs into
        // hundreds of milliseconds; llguidance builds its parser here and has
        // not shown up as a cost worth its own timer.
        let t_prefill = std::time::Instant::now();

        // Pin the chunk's shared prefix and hand it to every sequence *before*
        // any photo appends a tail: sequence 0 both holds the prefix and serves
        // the first photo, and the copy is only valid while sequence 0 holds
        // nothing else.
        let (chunk_prefix, mut seeding) =
            match self.prepare_chunk(ctx, requests, &mut prefix_reused) {
                Ok(prepared) => prepared,
                // The shared prefix is shared: if it cannot be evaluated, no photo
                // in this chunk can run, and each one has to say so.
                Err(e) => return requests.iter().map(|_| Err(clone_err(&e))).collect(),
            };

        for (idx, request) in requests.iter().enumerate() {
            let seq_id = i32::try_from(idx).unwrap_or(i32::MAX);
            let prefilled = match seeding[idx].take() {
                Some(e) => Err(e),
                None => self.prefill_one(ctx, request, idx, seq_id, chunk_prefix.as_ref()),
            };
            match prefilled {
                Ok(slot) => {
                    results.push(Ok(GenerationOutput::default()));
                    slots.push(slot);
                }
                Err(e) => {
                    // Drop anything this sequence wrote, so one bad photo
                    // cannot corrupt the cache for the rest of the batch.
                    // Sequence 0 keeps the pinned prefix; everything past it
                    // goes.
                    let keep = if seq_id == PREFIX_SEQ {
                        self.prefix.as_ref().map_or(0, |p| p.len)
                    } else {
                        0
                    };
                    let _ = ctx.kv_cache_seq_rm(seq_id, Some(keep.cast_unsigned()), None);
                    results.push(Err(e));
                }
            }
        }

        let prefill_ms = t_prefill.elapsed().as_secs_f64() * 1000.0;
        let photos = slots.len();

        let t_decode = std::time::Instant::now();
        if !slots.is_empty() {
            if let Err(e) = self.decode(ctx, &mut slots) {
                for slot in &slots {
                    results[slot.idx] = Err(clone_err(&e));
                }
            }
        }
        let decode_ms = t_decode.elapsed().as_secs_f64() * 1000.0;

        let mut total_prompt: u32 = 0;
        let mut total_completion: u32 = 0;
        for slot in slots {
            // Release only this photo's tail; sequence 0 keeps the prefix. The
            // length is the slot's own: a photo that fell back to evaluating its
            // whole prompt holds no prefix, and keeping bytes of its prompt
            // would leave the next batch reading them as one.
            let _ = ctx.kv_cache_seq_rm(slot.seq_id, Some(slot.prefix_len.cast_unsigned()), None);
            if results[slot.idx].is_ok() {
                total_prompt = total_prompt.saturating_add(slot.prompt_tokens);
                total_completion = total_completion.saturating_add(slot.completion_tokens);
                results[slot.idx] = Ok(GenerationOutput {
                    text: slot.text,
                    prompt_tokens: slot.prompt_tokens,
                    completion_tokens: slot.completion_tokens,
                    prefix_reused,
                });
            }
        }

        if photos > 0 {
            // Tokens per second is over the batch: with `n_parallel > 1` the
            // weights are read once per step for every sequence at once, so
            // the rate rises with batch width while per-photo latency does
            // not. That is the whole point of batching, and reading the rate
            // as a per-photo figure would misread it.
            let rate = if decode_ms > 0.0 {
                f64::from(total_completion) / (decode_ms / 1000.0)
            } else {
                0.0
            };
            log::debug!(
                "stages: photos={photos} prefill={prefill_ms:.1}ms decode={decode_ms:.1}ms | \
                 tokens: prompt={total_prompt} out={total_completion} | \
                 decode {rate:.1} tok/s (batch) | prefix_reused={prefix_reused}"
            );
        }
        results
    }

    /// Evaluate the chunk's shared prefix into sequence 0 and copy it into the
    /// chunk's other sequences.
    ///
    /// This runs before any photo appends its tail, which is what makes the copy
    /// legal (llama.cpp asserts `seq_cp() is only supported for full KV buffers`)
    /// and what makes sharing sequence 0 with the first photo safe.
    ///
    /// Returns the pinned prefix, if the chunk has one, and per request the
    /// error that stopped its sequence from being seeded.
    fn prepare_chunk(
        &mut self,
        ctx: &mut LlamaContext,
        requests: &[GenerationRequest],
        prefix_reused: &mut bool,
    ) -> Result<(Option<ChunkPrefix>, Vec<Option<LlamaError>>), LlamaError> {
        let mut seeding: Vec<Option<LlamaError>> = requests.iter().map(|_| None).collect();

        // A render failure is reported per photo by `prefill_one`, on the photo
        // it belongs to; here it only means there is nothing to pin.
        let prefix_text = requests
            .first()
            .and_then(|first| self.split_render(first).ok())
            .and_then(|split| split.prefix);
        let Some(prefix_text) = prefix_text else {
            // Every photo will evaluate its whole prompt, starting by clearing
            // its sequence — including sequence 0, so nothing may stay pinned.
            self.prefix = None;
            return Ok((None, seeding));
        };

        let (len, reused) = self.ensure_prefix(ctx, &prefix_text)?;
        *prefix_reused = reused;

        for (idx, seeded) in seeding.iter_mut().enumerate().skip(1) {
            let seq_id = i32::try_from(idx).unwrap_or(i32::MAX);
            if let Err(e) = ctx.kv_cache_seq_cp(PREFIX_SEQ, seq_id, None, None) {
                *seeded = Some(LlamaError::Inference(format!(
                    "seeding sequence {seq_id} failed: {e}"
                )));
            }
        }

        Ok((
            Some(ChunkPrefix {
                text: prefix_text,
                len,
            }),
            seeding,
        ))
    }

    /// Hand a sequence to a photo that evaluates its whole prompt. Sequence 0
    /// doubles as the prefix's home, so emptying it un-pins the prefix too.
    fn clear_seq(&mut self, ctx: &mut LlamaContext, seq_id: i32) -> Result<(), LlamaError> {
        if seq_id == PREFIX_SEQ {
            self.prefix = None;
        }
        ctx.kv_cache_seq_rm(seq_id, None, None).map_err(|e| {
            LlamaError::Inference(format!("clearing sequence {seq_id} failed: {e}"))
        })?;
        Ok(())
    }

    fn prefill_one(
        &mut self,
        ctx: &mut LlamaContext,
        request: &GenerationRequest,
        idx: usize,
        seq_id: i32,
        chunk_prefix: Option<&ChunkPrefix>,
    ) -> Result<Slot, LlamaError> {
        if request.image.is_some() && self.mtmd.is_none() {
            return Err(LlamaError::Prompt(
                "this model has no multimodal projector loaded, so it cannot analyse images"
                    .to_string(),
            ));
        }

        let split = self.split_render(request)?;
        // `prepare_chunk` already evaluated the chunk's prefix and seeded this
        // sequence with it. Re-pinning here would clear the cache out from under
        // the photos already prefilled in this chunk.
        let shared = match (&split.prefix, chunk_prefix) {
            (Some(text), Some(chunk)) => text == &chunk.text,
            _ => false,
        };
        let (prefix_len, tail, add_special) = if shared {
            // The prefix carries BOS and the template's opening tags; re-adding
            // specials for the tail would corrupt both.
            (chunk_prefix.map_or(0, |c| c.len), split.tail, false)
        } else {
            // Either the template gave no clean split, or this photo's stable
            // half differs from the one the chunk pinned. Evaluate the whole
            // prompt into this sequence instead — correct, just not cached.
            self.clear_seq(ctx, seq_id)?;
            let whole = match split.prefix {
                Some(prefix) => prefix + &split.tail,
                None => split.tail,
            };
            (0, whole, true)
        };

        let n_ctx_seq = self.n_ctx_seq;
        let n_batch = i32::try_from(ctx.n_batch()).unwrap_or(512);
        let mut sampler = self.build_sampler(request)?;

        let (n_past, tail_tokens) = match self.mtmd {
            Some(mtmd) => {
                let bitmap = match &request.image {
                    Some(image) => Some(
                        MtmdBitmap::from_image_data(image.width, image.height, &image.data)
                            .map_err(|e| LlamaError::Image(e.to_string()))?,
                    ),
                    None => None,
                };
                let bitmaps: Vec<&MtmdBitmap> = bitmap.iter().collect();
                let chunks = mtmd
                    .tokenize(
                        MtmdInputText {
                            text: tail,
                            add_special,
                            parse_special: true,
                        },
                        &bitmaps,
                    )
                    .map_err(|e| {
                        LlamaError::Prompt(format!("preparing the photo prompt failed: {e}"))
                    })?;
                let tail_tokens = i32::try_from(chunks.total_tokens()).unwrap_or(i32::MAX);
                check_budget(prefix_len, tail_tokens, request.max_tokens, n_ctx_seq)?;
                let n_past = chunks
                    .eval_chunks(mtmd, ctx, prefix_len, seq_id, n_batch, true)
                    .map_err(|e| {
                        LlamaError::Inference(format!("evaluating the photo prompt failed: {e}"))
                    })?;
                (n_past, tail_tokens)
            }
            None => {
                let bos = if add_special {
                    AddBos::Always
                } else {
                    AddBos::Never
                };
                let tokens = self.model.str_to_token(&tail, bos).map_err(|e| {
                    LlamaError::Prompt(format!("tokenizing the prompt failed: {e}"))
                })?;
                let tail_tokens = i32::try_from(tokens.len()).unwrap_or(i32::MAX);
                check_budget(prefix_len, tail_tokens, request.max_tokens, n_ctx_seq)?;

                let mut batch = LlamaBatch::new(tokens.len().max(1), 1);
                for (offset, token) in tokens.iter().enumerate() {
                    let pos = prefix_len + i32::try_from(offset).unwrap_or(0);
                    batch
                        .add(*token, pos, &[seq_id], offset + 1 == tokens.len())
                        .map_err(|e| {
                            LlamaError::Inference(format!("building the batch failed: {e}"))
                        })?;
                }
                ctx.decode(&mut batch).map_err(|e| {
                    LlamaError::Inference(format!("evaluating the prompt failed: {e}"))
                })?;
                (prefix_len + tail_tokens, tail_tokens)
            }
        };

        // Sample this sequence's first token *now*, while its own decode still
        // owns the logits buffer. Deferring it would lose them to the next
        // sequence's prefill.
        //
        // The index is a *batch* index, not a logits-row index: the prefill
        // requested logits only for its final token, so anything but -1 ("the
        // last output") aborts inside llama.cpp.
        // NB: `sample()` already accepts the token into the sampler chain.
        // Calling `accept()` again double-feeds the grammar matcher, which
        // corrupts its state; `llg_apply` then fails to compute a mask and
        // silently stops constraining, so the model quietly ignores the schema.
        let first = sampler.sample(ctx, -1);

        let mut slot = Slot {
            idx,
            seq_id,
            prefix_len,
            n_past,
            prompt_tokens: (prefix_len + tail_tokens).cast_unsigned(),
            completion_tokens: 0,
            text: String::new(),
            bytes: Vec::new(),
            sampler,
            budget: request.max_tokens.max(1),
            next_input: None,
            logit_idx: -1,
            done: false,
        };
        if self.model.is_eog_token(first) {
            slot.done = true;
        } else {
            slot.push_token(self.model, first);
            slot.next_input = Some(first);
        }
        Ok(slot)
    }

    fn build_sampler(&self, request: &GenerationRequest) -> Result<LlamaSampler, LlamaError> {
        let mut stages = Vec::new();
        if let Some(schema) = &request.schema {
            let factory = llguidance::ParserFactory::new_simple(&self.tok_env)
                .map_err(|e| LlamaError::Prompt(format!("grammar setup failed: {e}")))?;
            let grammar = llguidance::api::TopLevelGrammar::from_json_schema(schema.clone());
            // `Matcher::new` swallows a failed parser into a poisoned matcher
            // that rejects every token, which surfaces much later as a baffling
            // "byte '{' fails parse". Fail here, with the schema's actual error.
            let parser = factory.create_parser(grammar).map_err(|e| {
                LlamaError::Prompt(format!(
                    "the response schema is not usable as a grammar: {e}"
                ))
            })?;
            stages.push(LlamaSampler::from(llguidance::Matcher::new(Ok(parser))));
        }
        if request.temperature <= 0.0 {
            stages.push(LlamaSampler::greedy());
        } else {
            stages.push(LlamaSampler::temp(request.temperature));
            stages.push(LlamaSampler::dist(0));
        }
        Ok(LlamaSampler::chain_simple(stages))
    }

    /// Decode every live sequence together, one token per sequence per step.
    /// This is where batching pays off: the weights are read once per step
    /// regardless of how many photos are in flight.
    fn decode(&self, ctx: &mut LlamaContext, slots: &mut [Slot]) -> Result<(), LlamaError> {
        let n_ctx_seq = self.n_ctx_seq;
        let capacity = slots.len().max(1);
        let mut batch = LlamaBatch::new(capacity, i32::try_from(capacity).unwrap_or(1) + 1);

        loop {
            batch.clear();
            let mut any = false;
            for slot in slots.iter_mut() {
                slot.logit_idx = -1;
                if slot.done {
                    continue;
                }
                if slot.completion_tokens >= slot.budget || slot.n_past >= n_ctx_seq {
                    slot.done = true;
                    continue;
                }
                let Some(token) = slot.next_input else {
                    slot.done = true;
                    continue;
                };
                slot.logit_idx = batch.n_tokens();
                batch
                    .add(token, slot.n_past, &[slot.seq_id], true)
                    .map_err(|e| {
                        LlamaError::Inference(format!("building the batch failed: {e}"))
                    })?;
                any = true;
            }
            if !any {
                break;
            }

            ctx.decode(&mut batch)
                .map_err(|e| LlamaError::Inference(format!("decode failed: {e}")))?;

            for slot in slots.iter_mut() {
                if slot.done || slot.logit_idx < 0 {
                    continue;
                }
                slot.n_past += 1;
                // `sample()` accepts internally — see the note in `prefill_one`.
                let token = slot.sampler.sample(ctx, slot.logit_idx);
                if self.model.is_eog_token(token) {
                    slot.done = true;
                    slot.next_input = None;
                    continue;
                }
                slot.push_token(self.model, token);
                slot.next_input = Some(token);
            }
        }

        for slot in slots.iter_mut() {
            slot.finish();
        }
        Ok(())
    }
}

struct Slot {
    idx: usize,
    seq_id: i32,
    /// How much of this sequence is shared prefix and must survive cleanup.
    prefix_len: i32,
    n_past: i32,
    prompt_tokens: u32,
    completion_tokens: u32,
    text: String,
    /// Raw piece bytes. A single UTF-8 character can straddle two tokens, so
    /// the bytes are accumulated and decoded once at the end rather than
    /// per token.
    bytes: Vec<u8>,
    sampler: LlamaSampler,
    budget: u32,
    next_input: Option<LlamaToken>,
    logit_idx: i32,
    done: bool,
}

impl Slot {
    fn push_token(&mut self, model: &LlamaModel, token: LlamaToken) {
        if let Ok(piece) = token_bytes(model, token) {
            self.bytes.extend_from_slice(&piece);
        }
        self.completion_tokens += 1;
    }

    fn finish(&mut self) {
        self.text = String::from_utf8_lossy(&self.bytes).into_owned();
    }
}

fn token_bytes(model: &LlamaModel, token: LlamaToken) -> Result<Vec<u8>, LlamaError> {
    use llama_cpp_2::TokenToStringError;
    match model.token_to_piece_bytes(token, 32, false, None) {
        Ok(bytes) => Ok(bytes),
        // llama.cpp reports the required size as a negative number.
        Err(TokenToStringError::InsufficientBufferSpace(needed)) => model
            .token_to_piece_bytes(token, usize::try_from(-needed).unwrap_or(256), false, None)
            .map_err(|e| LlamaError::Inference(e.to_string())),
        Err(e) => Err(LlamaError::Inference(e.to_string())),
    }
}

/// `n_ctx_seq` is the window *one photo* gets, not the whole cache — see
/// [`ctx_per_seq`].
fn check_budget(
    prefix_len: i32,
    tail_tokens: i32,
    max_tokens: u32,
    n_ctx_seq: i32,
) -> Result<(), LlamaError> {
    let needed = i64::from(prefix_len) + i64::from(tail_tokens) + i64::from(max_tokens);
    if needed > i64::from(n_ctx_seq) {
        return Err(LlamaError::ContextOverflow(format!(
            "the prompt ({} tokens) plus the requested {max_tokens} output tokens exceeds the \
             {n_ctx_seq} tokens each photo gets of the context window. Lower Max Tokens, shrink \
             the keyword taxonomy, lower Photos in parallel, or raise the context size in the \
             plugin settings.",
            prefix_len + tail_tokens
        )));
    }
    Ok(())
}

/// `LlamaError` is not `Clone` by design (it wraps opaque native failures), but
/// a whole-batch failure has to be reported against every request in it.
fn clone_err(e: &LlamaError) -> LlamaError {
    match e {
        LlamaError::ContextOverflow(m) => LlamaError::ContextOverflow(m.clone()),
        LlamaError::Prompt(m) => LlamaError::Prompt(m.clone()),
        LlamaError::Image(m) => LlamaError::Image(m.clone()),
        LlamaError::ModelNotFound(m) => LlamaError::ModelNotFound(m.clone()),
        LlamaError::Load(m) => LlamaError::Load(m.clone()),
        LlamaError::Inference(m) => LlamaError::Inference(m.clone()),
        LlamaError::Shutdown => LlamaError::Shutdown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budget_check_rejects_overflow_before_any_decode() {
        assert!(check_budget(1000, 500, 200, 4096).is_ok());
        let err = check_budget(3000, 1000, 512, 4096).unwrap_err();
        assert!(matches!(err, LlamaError::ContextOverflow(_)));
        assert!(err.to_string().contains("4096 tokens each photo gets"));
    }

    #[test]
    fn budget_check_is_exact_at_the_boundary() {
        assert!(check_budget(2000, 1000, 1096, 4096).is_ok());
        assert!(check_budget(2000, 1000, 1097, 4096).is_err());
    }

    /// The bug behind #316: the KV cache is split per sequence, so budgeting
    /// against the undivided `n_ctx` promises a photo roughly `n_seq_max` times
    /// the room it has, and the overrun only surfaces mid-decode as
    /// `NoKvCacheSlot`.
    #[test]
    fn context_is_divided_between_sequences() {
        assert_eq!(ctx_per_seq(8192, 1), 8192);
        assert_eq!(ctx_per_seq(8192, 2), 4096);
        // Rounded down to llama.cpp's 256 boundary, never up: 2730 -> 2560.
        assert_eq!(ctx_per_seq(8192, 3), 2560);
    }

    #[test]
    fn a_budget_that_fits_the_whole_cache_can_still_overflow_one_sequence() {
        // 8192 total, two photos at once: 4096 each. A 2000-token prompt asking
        // for 4096 output fits the cache and not the sequence.
        let per_seq = i32::try_from(ctx_per_seq(8192, 2)).unwrap();
        assert!(check_budget(1500, 500, 4096, 8192).is_ok());
        assert!(check_budget(1500, 500, 4096, per_seq).is_err());
    }

    #[test]
    fn parallelism_is_capped_by_what_the_context_can_be_split_into() {
        // The shipped defaults: 8192 tokens, two photos at once.
        assert_eq!(parallel_that_fits(8192, 2), 2);
        assert_eq!(ctx_per_seq(8192, parallel_that_fits(8192, 2)), 4096);
        // Asking for more than the context can serve is reduced, not honoured.
        assert_eq!(parallel_that_fits(8192, 8), 2);
        assert_eq!(parallel_that_fits(32768, 8), 8);
        // Never below one, however small the context.
        assert_eq!(parallel_that_fits(1024, 4), 1);
        assert_eq!(parallel_that_fits(8192, 0), 1);
    }

    #[test]
    fn every_sequence_keeps_a_usable_window() {
        for n_ctx in [4096u32, 8192, 16384, 32768, 131_072] {
            for requested in 1..=16u32 {
                let n_seq_max = parallel_that_fits(n_ctx, requested);
                assert!(n_seq_max >= 1);
                let per_seq = ctx_per_seq(n_ctx, n_seq_max);
                assert!(
                    per_seq >= n_ctx.min(MIN_CTX_PER_SEQ),
                    "n_ctx={n_ctx} requested={requested} left {per_seq} per photo"
                );
            }
        }
    }
}
