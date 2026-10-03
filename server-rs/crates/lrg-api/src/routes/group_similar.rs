//! `/v1/cull/groups` and `/v1/cull/grade` — port of `routes/search.py`'s
//! `group_similar_route`/`cull_route` + `services/search.py`'s
//! `group_similar_images`/`cull_images`, backed by
//! `lrg_analysis::grouping::group_and_sort_images`.

use std::sync::Arc;

use axum::extract::State;
use axum::response::{IntoResponse, Json, Response};
use serde_json::{json, Map, Value};

use lrg_analysis::culling_config::{available_presets, get_culling_config};
use lrg_analysis::grouping::{group_and_sort_images_with_config, Group, GroupingInput};
use lrg_ml::clip_iqa::PromptSet;
use lrg_store::IMAGE_TABLE;

use crate::state::AppState;

pub fn router() -> axum::Router<Arc<AppState>> {
    axum::Router::new()
        .route("/cull/groups", axum::routing::post(group_similar))
        .route("/cull/grade", axum::routing::post(cull))
}

struct GroupingParams {
    photo_ids: Vec<String>,
    phash_threshold: Option<f64>,
    clip_threshold: Option<f64>,
    /// `None` = not supplied; the culling preset's own burst window applies.
    time_delta_seconds: Option<i64>,
    culling_preset: String,
    /// Emit the per-group `debug` block (thresholds plus every pairwise
    /// distance in the group). Off unless asked for: it is O(k²) floats per
    /// group, the plugin never reads it, and on a large burst it dominates the
    /// response body.
    include_debug: bool,
    /// Score the aesthetic term with CLIP-IQA over the stored SigLIP2
    /// embeddings instead of the contrast/colourfulness heuristic alone.
    ///
    /// On by default. It exists as a switch because the first call in a server
    /// lifetime loads the text tower, and because the evaluation harness needs
    /// to be able to measure what the signal is actually worth by turning it
    /// off.
    use_iqa: bool,
    /// Echo each photo's stored `cull_*` metadata verbatim, alongside the
    /// ranked `metrics` block.
    ///
    /// The two are not the same thing and the difference matters. `metrics` is
    /// what ranking *concluded* — short names, derived values like
    /// `sharpness_effective`, and preset weights already folded in. This is
    /// what ranking *read*: the stored inputs, under the keys the store uses.
    /// Only the latter can reconstruct a run, which is what the evaluation
    /// fixture exporter needs. Off by default because it roughly doubles the
    /// per-photo payload and nothing in the normal cull flow reads it.
    include_stored_metadata: bool,
    /// Overrides the preset's `semantic_weight` for this request.
    ///
    /// The genre prompt sets are hypotheses, and the shipped weights are a
    /// guess made without a validated fixture. This exists so the weight can be
    /// swept from the outside — `0.0` to switch the axis off, `0.5` to see what
    /// it does when it leads — without a rebuild. It is the fastest way to find
    /// out whether the signal suits a particular photographer's eye.
    semantic_weight: Option<f64>,
}

/// Mirrors `_parse_grouping_params`. `Err` carries the (status, body) to
/// return directly.
#[allow(clippy::result_large_err)] // error path only; not worth boxing for this
fn parse_grouping_params(data: &Value) -> Result<GroupingParams, Response> {
    let bad = |msg: &str| {
        Err((
            axum::http::StatusCode::BAD_REQUEST,
            Json(json!({"error": msg})),
        )
            .into_response())
    };

    let phash_threshold = match data.get("phash_threshold") {
        None => None,
        Some(Value::String(s)) if s == "auto" => None,
        Some(v) => match v
            .as_f64()
            .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
        {
            Some(n) => Some(n),
            None => return bad("Invalid phash_threshold value"),
        },
    };

    let clip_threshold = match data.get("clip_threshold") {
        None => None,
        Some(Value::String(s)) if s == "auto" => None,
        Some(v) => match v
            .as_f64()
            .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
        {
            Some(n) => Some(n),
            None => return bad("Invalid clip_threshold value"),
        },
    };

    // Absent means "let the preset decide" rather than the old hardcoded 1 —
    // that constant is what made `event`/`sports`'s own burst windows dead.
    let time_delta_seconds = match data.get("time_delta_seconds") {
        None | Some(Value::Null) => None,
        Some(v) => match v
            .as_i64()
            .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
        {
            Some(n) => Some(n),
            None => return bad("Invalid time_delta_seconds value"),
        },
    };

    let culling_preset = data
        .get("culling_preset")
        .and_then(Value::as_str)
        .unwrap_or("default")
        .trim()
        .to_lowercase();
    let culling_preset = if culling_preset.is_empty() {
        "default".to_string()
    } else {
        culling_preset
    };
    if !available_presets().contains(&culling_preset.as_str()) {
        return Err((
            axum::http::StatusCode::BAD_REQUEST,
            Json(json!({"error": "Invalid culling_preset value", "available_presets": available_presets()})),
        )
            .into_response());
    }

    let photo_ids: Vec<String> = data
        .get("photo_ids")
        .or_else(|| data.get("uuids"))
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    if photo_ids.is_empty() {
        return bad("Missing or invalid 'photo_ids' list in request body");
    }

    let flag = |value: Option<&Value>, default: bool| -> bool {
        match value {
            None | Some(Value::Null) => default,
            Some(Value::Bool(b)) => *b,
            Some(Value::String(s)) => s.eq_ignore_ascii_case("true") || s == "1",
            Some(other) => other.as_i64().is_some_and(|n| n != 0),
        }
    };
    let include_debug = flag(
        data.get("include_debug").or_else(|| data.get("debug")),
        false,
    );
    let use_iqa = flag(data.get("use_iqa"), true);
    let include_stored_metadata = flag(data.get("include_stored_metadata"), false);

    // Clamped, not rejected: a weight outside 0..1 is a tuning fat-finger, and
    // the blend clamps anyway.
    let semantic_weight = match data.get("semantic_weight") {
        None | Some(Value::Null) => None,
        Some(v) => match v
            .as_f64()
            .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
        {
            Some(n) => Some(n.clamp(0.0, 1.0)),
            None => return bad("Invalid semantic_weight value"),
        },
    };

    Ok(GroupingParams {
        photo_ids,
        phash_threshold,
        clip_threshold,
        time_delta_seconds,
        culling_preset,
        include_debug,
        use_iqa,
        include_stored_metadata,
        semantic_weight,
    })
}

async fn load_grouping_inputs(
    store: &lrg_store::Store,
    photo_ids: &[String],
) -> Result<Vec<GroupingInput>, String> {
    // Dedup while preserving order, matching the Python set-based unique pass.
    let mut seen = std::collections::HashSet::new();
    let unique: Vec<String> = photo_ids
        .iter()
        .filter(|id| seen.insert((*id).clone()))
        .cloned()
        .collect();

    let records = store
        .get(IMAGE_TABLE, &unique)
        .await
        .map_err(|e| e.to_string())?;
    Ok(records
        .into_iter()
        .map(|r| {
            let filename = r
                .metadata
                .get("filename")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let capture_time = r.metadata.get("capture_time").and_then(Value::as_f64);
            let phash = r
                .metadata
                .get("cull_phash")
                .or_else(|| r.metadata.get("phash"))
                .and_then(Value::as_str)
                .and_then(|s| u64::from_str_radix(s, 16).ok());
            GroupingInput {
                photo_id: r.id,
                filename,
                capture_time,
                embedding: r.vector,
                phash,
                metadata: r.metadata,
            }
        })
        .collect())
}

fn group_to_json(g: &Group, params: &GroupingParams, outcome: &GroupingOutcome) -> Value {
    let photos: Vec<Value> = g
        .photos
        .iter()
        .map(|p| {
            let mut photo = json!({
                "photo_id": p.photo_id,
                "rank": p.rank,
                "cull_score": p.cull_score,
                "winner": p.winner,
                "reject_candidate": p.reject_candidate,
                "reason_codes": p.reason_codes,
                "explanation": p.explanation,
                "metrics": Value::Object(p.metrics.clone()),
            });
            if let Some(stored) = outcome.stored_metadata.get(&p.photo_id) {
                photo["stored_metadata"] = Value::Object(stored.clone());
            }
            photo
        })
        .collect();
    let debug = if params.include_debug {
        let mut thresholds = Map::new();
        thresholds.insert("phash_hamming_threshold".into(), json!(g.thresholds.0));
        thresholds.insert("duplicate_distance".into(), json!(g.thresholds.1));
        thresholds.insert("burst_distance".into(), json!(g.thresholds.2));
        thresholds.insert(
            "duplicate_time_window_seconds".into(),
            json!(g.thresholds.3),
        );
        thresholds.insert("time_window_seconds".into(), json!(g.thresholds.4));
        json!({
            "culling_preset": params.culling_preset,
            "thresholds": thresholds,
            "pairwise_distances": g.pairwise_distances,
            "pairwise_phash_distances": g.pairwise_phash_distances,
            "edge_types": g.edge_types,
        })
    } else {
        Value::Null
    };

    json!({
        "group_id": g.group_id,
        "group_type": g.group_type,
        // `group_type` already carries "bracket"/"focus_stack"/"panorama", but
        // an older plugin only knows the three original values and would treat
        // an unfamiliar one as an ordinary group. `keep_all` is the field a
        // client should branch on: it says what to *do* rather than what the
        // group is, and it is absent-means-false for anything that predates it.
        "keep_all": g.keep_all(),
        "intentional_set": g.intentional_set.map(|k| k.as_str()),
        "group_size": g.group_size,
        "primary_photo_id": g.primary_photo_id,
        "photo_ids": g.photo_ids,
        "winner_photo_id": g.winner_photo_id,
        "alternate_photo_ids": g.alternate_photo_ids,
        "reject_candidate_photo_ids": g.reject_candidate_photo_ids,
        "photos": photos,
        "min_capture_time": g.min_capture_time,
        "max_capture_time": g.max_capture_time,
        "time_span_seconds": g.time_span_seconds,
        "debug": debug,
    })
}

/// What `compute_groups` produced, plus the facts the caller needs to decide
/// whether to warn the user.
struct GroupingOutcome {
    groups: Vec<Group>,
    /// Photo ids that had no row in `IMAGE_TABLE` at all — never indexed.
    /// `load_grouping_inputs` drops these silently, which used to make an
    /// unindexed folder look like an empty result with no explanation.
    missing_count: usize,
    /// Records that came back carrying a usable (non-zero) embedding.
    embedded_count: usize,
    /// The run asked the moment questions: the preset (or the
    /// `semantic_weight` override) weights the moment, and IQA is on.
    moment_asked: bool,
    /// Ranked photos that came back with no moment score at all — no
    /// embedding, as a rule. Ranking drops the moment for their whole group.
    moment_missing: usize,
    /// A prompt pass found embeddings to score but could not load the text
    /// tower, so every CLIP-IQA signal fell back or fell away.
    iqa_unavailable: bool,
    /// Per photo, the stored inputs ranking read. Empty unless
    /// `include_stored_metadata` was requested — building it clones a map per
    /// photo, which is pure waste on the normal cull path.
    stored_metadata: std::collections::HashMap<String, Map<String, Value>>,
}

/// The stored fields that reproduce a ranking run: every `cull_*` metric plus
/// the two non-`cull_` inputs ranking and set detection actually read.
///
/// An allow-list rather than the whole record, because the rest is captions,
/// keywords and provider names — none of it affects culling, all of it is
/// potentially identifying, and a fixture is meant to be shareable without
/// shipping anything about the photographs themselves.
fn stored_cull_fields(metadata: &Map<String, Value>) -> Map<String, Value> {
    metadata
        .iter()
        .filter(|(k, _)| {
            k.starts_with("cull_") || k.as_str() == "exposure_bias" || k.as_str() == "capture_time"
        })
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

/// How a prompt pass over one batch went.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PromptScoring {
    /// This many photos were scored; the rest had no embedding.
    Scored(usize),
    /// Nothing in the batch has an embedding, so nothing was asked.
    NoEmbeddings,
    /// There were embeddings to score, but the text tower would not load.
    Unavailable,
}

/// Scores every record that carries an embedding against one prompt set and
/// writes the result under `key`.
///
/// No failure here fails the cull: no model on disk, a text tower that will
/// not load, a photo indexed by the fast cull pass with no embedding. Ranking
/// has a fallback for each — the aesthetic heuristic, and for the moment simply
/// not applying the blend — so a missing score is a *worse* result, never a
/// broken one, and refusing to cull over an optional signal would be the wrong
/// trade. Worse is still not silent: the outcome comes back so
/// [`grouping_warnings`] can tell the user what the run lost.
fn apply_prompt_scores(
    state: &AppState,
    records: &mut [GroupingInput],
    set: lrg_ml::clip_iqa::PromptSet,
    key: &str,
) -> PromptScoring {
    if !records.iter().any(|r| {
        r.embedding
            .as_ref()
            .is_some_and(|v| v.iter().any(|x| *x != 0.0))
    }) {
        return PromptScoring::NoEmbeddings;
    }

    // Locked once for the whole batch rather than per record, which is why
    // this takes the guard instead of calling `score_prompt_set`.
    let mut cache = state.clip_iqa.lock().unwrap();
    let Some(prompts) = crate::routes::route_util::ensure_prompt_set(state, &mut cache, set) else {
        return PromptScoring::Unavailable;
    };

    let mut scored = 0usize;
    for record in records.iter_mut() {
        let Some(embedding) = record.embedding.as_ref() else {
            continue;
        };
        if let Some(score) = prompts.score(embedding) {
            record.metadata.insert(key.to_string(), json!(score));
            scored += 1;
        }
    }
    log::debug!(
        "CLIP-IQA {} scored {scored}/{} photo(s)",
        set.as_str(),
        records.len()
    );
    PromptScoring::Scored(scored)
}

async fn compute_groups(
    state: &AppState,
    params: &GroupingParams,
) -> Result<GroupingOutcome, String> {
    let Some(store) = state.store() else {
        return Ok(GroupingOutcome {
            groups: Vec::new(),
            missing_count: params.photo_ids.len(),
            embedded_count: 0,
            moment_asked: false,
            moment_missing: 0,
            iqa_unavailable: false,
            stored_metadata: Default::default(),
        });
    };
    let mut records = load_grouping_inputs(&store, &params.photo_ids).await?;

    let unique_requested: std::collections::HashSet<&String> = params.photo_ids.iter().collect();
    let missing_count = unique_requested.len().saturating_sub(records.len());
    let embedded_count = records
        .iter()
        .filter(|r| {
            r.embedding
                .as_ref()
                .is_some_and(|v| v.iter().any(|x| *x != 0.0))
        })
        .count();

    // The request's `semantic_weight` overrides the preset's for the ranking
    // itself, not just for deciding whether to score the axis — before, a
    // sweep scored the prompts and then ranked on the preset's own weight.
    let mut cfg = get_culling_config(&params.culling_preset);
    if let Some(weight) = params.semantic_weight {
        cfg.ranking.semantic_weight = weight;
    }
    let ranking = &cfg.ranking;

    // The moment is judged only where the preset weights it. Each question is
    // a text-tower call and a set of dot products, and asking a landscape
    // whether anyone is cheering produces a number that is then discarded.
    let moment_sets: Vec<(&'static str, &'static str)> = if ranking.semantic_weight > 0.0 {
        [
            (ranking.semantic_prompt_set, "cull_semantic_iqa"),
            (ranking.emotion_prompt_set, "cull_emotion_iqa"),
        ]
        .into_iter()
        .filter_map(|(name, key)| name.map(|n| (n, key)))
        .collect()
    } else {
        Vec::new()
    };
    let moment_asked = params.use_iqa && !moment_sets.is_empty();
    let mut iqa_unavailable = false;

    // Before grouping, because ranking runs inside it.
    if params.use_iqa {
        let mut passes = vec![(PromptSet::Quality, "cull_aesthetic_iqa")];
        for (name, key) in &moment_sets {
            match PromptSet::from_name(name) {
                Some(set) => passes.push((set, key)),
                // A config typo. Log rather than fail: the rest of the ranking
                // is unaffected, a cull that refuses to run over it helps
                // nobody, and the preset tests exist to catch it first.
                None => log::warn!(
                    "culling preset {:?} names unknown prompt set {name:?}; skipping that signal",
                    params.culling_preset
                ),
            }
        }
        // The kiss gate is asked whenever the preset sets a threshold, moment
        // or not: it decides whether closed eyes count as a blink, which the
        // ranking reads either way.
        if ranking.eyes_closed_intent_threshold > 0.0 {
            passes.push((PromptSet::EyesClosedIntent, "cull_eyes_closed_intent_iqa"));
        }
        for (set, key) in passes {
            if apply_prompt_scores(state, &mut records, set, key) == PromptScoring::Unavailable {
                iqa_unavailable = true;
            }
        }
    }

    // Counted on what ranking will see, so the warning describes the run that
    // happened rather than the store.
    let moment_missing = if moment_asked {
        records
            .iter()
            .filter(|r| {
                !r.metadata.contains_key("cull_semantic_iqa")
                    && !r.metadata.contains_key("cull_emotion_iqa")
            })
            .count()
    } else {
        0
    };

    // Captured after the IQA passes and before grouping consumes the records,
    // so it reflects exactly the inputs this run ranked on — including the
    // injected `cull_*_iqa` scores, which are not stored
    // in the database and would otherwise be unreproducible from a fixture.
    let stored_metadata = if params.include_stored_metadata {
        records
            .iter()
            .map(|r| (r.photo_id.clone(), stored_cull_fields(&r.metadata)))
            .collect()
    } else {
        Default::default()
    };

    Ok(GroupingOutcome {
        stored_metadata,
        groups: group_and_sort_images_with_config(
            records,
            params.phash_threshold,
            params.clip_threshold,
            params.time_delta_seconds,
            &cfg,
        ),
        missing_count,
        embedded_count,
        moment_asked,
        moment_missing,
        iqa_unavailable,
    })
}

/// The user-facing caveats for a grouping run, most fundamental first; empty
/// when there is nothing worth saying.
///
/// This deliberately inspects the *data*, not the model. It used to test
/// `siglip.status() != "loaded"`, i.e. whether the model happened to be
/// resident in RAM — but SigLIP idle-unloads after 30 minutes, so any cull run
/// on an idle server told the user visual grouping was disabled while it was in
/// fact working perfectly from embeddings already in the database.
///
/// A list, not one slot: an unindexed photo and an unjudged moment are two
/// problems with two different fixes, and the second must not erase the first.
fn grouping_warnings(outcome: &GroupingOutcome) -> Vec<String> {
    const MOMENT: &str = "the moment (emotion, expression, action)";
    let mut warnings = Vec::new();
    let considered = outcome.embedded_count + outcome.missing_count;
    if outcome.missing_count > 0 && considered == outcome.missing_count {
        warnings.push(format!(
            "None of the {} selected photo(s) have been analyzed yet, so there is nothing to \
             group. Run 'Analyze & Index Photos' on them first.",
            outcome.missing_count
        ));
        return warnings;
    }
    if outcome.missing_count > 0 {
        warnings.push(format!(
            "{} selected photo(s) have not been analyzed yet and were skipped. Run \
             'Analyze & Index Photos' on them to include them.",
            outcome.missing_count
        ));
    }
    if outcome.embedded_count == 0 {
        warnings.push(if outcome.moment_asked {
            format!(
                "No image analysis is stored for these photos, so grouping used perceptual \
                 hashes and capture time only, and {MOMENT} was not judged. Run 'Cull Similar \
                 Photos' again and choose 'Prepare now'."
            )
        } else {
            "No visual embeddings are stored for these photos, so grouping used perceptual \
             hashes and capture time only. Re-run 'Analyze & Index Photos' with embeddings \
             enabled for content-aware grouping."
                .to_string()
        });
        return warnings;
    }
    if outcome.iqa_unavailable {
        warnings.push(if outcome.moment_asked {
            format!(
                "The image model could not be loaded, so {MOMENT} was not judged and photos \
                 were ranked on sharpness, exposure and faces only. Run \"Download AI models\" \
                 in Plug-in Manager, then cull again."
            )
        } else {
            "The image model could not be loaded, so the aesthetic impression was judged \
             with a simpler fallback. Run \"Download AI models\" in Plug-in Manager, then \
             cull again."
                .to_string()
        });
    } else if outcome.moment_missing > 0 {
        warnings.push(format!(
            "{} photo(s) have no image analysis yet, so {MOMENT} was not judged for them or \
             for the photos grouped with them. Run 'Cull Similar Photos' again and choose \
             'Prepare now' to include it.",
            outcome.moment_missing
        ));
    }
    warnings
}

/// The legacy single-string `warning`, for a plugin that predates `warnings`:
/// every message, so an older client loses none of them either.
fn joined_warning(warnings: &[String]) -> Option<String> {
    (!warnings.is_empty()).then(|| warnings.join("\n\n"))
}

async fn group_similar(State(state): State<Arc<AppState>>, body: Option<Json<Value>>) -> Response {
    let Some(Json(data)) = body else {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            Json(json!({"error": "Request must be JSON"})),
        )
            .into_response();
    };
    let params = match parse_grouping_params(&data) {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    match compute_groups(&state, &params).await {
        Ok(outcome) => {
            let warnings = grouping_warnings(&outcome);
            let json_groups: Vec<Value> = outcome
                .groups
                .iter()
                .map(|g| group_to_json(g, &params, &outcome))
                .collect();
            let mut response = json!({"groups": json_groups});
            if let Some(w) = joined_warning(&warnings) {
                response["warning"] = json!(w);
                response["warnings"] = json!(warnings);
            }
            Json(response).into_response()
        }
        Err(e) => {
            log::error!("Error during similarity grouping: {e}");
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e})),
            )
                .into_response()
        }
    }
}

async fn cull(State(state): State<Arc<AppState>>, body: Option<Json<Value>>) -> Response {
    let Some(Json(data)) = body else {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            Json(json!({"error": "Request must be JSON"})),
        )
            .into_response();
    };
    let params = match parse_grouping_params(&data) {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    match compute_groups(&state, &params).await {
        Ok(outcome) => {
            let warnings = grouping_warnings(&outcome);
            let (mut picks, mut alternates, mut rejects, mut near_dup_groups) =
                (0i64, 0i64, 0i64, 0i64);
            let (mut set_groups, mut set_photos) = (0i64, 0i64);
            for g in &outcome.groups {
                if g.group_type == "near_duplicate" {
                    near_dup_groups += 1;
                }
                if g.keep_all() {
                    set_groups += 1;
                    set_photos += g.photo_ids.len() as i64;
                }
                for p in &g.photos {
                    if p.winner {
                        picks += 1;
                    } else if p.reject_candidate {
                        rejects += 1;
                    } else {
                        alternates += 1;
                    }
                }
            }
            let json_groups: Vec<Value> = outcome
                .groups
                .iter()
                .map(|g| group_to_json(g, &params, &outcome))
                .collect();
            Json(json!({
                "status": "success",
                "warning": joined_warning(&warnings),
                "warnings": warnings,
                "summary": {
                    "group_count": outcome.groups.len(),
                    "pick_count": picks,
                    "alternate_count": alternates,
                    "reject_candidate_count": rejects,
                    "near_duplicate_group_count": near_dup_groups,
                    // Brackets, focus stacks and panoramas: groups where every
                    // frame is wanted. Surfaced separately so the plugin can
                    // tell the user *why* a chunk of their selection produced
                    // no reject candidates.
                    "intentional_set_group_count": set_groups,
                    "intentional_set_photo_count": set_photos,
                    "culling_preset": params.culling_preset,
                    // Surfaced so the plugin can tell the user that some of
                    // their selection was skipped rather than silently ranking
                    // fewer photos than they picked.
                    "unindexed_count": outcome.missing_count,
                },
                "groups": json_groups,
            }))
            .into_response()
        }
        Err(e) => {
            log::error!("Error during culling: {e}");
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e})),
            )
                .into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outcome(embedded: usize, missing: usize) -> GroupingOutcome {
        GroupingOutcome {
            groups: Vec::new(),
            missing_count: missing,
            embedded_count: embedded,
            moment_asked: true,
            moment_missing: 0,
            iqa_unavailable: false,
            stored_metadata: Default::default(),
        }
    }

    #[test]
    fn a_clean_run_says_nothing() {
        assert!(grouping_warnings(&outcome(10, 0)).is_empty());
        assert_eq!(joined_warning(&[]), None);
    }

    /// Two problems, two fixes: the second must not erase the first.
    #[test]
    fn skipped_photos_and_an_unjudged_moment_both_reach_the_user() {
        let mut o = outcome(10, 2);
        o.moment_missing = 3;
        let warnings = grouping_warnings(&o);
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        assert!(warnings[0].contains("2 selected photo(s) have not been analyzed"));
        assert!(warnings[1].starts_with("3 photo(s) have no image analysis"));
        assert!(warnings[1].contains("Prepare now"));
        let joined = joined_warning(&warnings).unwrap();
        assert!(joined.contains(&warnings[0]) && joined.contains(&warnings[1]));
    }

    /// No embeddings at all: one message covering both grouping and the moment,
    /// not two that tell the user the same fix twice.
    #[test]
    fn no_embeddings_is_one_message_for_a_moment_preset() {
        let warnings = grouping_warnings(&outcome(0, 0));
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("perceptual hashes") && warnings[0].contains("moment"));

        let mut o = outcome(0, 0);
        o.moment_asked = false;
        let warnings = grouping_warnings(&o);
        assert!(!warnings[0].contains("moment"), "{warnings:?}");
    }

    #[test]
    fn an_unloadable_model_names_the_fix() {
        let mut o = outcome(10, 0);
        o.iqa_unavailable = true;
        o.moment_missing = 10;
        let warnings = grouping_warnings(&o);
        assert_eq!(
            warnings.len(),
            1,
            "the model is the one cause: {warnings:?}"
        );
        assert!(warnings[0].contains("Download AI models") && warnings[0].contains("moment"));

        o.moment_asked = false;
        let warnings = grouping_warnings(&o);
        assert!(warnings[0].contains("aesthetic"), "{warnings:?}");
    }
}
