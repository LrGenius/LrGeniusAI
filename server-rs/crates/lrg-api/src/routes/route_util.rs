//! Small helpers shared by `training.rs`, `style_edit.rs`, `group_similar.rs`
//! and `index_upload.rs`: local-hour resolution for time-of-day bucketing,
//! CLIP zero-shot scene-tag probing, and the CLIP-IQA prompt-set cache.

use std::collections::HashMap;

use chrono::{Local, TimeZone, Timelike};

use lrg_analysis::training::{scene_tags_from_similarities, SCENE_PROBES};
use lrg_ml::clip_iqa::{IqaPrompts, PromptSet};
use lrg_ml::siglip::{l2_normalize, SiglipModel};

use crate::state::AppState;

/// Local-hour equivalent of Python's `datetime.fromtimestamp(unix).hour`.
pub(crate) fn local_hour(capture_time_unix: Option<f64>) -> Option<u32> {
    let secs = capture_time_unix?;
    Local
        .timestamp_opt(secs as i64, 0)
        .single()
        .map(|dt| dt.hour())
}

/// Fetch a CLIP-IQA prompt set from `AppState::clip_iqa`, embedding it on
/// first use.
///
/// Takes the already-locked cache rather than locking itself, so a caller
/// scoring a whole batch holds the lock once instead of per record.
///
/// Returns `None` when the text tower could not run — a missing prompt set is
/// a missing *signal*, never an error: culling falls back to its heuristic and
/// the species gate falls back to letting the photo through.
pub(crate) fn ensure_prompt_set<'a>(
    state: &AppState,
    cache: &'a mut HashMap<&'static str, IqaPrompts>,
    set: PromptSet,
) -> Option<&'a IqaPrompts> {
    // Keyed by the set, not by the caller's metadata key: two callers asking
    // the same question must share one text-tower pass.
    let cache_key = set.as_str();
    if !cache.contains_key(cache_key) {
        match IqaPrompts::compute_set(&state.siglip, set) {
            Ok(p) => {
                log::info!(
                    "CLIP-IQA: embedded {} {cache_key} prompt pairs",
                    p.pair_count()
                );
                cache.insert(cache_key, p);
            }
            Err(e) => {
                log::warn!("CLIP-IQA {cache_key} prompts unavailable, skipping that signal: {e}");
                return None;
            }
        }
    }
    cache.get(cache_key)
}

/// [`ensure_prompt_set`] for a single embedding, locking the cache itself.
pub(crate) fn score_prompt_set(state: &AppState, set: PromptSet, embedding: &[f32]) -> Option<f64> {
    let mut cache = state.clip_iqa.lock().unwrap();
    ensure_prompt_set(state, &mut cache, set)?.score(embedding)
}

/// Port of `compute_scene_tags`: probe the image embedding against the
/// fixed scene-type text prompts via this project's own SigLIP2 model
/// (see the module note in `lrg-analysis::training` on why this is more
/// consistent than Python's separate/inconsistent ViT-L-14 fallback).
pub(crate) fn compute_scene_tags(siglip: &SiglipModel, image_embedding: &[f32]) -> Vec<String> {
    let probe_texts: Vec<String> = SCENE_PROBES
        .iter()
        .map(|&(_, text)| text.to_string())
        .collect();
    let Ok(mut text_embs) = siglip.embed_text(&probe_texts) else {
        return Vec::new();
    };
    let sims: Vec<(String, f64)> = SCENE_PROBES
        .iter()
        .zip(text_embs.iter_mut())
        .map(|(&(name, _), text_emb)| {
            l2_normalize(text_emb);
            let sim: f64 = image_embedding
                .iter()
                .zip(text_emb.iter())
                .map(|(a, b)| *a as f64 * *b as f64)
                .sum();
            (name.to_string(), sim)
        })
        .collect();
    scene_tags_from_similarities(&sims)
}
