//! Small helpers shared by `training.rs`, `style_edit.rs`, `group_similar.rs`
//! and `index_upload.rs`: multipart request parsing, local-hour resolution for
//! time-of-day bucketing, CLIP zero-shot scene-tag probing, and the CLIP-IQA
//! prompt-set cache.

use std::collections::HashMap;

use axum::extract::Multipart;
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

/// One `image` part of an upload.
///
/// `filename` stays optional because the callers disagree about the default —
/// `/index` substitutes `"photo"`, the edit routes keep `None` and let the
/// image sniffer decide — and baking either one in here would silently change
/// the other's behaviour.
pub(crate) struct MultipartImage {
    pub(crate) bytes: Vec<u8>,
    pub(crate) filename: Option<String>,
}

/// Everything a multipart upload in this API can carry.
///
/// The four upload routes (`/edit`, `/style_edit`, `/index`, `/training/add`)
/// had four copies of the same field loop — 49 duplicated lines, and the
/// widest co-change surface in the crate: a change to how one route reads a
/// part had to be remembered in three other files. They differ only in which
/// parts they care about afterwards, which is what this shape captures.
///
/// The raw fields filter and default nothing: `photo_ids` and `uuids` stay
/// separate (only `/index` distinguishes them), empty values are kept, and
/// `filename` stays `None` rather than acquiring someone's default. Callers
/// that want the tidied view take [`MultipartForm::photo_ids_or_uuids`] or
/// [`MultipartForm::into_single_photo`], which do drop empties.
pub(crate) struct MultipartForm {
    pub(crate) images: Vec<MultipartImage>,
    pub(crate) photo_ids: Vec<String>,
    pub(crate) uuids: Vec<String>,
    pub(crate) fields: HashMap<String, String>,
}

impl MultipartForm {
    /// `photo_id` parts, falling back to `uuid` parts, with empties dropped.
    ///
    /// What the single-photo routes mean by "the photo this is about": they
    /// accept either spelling and never both, so a fallback rather than a
    /// concatenation keeps the result unambiguous if a client sends both.
    pub(crate) fn photo_ids_or_uuids(&self) -> Vec<String> {
        let ids: Vec<String> = self
            .photo_ids
            .iter()
            .filter(|s| !s.is_empty())
            .cloned()
            .collect();
        if !ids.is_empty() {
            return ids;
        }
        self.uuids
            .iter()
            .filter(|s| !s.is_empty())
            .cloned()
            .collect()
    }

    /// Collapses the form to what a single-photo route actually uses.
    ///
    /// `/edit`, `/style_edit` and `/training/add` each take one image and one
    /// id; this hands them that directly so the call site is a destructuring
    /// `let` rather than four mutable accumulators.
    pub(crate) fn into_single_photo(self) -> SinglePhotoForm {
        let photo_ids = self.photo_ids_or_uuids();
        let (image_bytes, filename) = match self.images.into_iter().next() {
            Some(img) => (Some(img.bytes), img.filename),
            None => (None, None),
        };
        SinglePhotoForm {
            photo_ids,
            image_bytes,
            filename,
            fields: self.fields,
        }
    }
}

/// [`MultipartForm`] as the single-photo routes see it.
pub(crate) struct SinglePhotoForm {
    pub(crate) photo_ids: Vec<String>,
    pub(crate) image_bytes: Option<Vec<u8>>,
    pub(crate) filename: Option<String>,
    pub(crate) fields: HashMap<String, String>,
}

/// Reads a whole multipart body into [`MultipartForm`].
///
/// The `Err` is the human half of a 400 — callers wrap it in whatever error
/// envelope they already use, since those differ across these routes.
pub(crate) async fn parse_multipart(multipart: &mut Multipart) -> Result<MultipartForm, String> {
    let mut form = MultipartForm {
        images: Vec::new(),
        photo_ids: Vec::new(),
        uuids: Vec::new(),
        fields: HashMap::new(),
    };

    loop {
        let field = match multipart.next_field().await {
            Ok(Some(f)) => f,
            Ok(None) => break,
            Err(e) => return Err(format!("invalid multipart body: {e}")),
        };
        let name = field.name().unwrap_or("").to_string();
        match name.as_str() {
            "image" => {
                let filename = field.file_name().map(str::to_string);
                match field.bytes().await {
                    Ok(bytes) => form.images.push(MultipartImage {
                        bytes: bytes.to_vec(),
                        filename,
                    }),
                    // Logged rather than dropped in silence: an image part the
                    // server could not read means the user's photo did not get
                    // indexed, and the count of successes would not say so.
                    Err(e) => log::warn!("Skipping unreadable image field: {e}"),
                }
            }
            "photo_id" => {
                if let Ok(text) = field.text().await {
                    form.photo_ids.push(text);
                }
            }
            "uuid" => {
                if let Ok(text) = field.text().await {
                    form.uuids.push(text);
                }
            }
            _ => {
                if let Ok(text) = field.text().await {
                    form.fields.insert(name, text);
                }
            }
        }
    }
    Ok(form)
}
