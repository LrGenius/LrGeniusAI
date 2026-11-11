//! `/v1/cull/groups` and `/v1/cull/grade` — port of `routes/search.py`'s
//! `group_similar_route`/`cull_route` + `services/search.py`'s
//! `group_similar_images`/`cull_images`, backed by
//! `lrg_analysis::grouping::group_and_sort_images`.

use std::sync::Arc;

use axum::extract::State;
use axum::response::{IntoResponse, Json, Response};
use serde_json::{json, Map, Value};

use lrg_analysis::culling_config::{
    available_presets, get_culling_config, preset_for_genres, CullingConfig, AUTO_PRESET,
};
use lrg_analysis::grouping::{partition_records, Group, GroupingInput};
use lrg_ml::clip_iqa::PromptSet;
use lrg_store::{meta, IMAGE_TABLE};

use crate::prompt_embeddings::PromptEmbeddings;
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
    /// On by default. It exists as a switch because the evaluation harness
    /// needs to be able to measure what the signal is actually worth by
    /// turning it off. (It used to matter for cost too, when the first call in
    /// a server lifetime loaded the text tower; the prompt vectors now come
    /// out of the database.)
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
    /// Echo each photo's stored SigLIP2 embedding, packed by
    /// [`lrg_analysis::eval::quantize_embedding`], as `embedding_q8`.
    ///
    /// For fixtures too. The grouper reads the embedding, so a fixture without
    /// one is grouped on pHash and capture time alone when it is replayed, and
    /// a frame two stops brighter, or a soft one, then lands in a group of its
    /// own and is scored against nothing. About 1.5 KB per photo, which is why
    /// it is a flag of its own rather than part of `include_stored_metadata`.
    include_embeddings: bool,
    /// Overrides the preset's `semantic_weight` for this request.
    ///
    /// The genre prompt sets are hypotheses, and the shipped weights are a
    /// guess made without a validated fixture. This exists so the weight can be
    /// swept from the outside — `0.0` to switch the axis off, `0.5` to see what
    /// it does when it leads — without a rebuild. It is the fastest way to find
    /// out whether the signal suits a particular photographer's eye.
    semantic_weight: Option<f64>,
    /// Scales the preset's `reject_score_delta` — how far below the pick a
    /// frame's score must fall before it is a reject candidate.
    ///
    /// How hard to cull is the photographer's call, not the preset's: the same
    /// wedding is culled to one frame per burst for a client gallery and to
    /// three for an album edit. Above 1 fewer frames are nominated (a wider
    /// margin), below 1 more. It moves only the score-margin rule; the
    /// defect rules (blur, blink, occlusion, exposure) fire at their usual
    /// thresholds, because a blink is a blink however gentle the cull.
    /// Clamped to 0.25..4.
    reject_margin_scale: Option<f64>,
    /// Judge each group by its key people's faces: the people named in People
    /// and those in many of the selected photos (`crate::key_people`). Needs
    /// the full face pass; off by default.
    key_people: bool,
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
    let include_embeddings = flag(data.get("include_embeddings"), false);
    let key_people = flag(data.get("key_people"), false);

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

    // Clamped like `semantic_weight`: a factor of 0 would reject every frame
    // but the pick, and anything past 4 is indistinguishable from "never".
    let reject_margin_scale = match data.get("reject_margin_scale") {
        None | Some(Value::Null) => None,
        Some(v) => match v
            .as_f64()
            .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
        {
            Some(n) if n.is_finite() => Some(n.clamp(0.25, 4.0)),
            _ => return bad("Invalid reject_margin_scale value"),
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
        include_embeddings,
        semantic_weight,
        reject_margin_scale,
        key_people,
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
            if let Some(packed) = outcome.embeddings.get(&p.photo_id) {
                photo["embedding_q8"] = json!(packed);
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
            "culling_preset": g.preset.unwrap_or(&params.culling_preset),
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
        // The preset this group was ranked with: the request's, or under
        // `auto` the one its photos' genre called for.
        "preset": g.preset.unwrap_or(&params.culling_preset),
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
    /// Ranked photos with no aesthetic score from the current export (never
    /// scored, or scored by another export): their aesthetic term rests on the
    /// contrast/colour heuristic alone.
    aesthetic_missing: usize,
    /// Whether the photo quality model is on disk, which decides what the
    /// warning about missing scores tells the user to do.
    aesthetic_model_ready: bool,
    /// Ranked photos with faces that were prepared before eye and face focus
    /// were measured, when the preset judges focus. Ranking leaves focus out
    /// of their whole group.
    ///
    /// Photos whose faces were measured but too small to judge are *not*
    /// counted: nothing the user can do changes that, and their explanation
    /// already shows no eye-focus reading. Saying so on every cull of an
    /// event full of small faces would only teach people to skip the dialog.
    focus_unmeasured: usize,
    /// Ranked photos with faces whose eyes were not analysed by the face state
    /// model; their group falls back to the rough eye estimate.
    state_unmeasured: usize,
    /// Whether that model is on disk, which decides the fix the warning names.
    face_state_model_ready: bool,
    /// Ranked photos that did not go through the current subject pass, when
    /// the preset judges focus on the subject. Their group is ranked on
    /// frame-wide sharpness, as before the pass existed.
    subject_unmeasured: usize,
    /// Whether the subject model is on disk, which decides the fix.
    subject_model_ready: bool,
    /// Under `culling_preset: "auto"`, ranked photos with no genre: their
    /// group's preset was chosen from the others, or is `default`.
    genre_missing: usize,
    /// What `key_people` found, when it was asked for.
    key_people: Option<KeyPeopleOutcome>,
    /// CLIP-IQA signals the run asked for and had to do without, because
    /// their prompt vectors were neither stored nor obtainable from the text
    /// tower.
    unavailable: Option<UnavailableSignals>,
    /// Per photo, the stored inputs ranking read. Empty unless
    /// `include_stored_metadata` was requested — building it clones a map per
    /// photo, which is pure waste on the normal cull path.
    stored_metadata: std::collections::HashMap<String, Map<String, Value>>,
    /// Per photo with a non-zero embedding, the embedding packed for a
    /// fixture. Empty unless `include_embeddings` was requested.
    embeddings: std::collections::HashMap<String, String>,
}

/// What judging by key people found.
struct KeyPeopleOutcome {
    /// The key people with a name in People.
    named: Vec<String>,
    /// And how many without one.
    unnamed: usize,
    /// Groups judged by their key people's faces.
    groups: usize,
    /// Photos with face rows, and photos with faces never through face
    /// recognition.
    recognised: usize,
    unrecognised: usize,
}

/// The ranking signals a run lost, and the one reason they share.
struct UnavailableSignals {
    /// What each lost signal is, phrased for the user, each once.
    signals: Vec<&'static str>,
    /// [`PromptEmbeddings::problem`]: why the prompts are missing, which
    /// already names the fix.
    why: String,
}

/// The user-facing names of the signals a prompt pass feeds.
const MOMENT_SIGNAL: &str = "the AI judgement of the moment (emotion, expression, action)";
const KISS_GATE_SIGNAL: &str = "the check whether closed eyes are a kiss or a laugh";

/// The stored fields that reproduce a ranking run: every `cull_*` metric plus
/// the non-`cull_` inputs ranking, set detection and `auto`'s choice of preset
/// actually read.
///
/// An allow-list rather than the whole record, because the rest is captions,
/// keywords and provider names — none of it affects culling, all of it is
/// potentially identifying, and a fixture is meant to be shareable without
/// shipping anything about the photographs themselves. The genre is one coarse
/// word ("sport", "portrait"); the names key people put on reasons
/// (`cull_who_*`) are left out.
fn stored_cull_fields(metadata: &Map<String, Value>) -> Map<String, Value> {
    metadata
        .iter()
        .filter(|(k, _)| {
            (k.starts_with("cull_") && !k.starts_with("cull_who_"))
                || k.as_str() == "exposure_bias"
                || k.as_str() == "capture_time"
                || k.as_str() == meta::GENRE_FIELD
        })
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

/// Scores every record that carries an embedding and that `asks` against one
/// prompt set, and writes the result under `key`.
///
/// No failure here fails the cull. Per photo, one that cannot be scored (no
/// embedding — the fast cull pass stores none) is skipped: ranking has a
/// fallback for it, the aesthetic heuristic and for the moment simply not
/// applying the blend, and [`grouping_warnings`] counts the photos whose moment
/// went unjudged. A set that is unavailable altogether is the caller's to
/// report.
///
/// Returns how many photos were scored, or `None` when the set itself is
/// unavailable.
fn apply_prompt_scores(
    prompts: &PromptEmbeddings,
    records: &mut [GroupingInput],
    asks: impl Fn(usize) -> bool,
    set: PromptSet,
    key: &str,
) -> Option<usize> {
    let prompts = prompts.prompt_set(set)?;
    let mut scored = 0usize;
    for (_, record) in records.iter_mut().enumerate().filter(|(i, _)| asks(*i)) {
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
    Some(scored)
}

/// Scales a stored AestheticSigLIP score to the `0..1` ranking works in.
///
/// The model's score runs 1 to 9 (its first and last bucket centers), so
/// `(s - 1) / 8` fills the unit interval. The ranking only compares photos
/// within a group and reads two absolute bars (`reason_low_aesthetic_threshold`,
/// 0.35-0.42), which on this scale sit at model scores of 3.8-4.4: the bottom of
/// the model's own "bad framing, low effort" band.
fn aesthetic_unit(score: f64) -> f64 {
    ((score - lrg_ml::aesthetic::SCORE_MIN as f64)
        / (lrg_ml::aesthetic::SCORE_MAX as f64 - lrg_ml::aesthetic::SCORE_MIN as f64))
        .clamp(0.0, 1.0)
}

/// Writes `cull_aesthetic_model` onto every record that carries a score from
/// the *current* export, and returns how many do not.
///
/// A score from another export is not comparable with this one's (different
/// weights, different calibration), so it counts as missing rather than being
/// mixed in. Needs no embedding and no text tower, unlike the prompt passes: it
/// is a stored number, which is also why a cull-only catalog has it.
fn apply_aesthetic_scores(records: &mut [GroupingInput]) -> usize {
    let mut missing = 0usize;
    for record in records.iter_mut() {
        let current = record
            .metadata
            .get(meta::AESTHETIC_MODEL_FIELD)
            .and_then(Value::as_str)
            == Some(lrg_ml::aesthetic::MODEL_ID);
        let score = record
            .metadata
            .get(meta::AESTHETIC_SCORE_FIELD)
            .and_then(Value::as_f64)
            .filter(|_| current);
        match score {
            Some(score) => {
                record
                    .metadata
                    .insert("cull_aesthetic_model".into(), json!(aesthetic_unit(score)));
            }
            None => missing += 1,
        }
    }
    missing
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
            aesthetic_missing: 0,
            aesthetic_model_ready: state.aesthetic.is_cached(),
            focus_unmeasured: 0,
            state_unmeasured: 0,
            face_state_model_ready: state.face_state.is_cached(),
            subject_unmeasured: 0,
            subject_model_ready: state.subject.is_cached(),
            genre_missing: 0,
            key_people: None,
            unavailable: None,
            stored_metadata: Default::default(),
            embeddings: Default::default(),
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
    // The photographer's cull strength is applied on top of the preset's own
    // margin, so every preset keeps its relative calibration.
    let config_of = |preset: &str| -> CullingConfig {
        let mut cfg = get_culling_config(preset);
        if let Some(weight) = params.semantic_weight {
            cfg.ranking.semantic_weight = weight;
        }
        if let Some(scale) = params.reject_margin_scale {
            cfg.ranking.reject_score_delta *= scale;
        }
        cfg
    };
    let genre_of = |r: &GroupingInput| -> Option<String> {
        r.metadata
            .get(meta::GENRE_FIELD)
            .and_then(Value::as_str)
            .map(str::to_string)
    };

    // `auto` groups with the preset most of the selection calls for (its
    // burst window and distances), then ranks each group with the preset most
    // of *its* photos call for. Any other preset is one config for both.
    let auto = params.culling_preset == AUTO_PRESET;
    let requested: &'static str = available_presets()
        .into_iter()
        .find(|p| *p == params.culling_preset)
        .unwrap_or("default");
    let genres: Vec<Option<String>> = records.iter().map(genre_of).collect();
    let genre_missing = if auto {
        genres.iter().filter(|g| g.is_none()).count()
    } else {
        0
    };
    let grouping_preset = if auto {
        preset_for_genres(genres.iter().flatten().map(String::as_str)).unwrap_or("default")
    } else {
        requested
    };

    // The aesthetic term comes from the score indexing stored, not from a
    // prompt pass, so it is not behind `use_iqa` or an embedding.
    let aesthetic_missing = apply_aesthetic_scores(&mut records);

    let mut partition = partition_records(
        records,
        params.phash_threshold,
        params.clip_threshold,
        params.time_delta_seconds,
        &config_of(grouping_preset),
    );
    let key_people = if params.key_people {
        let names = crate::routes::faces::load_person_names(state);
        let key = crate::key_people::load(&store, partition.records(), names).await?;
        let groups = crate::key_people::apply(&mut partition, &key);
        Some(KeyPeopleOutcome {
            named: key.named(),
            unnamed: key.unnamed(),
            groups,
            recognised: key.recognised(),
            unrecognised: key.unrecognised,
        })
    } else {
        None
    };
    let group_presets: Vec<&'static str> = (0..partition.group_count())
        .map(|g| {
            if auto {
                let members: Vec<Option<String>> = partition.members(g).map(genre_of).collect();
                preset_for_genres(members.iter().flatten().map(String::as_str)).unwrap_or("default")
            } else {
                requested
            }
        })
        .collect();
    let mut configs: Vec<(&'static str, CullingConfig)> = Vec::new();
    for &preset in &group_presets {
        if !configs.iter().any(|(p, _)| *p == preset) {
            configs.push((preset, config_of(preset)));
        }
    }
    let config = |preset: &str| -> CullingConfig {
        configs
            .iter()
            .find(|(p, _)| *p == preset)
            .map(|(_, c)| *c)
            .unwrap_or_else(|| config_of(preset))
    };
    // Each photo's preset, aligned with `partition.records()`.
    let photo_presets: Vec<&'static str> = partition
        .group_of()
        .iter()
        .map(|&g| group_presets[g])
        .collect();

    // The moment is judged only where the preset weights it. Asking a
    // landscape whether anyone is cheering produces a number that is then
    // discarded — and its prompts are not worth preparing either.
    let moment_sets = |cfg: &CullingConfig| -> Vec<(&'static str, &'static str)> {
        if cfg.ranking.semantic_weight > 0.0 {
            [
                (cfg.ranking.semantic_prompt_set, "cull_semantic_iqa"),
                (cfg.ranking.emotion_prompt_set, "cull_emotion_iqa"),
            ]
            .into_iter()
            .filter_map(|(name, key)| name.map(|n| (n, key)))
            .collect()
        } else {
            Vec::new()
        }
    };
    let asks_moment = |preset: &str| params.use_iqa && !moment_sets(&config(preset)).is_empty();
    let moment_asked = configs.iter().any(|(p, _)| asks_moment(p));
    let mut unavailable = None;

    // Before ranking, which reads them. Only when some photo can be scored:
    // preparing the prompts may mean a text-tower run the first time ever,
    // which is not worth paying for a run with nothing to score.
    if params.use_iqa && embedded_count > 0 {
        let mut passes: Vec<(&'static str, PromptSet, &str, &'static str)> = Vec::new();
        for (preset, cfg) in &configs {
            for (name, key) in moment_sets(cfg) {
                match PromptSet::from_name(name) {
                    Some(set) => passes.push((preset, set, key, MOMENT_SIGNAL)),
                    // A config typo. Log rather than fail: the rest of the
                    // ranking is unaffected, a cull that refuses to run over it
                    // helps nobody, and the preset tests exist to catch it
                    // first.
                    None => log::warn!(
                        "culling preset {preset:?} names unknown prompt set {name:?}; skipping \
                         that signal"
                    ),
                }
            }
            // The kiss gate is asked whenever the preset sets a threshold,
            // moment or not: it decides whether closed eyes count as a blink,
            // which the ranking reads either way.
            if cfg.ranking.eyes_closed_intent_threshold > 0.0 {
                passes.push((
                    preset,
                    PromptSet::EyesClosedIntent,
                    "cull_eyes_closed_intent_iqa",
                    KISS_GATE_SIGNAL,
                ));
            }
        }

        // One preparation for every set the run asks, so a first run that
        // has to embed them pays for a single text-tower pass. Nothing to
        // prepare for a preset that asks no question of the prompts.
        let mut sets: Vec<PromptSet> = Vec::new();
        for (_, set, _, _) in &passes {
            if !sets.contains(set) {
                sets.push(*set);
            }
        }
        let prompts =
            crate::prompt_embeddings::prepare(state, &crate::prompt_embeddings::texts_of(&sets))
                .await;
        let mut lost: Vec<&'static str> = Vec::new();
        for (preset, set, key, signal) in passes {
            let asks = |i: usize| photo_presets[i] == preset;
            if apply_prompt_scores(&prompts, partition.records_mut(), asks, set, key).is_none()
                && !lost.contains(&signal)
            {
                lost.push(signal);
            }
        }
        if !lost.is_empty() {
            unavailable = Some(UnavailableSignals {
                signals: lost,
                why: prompts
                    .problem()
                    .unwrap_or("their prompts could not be prepared")
                    .to_string(),
            });
        }
    }

    // Counted on what ranking will see, each photo by the preset of its group,
    // so the warnings describe the run that happened rather than the store.
    let records = partition.records();
    let counted = |test: &dyn Fn(&GroupingInput, &CullingConfig) -> bool| -> usize {
        records
            .iter()
            .zip(&photo_presets)
            .filter(|(r, p)| test(r, &config(p)))
            .count()
    };
    let has_faces = |r: &GroupingInput| {
        r.metadata
            .get("cull_face_count")
            .and_then(Value::as_i64)
            .unwrap_or(0)
            > 0
    };
    let moment_missing = counted(&|r, cfg| {
        params.use_iqa
            && !moment_sets(cfg).is_empty()
            && !r.metadata.contains_key("cull_semantic_iqa")
            && !r.metadata.contains_key("cull_emotion_iqa")
    });
    let focus_unmeasured = counted(&|r, cfg| {
        cfg.ranking.judges_focus()
            && has_faces(r)
            && !r
                .metadata
                .contains_key(lrg_analysis::grouping::FOCUS_VERSION_KEY)
    });
    // Every preset reads the eyes (the blink rules), so this is counted
    // whatever the preset.
    let state_unmeasured = counted(&|r, _| {
        has_faces(r) && !lrg_analysis::grouping::face_state_is_current(&r.metadata)
    });
    // Only where the preset reads the subject at all; counted whether or not
    // the model is there, since the fix differs and both are said.
    let subject_unmeasured = counted(&|r, cfg| {
        (cfg.ranking.subject_focus_weight > 0.0
            || cfg.ranking.reason_subject_focus_threshold > 0.0
            || cfg.ranking.reject_subject_focus_threshold > 0.0)
            && !lrg_analysis::grouping::subject_is_current(&r.metadata)
    });

    // Captured after the IQA passes and before ranking consumes the records,
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
    let embeddings = if params.include_embeddings {
        records
            .iter()
            .filter_map(|r| {
                let v = r.embedding.as_ref()?;
                v.iter().any(|x| *x != 0.0).then(|| {
                    (
                        r.photo_id.clone(),
                        lrg_analysis::eval::quantize_embedding(v),
                    )
                })
            })
            .collect()
    } else {
        Default::default()
    };

    let groups = partition.rank(|g| {
        let preset = group_presets[g];
        (config(preset), auto.then_some(preset))
    });
    Ok(GroupingOutcome {
        stored_metadata,
        embeddings,
        groups,
        missing_count,
        embedded_count,
        moment_asked,
        moment_missing,
        aesthetic_missing,
        aesthetic_model_ready: state.aesthetic.is_cached(),
        focus_unmeasured,
        state_unmeasured,
        face_state_model_ready: state.face_state.is_cached(),
        subject_unmeasured,
        subject_model_ready: state.subject.is_cached(),
        genre_missing,
        key_people,
        unavailable,
    })
}

/// `a`, `a and b`, `a, b and c`.
fn join_as_prose(items: &[&str]) -> String {
    match items {
        [] => String::new(),
        [only] => only.to_string(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
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
/// One cause is still said once: signals lost to the same missing model share
/// one message rather than one each.
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
    // Independent of embeddings: the score is stored, so a cull-only catalog
    // has it and a catalog indexed before scoring existed does not.
    if outcome.aesthetic_missing > 0 {
        warnings.push(if outcome.aesthetic_model_ready {
            format!(
                "{} photo(s) have no aesthetic score from the current AI model yet, so their \
                 aesthetic term rests on a contrast and colour heuristic, which can rank them \
                 unevenly against scored photos in the same group. Run 'Analyze & Index \
                 Photos' on them to score them.",
                outcome.aesthetic_missing
            )
        } else {
            format!(
                "{} photo(s) have no aesthetic score because the photo quality model is not \
                 downloaded yet, so their aesthetic term rests on a contrast and colour \
                 heuristic. Run \"Download AI models\" in Plug-in Manager, then 'Analyze & Index \
                 Photos' on them.",
                outcome.aesthetic_missing
            )
        });
    }
    // Also independent of embeddings: focus and the face state come from the
    // face pass. One fix ("Prepare now") is said once, naming what it brings;
    // a missing model is a different fix and a message of its own.
    let state_needs_prep = outcome.face_state_model_ready && outcome.state_unmeasured > 0;
    let prep_count = outcome.focus_unmeasured.max(if state_needs_prep {
        outcome.state_unmeasured
    } else {
        0
    });
    if prep_count > 0 {
        let what = match (outcome.focus_unmeasured > 0, state_needs_prep) {
            (true, true) => "eye focus and eye state",
            (true, false) => "eye and face focus",
            _ => "eye state, smile and gaze",
        };
        warnings.push(format!(
            "{prep_count} photo(s) with faces were prepared before {what} were measured, so \
             they and the photos grouped with them were ranked without it. Run 'Cull Similar \
             Photos' again and choose 'Prepare now' to measure it."
        ));
    }
    if !outcome.face_state_model_ready && outcome.state_unmeasured > 0 {
        warnings.push(format!(
            "Eye state, smile and gaze were not analysed for {} photo(s) with faces because \
             the eye and expression model is not downloaded yet, so a rough eye estimate \
             decided blinks. Run \"Download AI models\" in Plug-in Manager, then 'Cull \
             Similar Photos' and 'Prepare now'.",
            outcome.state_unmeasured
        ));
    }
    if let Some(key) = &outcome.key_people {
        if key.unrecognised > 0 {
            warnings.push(format!(
                "{} photo(s) with faces have not been through face recognition, so the key \
                 people could not be found in them. Run 'Cull Similar Photos' again with key \
                 people on and choose 'Prepare now'.",
                key.unrecognised
            ));
        }
        // Only where some photos could be told apart: with none, the warning
        // above is the cause.
        if key.recognised > 0 && key.named.is_empty() && key.unnamed == 0 {
            warnings.push(
                "No one is in enough of these photos to be a key person, and no one in them is \
                 named in People, so faces were judged as usual. Name the people who matter in \
                 People to have them judged first."
                    .to_string(),
            );
        }
    }
    if outcome.genre_missing > 0 {
        warnings.push(format!(
            "{} photo(s) have no genre yet, so the preset for their groups was chosen from the \
             other photos in them, or was Default where none had one. Run 'Cull Similar Photos' \
             again and choose 'Prepare now' to read their genre.",
            outcome.genre_missing
        ));
    }
    if outcome.subject_unmeasured > 0 {
        warnings.push(if outcome.subject_model_ready {
            format!(
                "{} photo(s) were prepared before their subject was found, so they and the \
                 photos grouped with them were judged on frame-wide sharpness, which can take \
                 focus on the background for a sharp photo. Run 'Cull Similar Photos' again \
                 and choose 'Prepare now' to measure focus on the subject.",
                outcome.subject_unmeasured
            )
        } else {
            format!(
                "Focus on the subject was not measured for {} photo(s) because the subject \
                 model is not downloaded yet, so sharpness was judged on the whole frame. Run \
                 \"Download AI models\" in Plug-in Manager, then 'Cull Similar Photos' and \
                 'Prepare now'.",
                outcome.subject_unmeasured
            )
        });
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
    let moment_lost = outcome
        .unavailable
        .as_ref()
        .is_some_and(|u| u.signals.contains(&MOMENT_SIGNAL));
    if let Some(u) = &outcome.unavailable {
        warnings.push(format!(
            "Ranking ran without {} because {}. Picks rest on the remaining measurements \
             (sharpness, exposure, faces) alone.",
            join_as_prose(&u.signals),
            u.why
        ));
    }
    // Every photo misses the moment when its prompts are missing; that is the
    // message above, not a second one about photos to prepare.
    if !moment_lost && outcome.moment_missing > 0 {
        warnings.push(format!(
            "{} photo(s) have no image analysis yet, so {MOMENT} was not judged for them or \
             for the photos grouped with them. Run 'Cull Similar Photos' again and choose \
             'Prepare now' to include it.",
            outcome.moment_missing
        ));
    }
    warnings
}

/// Groups per preset, for groups ranked with a preset of their own (`auto`);
/// empty otherwise.
fn preset_counts(groups: &[Group]) -> Map<String, Value> {
    let mut counts = Map::new();
    for preset in groups.iter().filter_map(|g| g.preset) {
        let n = counts.get(preset).and_then(Value::as_u64).unwrap_or(0);
        counts.insert(preset.to_string(), json!(n + 1));
    }
    counts
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
            let mut response = json!({"groups": json_groups, "warnings": warnings});
            if let Some(w) = joined_warning(&warnings) {
                response["warning"] = json!(w);
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
                    // Under `auto`, how many groups each preset ranked, so the
                    // plugin can say what the genre chose.
                    "presets": preset_counts(&outcome.groups),
                    // When asked: who the key people were, and how many groups
                    // were judged by them.
                    "key_people": outcome.key_people.as_ref().map(|k| json!({
                        "named": k.named,
                        "unnamed": k.unnamed,
                        "groups": k.groups,
                    })),
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
            aesthetic_missing: 0,
            aesthetic_model_ready: true,
            focus_unmeasured: 0,
            state_unmeasured: 0,
            face_state_model_ready: true,
            subject_unmeasured: 0,
            subject_model_ready: true,
            genre_missing: 0,
            key_people: None,
            unavailable: None,
            stored_metadata: Default::default(),
            embeddings: Default::default(),
        }
    }

    /// Key people asked for: photos never through face recognition are named
    /// with the fix, and a selection with nobody to judge by says so.
    #[test]
    fn key_people_say_what_they_could_not_do() {
        let mut o = outcome(10, 0);
        o.key_people = Some(KeyPeopleOutcome {
            named: Vec::new(),
            unnamed: 0,
            groups: 0,
            recognised: 6,
            unrecognised: 4,
        });
        let warnings = grouping_warnings(&o);
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        assert!(warnings[0].starts_with("4 photo(s) with faces"));
        assert!(warnings[0].contains("Prepare now"));
        assert!(warnings[1].contains("No one is in enough"));

        // None recognised at all: the cause is said, not "no one counts".
        o.key_people = Some(KeyPeopleOutcome {
            named: Vec::new(),
            unnamed: 0,
            groups: 0,
            recognised: 0,
            unrecognised: 10,
        });
        assert_eq!(grouping_warnings(&o).len(), 1);

        o.key_people = Some(KeyPeopleOutcome {
            named: vec!["Anna".into()],
            unnamed: 1,
            groups: 3,
            recognised: 10,
            unrecognised: 0,
        });
        assert!(grouping_warnings(&o).is_empty());
    }

    /// Photos prepared before focus was measured are ranked without it, and
    /// the fix is one the user can take: prepare them again.
    #[test]
    fn photos_prepared_before_focus_existed_are_named_with_the_fix() {
        let mut o = outcome(10, 0);
        o.focus_unmeasured = 3;
        let warnings = grouping_warnings(&o);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].starts_with("3 photo(s) with faces"));
        assert!(warnings[0].contains("Prepare now"));
        // Said even when grouping fell back to hashes: a different problem.
        let mut o = outcome(0, 0);
        o.focus_unmeasured = 2;
        assert_eq!(grouping_warnings(&o).len(), 2);
    }

    /// Photos prepared before the subject pass are ranked on frame-wide
    /// sharpness; the fix is preparing them, or first downloading the model.
    #[test]
    fn photos_without_a_subject_reading_are_named_with_the_fix() {
        let mut o = outcome(10, 0);
        o.subject_unmeasured = 4;
        let warnings = grouping_warnings(&o);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].starts_with("4 photo(s) were prepared before their subject"));
        assert!(warnings[0].contains("Prepare now"));
        o.subject_model_ready = false;
        let warnings = grouping_warnings(&o);
        assert!(warnings[0].contains("Download AI models"), "{warnings:?}");
    }

    /// Old preparation and a missing model are two problems with two fixes;
    /// two signals with the same fix are one message.
    #[test]
    fn face_state_warnings_name_the_fix_that_applies() {
        let mut o = outcome(10, 0);
        o.state_unmeasured = 4;
        let w = grouping_warnings(&o);
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].contains("eye state, smile and gaze") && w[0].contains("Prepare now"));

        o.focus_unmeasured = 4;
        let w = grouping_warnings(&o);
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].contains("eye focus and eye state"));

        o.face_state_model_ready = false;
        let w = grouping_warnings(&o);
        assert_eq!(w.len(), 2, "{w:?}");
        assert!(w[1].contains("Download AI models"));
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

    /// Signals lost to the one missing model: one message naming each of them
    /// and the fix, not one per signal — and no second one telling the user
    /// to prepare photos whose analysis is not what is missing.
    #[test]
    fn unavailable_prompts_are_one_message_naming_the_fix() {
        let mut o = outcome(10, 0);
        o.unavailable = Some(UnavailableSignals {
            signals: vec![KISS_GATE_SIGNAL, MOMENT_SIGNAL],
            why: "the AI search model is not downloaded yet — run \"Download AI models\" in \
                  Plug-in Manager"
                .into(),
        });
        o.moment_missing = 10;
        let warnings = grouping_warnings(&o);
        assert_eq!(
            warnings.len(),
            1,
            "the model is the one cause: {warnings:?}"
        );
        assert!(
            warnings[0].contains("a kiss or a laugh and the AI judgement of the moment")
                && warnings[0].contains("Download AI models"),
            "{warnings:?}"
        );

        // Only the kiss check missing — the moment's prompts came from the
        // database — leaves the unprepared photos their own message.
        o.unavailable.as_mut().unwrap().signals = vec![KISS_GATE_SIGNAL];
        o.moment_missing = 2;
        let warnings = grouping_warnings(&o);
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        assert!(!warnings[0].contains("moment"), "{warnings:?}");
        assert!(warnings[1].starts_with("2 photo(s) have no image analysis"));
    }

    /// Photos without a current aesthetic score are said once, with the fix that
    /// fits: score them, or download the model first.
    #[test]
    fn photos_without_an_aesthetic_score_are_reported_with_the_right_fix() {
        let mut o = outcome(10, 0);
        o.aesthetic_missing = 4;
        let warnings = grouping_warnings(&o);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].starts_with("4 photo(s) have no aesthetic score"));
        assert!(warnings[0].contains("Analyze & Index"), "{warnings:?}");
        assert!(!warnings[0].contains("Download AI models"), "{warnings:?}");

        o.aesthetic_model_ready = false;
        let warnings = grouping_warnings(&o);
        assert!(warnings[0].contains("not downloaded yet"), "{warnings:?}");
        assert!(warnings[0].contains("Download AI models"), "{warnings:?}");
    }

    /// The score is stored data, so the warning is not hidden by a catalog with
    /// no embeddings — a cull-only one has scores and a pre-scoring one does not.
    #[test]
    fn the_aesthetic_warning_survives_a_catalog_without_embeddings() {
        let mut o = outcome(0, 0);
        o.aesthetic_missing = 3;
        let warnings = grouping_warnings(&o);
        assert!(
            warnings.iter().any(|w| w.contains("no aesthetic score")),
            "{warnings:?}"
        );
    }

    fn record_with(metadata: Value) -> GroupingInput {
        GroupingInput {
            photo_id: "p".into(),
            filename: "p.jpg".into(),
            capture_time: None,
            phash: None,
            embedding: None,
            metadata: metadata.as_object().cloned().unwrap_or_default(),
        }
    }

    #[test]
    fn the_model_score_is_scaled_to_the_unit_interval() {
        assert_eq!(aesthetic_unit(1.0), 0.0);
        assert_eq!(aesthetic_unit(9.0), 1.0);
        assert_eq!(aesthetic_unit(5.0), 0.5);
        // Outside what the model can produce: clamped, never negative or above 1.
        assert_eq!(aesthetic_unit(0.0), 0.0);
        assert_eq!(aesthetic_unit(12.0), 1.0);
    }

    /// Only a score from the current export is used; another export's is not
    /// comparable and counts as missing, like one that was never made.
    #[test]
    fn only_a_current_aesthetic_score_reaches_the_ranking() {
        let current = lrg_ml::aesthetic::MODEL_ID;
        let mut records = vec![
            record_with(json!({"aesthetic_score": 7.0, "aesthetic_model": current})),
            record_with(json!({"aesthetic_score": 7.0, "aesthetic_model": "another-export"})),
            record_with(json!({"aesthetic_score": 7.0})),
            record_with(json!({})),
        ];
        let missing = apply_aesthetic_scores(&mut records);
        assert_eq!(missing, 3);
        assert_eq!(records[0].metadata["cull_aesthetic_model"], 0.75);
        for r in &records[1..] {
            assert!(!r.metadata.contains_key("cull_aesthetic_model"));
        }
    }

    #[test]
    fn lost_signals_are_listed_as_prose() {
        assert_eq!(join_as_prose(&["a"]), "a");
        assert_eq!(join_as_prose(&["a", "b"]), "a and b");
        assert_eq!(join_as_prose(&["a", "b", "c"]), "a, b and c");
    }
}
