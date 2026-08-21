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
/// `/v1/index/photos` substitutes `"photo"`, the edit routes keep `None` and let the
/// image sniffer decide — and baking either one in here would silently change
/// the other's behaviour.
#[derive(Debug)]
pub(crate) struct MultipartImage {
    pub(crate) bytes: Vec<u8>,
    pub(crate) filename: Option<String>,
}

/// Everything a multipart upload in this API can carry.
///
/// The four upload routes (`/v1/edit/recipe`, `/v1/edit/style`, `/v1/index/photos`, `/v1/edit/training`)
/// had four copies of the same field loop — 49 duplicated lines, and the
/// widest co-change surface in the crate: a change to how one route reads a
/// part had to be remembered in three other files. They differ only in which
/// parts they care about afterwards, which is what this shape captures.
///
/// The raw fields filter and default nothing: `photo_ids` and `uuids` stay
/// separate (only `/v1/index/photos` distinguishes them), empty values are kept, and
/// `filename` stays `None` rather than acquiring someone's default. Callers
/// that want the tidied view take [`MultipartForm::photo_ids_or_uuids`] or
/// [`MultipartForm::into_single_photo`], which do drop empties.
#[derive(Debug)]
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
    /// `/v1/edit/recipe`, `/v1/edit/style` and `/v1/edit/training` each take one image and one
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
#[derive(Debug)]
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

#[cfg(test)]
mod multipart_tests {
    use super::*;
    use axum::body::Body;
    use axum::extract::FromRequest;
    use axum::http::{header, Request};

    const BOUNDARY: &str = "----lrgtestboundary";

    /// One part: field name, optional filename (which makes it a file part),
    /// and the value.
    type Part<'a> = (&'a str, Option<&'a str>, &'a str);

    fn body_for(parts: &[Part<'_>]) -> Vec<u8> {
        let mut body: Vec<u8> = Vec::new();
        for (name, filename, value) in parts {
            body.extend_from_slice(format!("--{BOUNDARY}\r\n").as_bytes());
            match filename {
                Some(f) => body.extend_from_slice(
                    format!(
                        "Content-Disposition: form-data; name=\"{name}\"; filename=\"{f}\"\r\n\r\n"
                    )
                    .as_bytes(),
                ),
                None => body.extend_from_slice(
                    format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n").as_bytes(),
                ),
            }
            body.extend_from_slice(value.as_bytes());
            body.extend_from_slice(b"\r\n");
        }
        body.extend_from_slice(format!("--{BOUNDARY}--\r\n").as_bytes());
        body
    }

    async fn parse(parts: &[Part<'_>]) -> Result<MultipartForm, String> {
        let request = Request::builder()
            .method("POST")
            .header(
                header::CONTENT_TYPE,
                format!("multipart/form-data; boundary={BOUNDARY}"),
            )
            .body(Body::from(body_for(parts)))
            .unwrap();
        let mut multipart = Multipart::from_request(request, &())
            .await
            .expect("well-formed body");
        parse_multipart(&mut multipart).await
    }

    #[tokio::test]
    async fn sorts_parts_into_images_ids_and_fields() {
        let form = parse(&[
            ("image", Some("a.jpg"), "AAAA"),
            ("photo_id", None, "id-a"),
            ("image", Some("b.jpg"), "BBBB"),
            ("photo_id", None, "id-b"),
            ("uuid", None, "uuid-a"),
            ("tasks", None, "embeddings"),
        ])
        .await
        .expect("parses");

        assert_eq!(form.images.len(), 2);
        assert_eq!(form.images[0].filename.as_deref(), Some("a.jpg"));
        assert_eq!(form.images[1].bytes, b"BBBB");
        // photo_id and uuid stay apart: only `/v1/index/photos` distinguishes them, and
        // merging here would take that choice away from it.
        assert_eq!(form.photo_ids, vec!["id-a", "id-b"]);
        assert_eq!(form.uuids, vec!["uuid-a"]);
        assert_eq!(
            form.fields.get("tasks").map(String::as_str),
            Some("embeddings")
        );
    }

    #[tokio::test]
    async fn an_image_part_without_a_filename_keeps_none() {
        // `/v1/index/photos` substitutes "photo" itself; the parser must not decide for it.
        let form = parse(&[("image", Some(""), "AAAA")]).await.expect("parses");
        assert_eq!(form.images.len(), 1);
        assert_eq!(form.images[0].filename.as_deref(), Some(""));
    }

    #[tokio::test]
    async fn photo_ids_win_over_uuids_when_both_are_sent() {
        let form = parse(&[("photo_id", None, "id-a"), ("uuid", None, "uuid-a")])
            .await
            .expect("parses");
        assert_eq!(form.photo_ids_or_uuids(), vec!["id-a"]);
    }

    #[tokio::test]
    async fn uuids_are_the_fallback_when_no_photo_id_is_sent() {
        let form = parse(&[("uuid", None, "uuid-a")]).await.expect("parses");
        assert_eq!(form.photo_ids_or_uuids(), vec!["uuid-a"]);
    }

    #[tokio::test]
    async fn an_empty_id_does_not_suppress_the_uuid_fallback() {
        // The raw list keeps the empty value; the accessor drops it. Without
        // that, an empty `photo_id` made the list non-empty and the real id in
        // `uuid` was never reached.
        let form = parse(&[("photo_id", None, ""), ("uuid", None, "uuid-a")])
            .await
            .expect("parses");
        assert_eq!(form.photo_ids, vec![""]);
        assert_eq!(form.photo_ids_or_uuids(), vec!["uuid-a"]);
    }

    #[tokio::test]
    async fn single_photo_view_takes_the_first_image() {
        let form = parse(&[
            ("image", Some("a.jpg"), "AAAA"),
            ("image", Some("b.jpg"), "BBBB"),
            ("photo_id", None, "id-a"),
            ("db_path", None, "/tmp/x.db"),
        ])
        .await
        .expect("parses");

        let single = form.into_single_photo();
        assert_eq!(single.photo_ids, vec!["id-a"]);
        assert_eq!(single.image_bytes.as_deref(), Some(&b"AAAA"[..]));
        assert_eq!(single.filename.as_deref(), Some("a.jpg"));
        assert_eq!(
            single.fields.get("db_path").map(String::as_str),
            Some("/tmp/x.db")
        );
    }

    #[tokio::test]
    async fn single_photo_view_reports_no_image_rather_than_failing() {
        // `/v1/edit/recipe` turns this into its "mismatch between images and photo IDs"
        // reply; the parser's job is only to say the image is absent.
        let form = parse(&[("photo_id", None, "id-a")]).await.expect("parses");
        let single = form.into_single_photo();
        assert!(single.image_bytes.is_none());
        assert!(single.filename.is_none());
    }

    #[tokio::test]
    async fn a_malformed_body_is_an_error_not_a_panic() {
        let request = Request::builder()
            .method("POST")
            .header(
                header::CONTENT_TYPE,
                format!("multipart/form-data; boundary={BOUNDARY}"),
            )
            // A real boundary, but the body stops before the terminator —
            // what a client that dies mid-upload actually sends.
            .body(Body::from(format!(
                "--{BOUNDARY}\r\nContent-Disposition: form-data; \
                 name=\"image\"\r\n\r\ntruncated"
            )))
            .unwrap();
        let mut multipart = Multipart::from_request(request, &()).await.unwrap();
        let err = parse_multipart(&mut multipart).await.unwrap_err();
        assert!(err.contains("invalid multipart body"), "{err}");
    }
}
