//! Small helpers shared by `training.rs` and `style_edit.rs`: local-hour
//! resolution for time-of-day bucketing, and CLIP zero-shot scene-tag
//! probing against the already-loaded SigLIP2 model.

use chrono::{Local, TimeZone, Timelike};

use lrg_analysis::training::{scene_tags_from_similarities, SCENE_PROBES};
use lrg_ml::siglip::{l2_normalize, SiglipModel};

/// Local-hour equivalent of Python's `datetime.fromtimestamp(unix).hour`.
pub(crate) fn local_hour(capture_time_unix: Option<f64>) -> Option<u32> {
    let secs = capture_time_unix?;
    Local
        .timestamp_opt(secs as i64, 0)
        .single()
        .map(|dt| dt.hour())
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
