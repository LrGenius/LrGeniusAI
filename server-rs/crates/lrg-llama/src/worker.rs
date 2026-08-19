//! The worker thread: owns the model, the mtmd projector and the context, and
//! is the only place llama.cpp is ever touched.
//!
//! # The prefix cache
//!
//! Sequence 0 is reserved. It holds the evaluated run-constant prefix
//! (`system_prompt` + `stable_prompt`, rendered through the model's chat
//! template) and is never generated into. Each photo `i` gets sequence `i + 1`,
//! seeded with `kv_cache_seq_cp(0, i + 1, ..)` — a metadata-only copy, not a
//! re-evaluation — then evaluates only its own volatile tail and image.
//! Afterwards that tail is dropped with `kv_cache_seq_rm(i + 1, prefix_len, ..)`
//! while sequence 0 survives for the next batch.
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

/// Sequence 0 is the pinned prefix and is never generated into.
const PREFIX_SEQ: i32 = 0;

struct PinnedPrefix {
    hash: u64,
    len: i32,
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

    // One sequence for the pinned prefix plus one per concurrently decoded photo.
    let n_parallel = config.n_parallel.max(1);
    let n_seq_max = n_parallel + 1;
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

    let info = EngineInfo {
        model_path: config.model_path.display().to_string(),
        mmproj_path: config.mmproj_path.as_ref().map(|p| p.display().to_string()),
        n_ctx: ctx.n_ctx(),
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

        let n_ctx = i32::try_from(ctx.n_ctx()).unwrap_or(i32::MAX);
        if len >= n_ctx {
            return Err(LlamaError::ContextOverflow(format!(
                "the fixed part of the prompt is {len} tokens but the context window is only \
                 {n_ctx}. Shrink the keyword taxonomy or catalog vocabulary, or raise the \
                 context size in the plugin settings."
            )));
        }

        let mut batch = LlamaBatch::new(tokens.len(), 1);
        batch
            .add_sequence(&tokens, PREFIX_SEQ, false)
            .map_err(|e| LlamaError::Inference(format!("building the prompt batch failed: {e}")))?;
        ctx.decode(&mut batch)
            .map_err(|e| LlamaError::Inference(format!("evaluating the prompt failed: {e}")))?;

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

        for (idx, request) in requests.iter().enumerate() {
            let seq_id = i32::try_from(idx).unwrap_or(i32::MAX) + 1;
            match self.prefill_one(ctx, request, idx, seq_id, &mut prefix_reused) {
                Ok(slot) => {
                    results.push(Ok(GenerationOutput::default()));
                    slots.push(slot);
                }
                Err(e) => {
                    // Drop anything this sequence wrote, so one bad photo
                    // cannot corrupt the cache for the rest of the batch.
                    let _ = ctx.kv_cache_seq_rm(seq_id, None, None);
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
        let prefix_len = self.prefix.as_ref().map_or(0, |p| p.len);
        for slot in slots {
            // Release only this photo's tail; sequence 0 keeps the prefix.
            let _ = ctx.kv_cache_seq_rm(slot.seq_id, Some(prefix_len.cast_unsigned()), None);
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

    fn prefill_one(
        &mut self,
        ctx: &mut LlamaContext,
        request: &GenerationRequest,
        idx: usize,
        seq_id: i32,
        prefix_reused: &mut bool,
    ) -> Result<Slot, LlamaError> {
        if request.image.is_some() && self.mtmd.is_none() {
            return Err(LlamaError::Prompt(
                "this model has no multimodal projector loaded, so it cannot analyse images"
                    .to_string(),
            ));
        }

        let split = self.split_render(request)?;
        let prefix_len = match &split.prefix {
            Some(prefix_text) => {
                let (len, reused) = self.ensure_prefix(ctx, prefix_text)?;
                *prefix_reused = reused;
                // Metadata-only copy of the prefix KV — no recomputation.
                //
                // The range must be the whole sequence: llama.cpp asserts
                // `seq_cp() is only supported for full KV buffers`. That is
                // exactly what we want anyway, because sequence 0 is reserved
                // for the prefix and never generated into, so "all of seq 0"
                // and "the prefix" are the same thing.
                ctx.kv_cache_seq_cp(PREFIX_SEQ, seq_id, None, None)
                    .map_err(|e| {
                        LlamaError::Inference(format!("seeding sequence {seq_id} failed: {e}"))
                    })?;
                len
            }
            None => {
                ctx.kv_cache_seq_rm(seq_id, None, None).map_err(|e| {
                    LlamaError::Inference(format!("clearing sequence {seq_id} failed: {e}"))
                })?;
                0
            }
        };
        // The prefix carries BOS and the template's opening tags; re-adding
        // specials for the tail would corrupt both.
        let add_special = split.prefix.is_none();

        let n_ctx = i32::try_from(ctx.n_ctx()).unwrap_or(i32::MAX);
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
                            text: split.tail,
                            add_special,
                            parse_special: true,
                        },
                        &bitmaps,
                    )
                    .map_err(|e| {
                        LlamaError::Prompt(format!("preparing the photo prompt failed: {e}"))
                    })?;
                let tail_tokens = i32::try_from(chunks.total_tokens()).unwrap_or(i32::MAX);
                check_budget(prefix_len, tail_tokens, request.max_tokens, n_ctx)?;
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
                let tokens = self.model.str_to_token(&split.tail, bos).map_err(|e| {
                    LlamaError::Prompt(format!("tokenizing the prompt failed: {e}"))
                })?;
                let tail_tokens = i32::try_from(tokens.len()).unwrap_or(i32::MAX);
                check_budget(prefix_len, tail_tokens, request.max_tokens, n_ctx)?;

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
        let n_ctx = i32::try_from(ctx.n_ctx()).unwrap_or(i32::MAX);
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
                if slot.completion_tokens >= slot.budget || slot.n_past >= n_ctx {
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

fn check_budget(
    prefix_len: i32,
    tail_tokens: i32,
    max_tokens: u32,
    n_ctx: i32,
) -> Result<(), LlamaError> {
    let needed = i64::from(prefix_len) + i64::from(tail_tokens) + i64::from(max_tokens);
    if needed > i64::from(n_ctx) {
        return Err(LlamaError::ContextOverflow(format!(
            "the prompt ({} tokens) plus the requested {max_tokens} output tokens exceeds the \
             {n_ctx}-token context window. Lower Max Tokens, shrink the keyword taxonomy, or \
             raise the context size in the plugin settings.",
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
        assert!(err.to_string().contains("4096-token context window"));
    }

    #[test]
    fn budget_check_is_exact_at_the_boundary() {
        assert!(check_budget(2000, 1000, 1096, 4096).is_ok());
        assert!(check_budget(2000, 1000, 1097, 4096).is_err());
    }
}
