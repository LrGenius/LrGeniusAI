//! Faces: detection, similarity search, person clustering, and the person
//! editing the People page does.
//!
//! The clustering *policy* — quality gating, chaining resistance, and the rule
//! that a user's manual assignment outranks anything the algorithm believes —
//! lives in [`lrg_analysis::face_cluster`]. This module is the store side of
//! it: which rows go in, and what the answer means for `FACE_TABLE`.
//!
//! Two metadata fields on a face row carry a person:
//!
//! - `person_id` — who this face belongs to. Empty or `person_unassigned`
//!   means nobody.
//! - `person_manual` — true when a human said so, in the People page. It is
//!   the difference between a guess a re-cluster may revise and a fact it must
//!   preserve, and every editing endpoint here sets it.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use axum::{
    extract::{Path as UrlPath, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post, put},
    Json, Router,
};
use base64::Engine;
use lrg_imaging::cull_config::FaceMetricsConfig;
use lrg_ml::faces::FacePass;
use serde::Deserialize;
use serde_json::{json, Map, Value};

use lrg_analysis::face_cluster::{self, ClusterParams, FaceInput, QualityGate};
use lrg_analysis::{clustering, persons};
use lrg_store::{StoreRecord, FACE_TABLE};

use crate::state::AppState;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/faces/detect", post(detect))
        .route("/faces/search", post(query))
        .route("/faces/cluster", post(cluster))
        .route("/faces/assign", post(assign_faces))
        .route("/faces/persons", get(list_persons))
        .route("/faces/persons/merge", post(merge_persons))
        .route(
            "/faces/persons/{person_id}/thumbnail",
            get(person_thumbnail),
        )
        .route("/faces/persons/{person_id}", put(set_person_name))
        .route("/faces/persons/{person_id}/photos", get(person_photos))
        .route("/faces/persons/{person_id}/faces", get(person_faces))
}

#[allow(clippy::result_large_err)] // error path only; not worth boxing for this
fn decode_image(body: &Value) -> Result<(Vec<u8>, usize, usize), Response> {
    let b64 = body.get("image").and_then(Value::as_str).ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "Missing 'image' (base64) in JSON body"})),
        )
            .into_response()
    })?;
    let raw = base64::engine::general_purpose::STANDARD
        .decode(b64)
        .map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": format!("Invalid base64 image: {e}")})),
            )
                .into_response()
        })?;
    let decoded = image::load_from_memory(&raw)
        .map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": format!("Invalid base64 image: {e}")})),
            )
                .into_response()
        })?
        .to_rgb8();
    let (w, h) = (decoded.width() as usize, decoded.height() as usize);
    Ok((decoded.into_raw(), w, h))
}

async fn detect(State(state): State<Arc<AppState>>, body: Option<Json<Value>>) -> Response {
    log::info!("Faces detect request received");
    let data = body.map(|Json(v)| v).unwrap_or(Value::Null);
    let (pixels, w, h) = match decode_image(&data) {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    match state.face.detect_faces(
        &pixels,
        w,
        h,
        &FaceMetricsConfig::defaults(),
        FacePass::Full,
    ) {
        Ok(faces) => {
            let result: Vec<Value> = faces
                .iter()
                .enumerate()
                .map(|(i, f)| json!({"thumbnail": f.thumbnail_base64, "index": i}))
                .collect();
            Json(json!({"status": "ok", "faces": result})).into_response()
        }
        Err(e) => {
            log::error!("Face detection failed: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
                .into_response()
        }
    }
}

async fn query(State(state): State<Arc<AppState>>, body: Option<Json<Value>>) -> Response {
    log::info!("Faces query request received");
    let data = body.map(|Json(v)| v).unwrap_or(Value::Null);
    let (pixels, w, h) = match decode_image(&data) {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    let n_results = data.get("n_results").and_then(Value::as_u64).unwrap_or(10) as usize;
    let face_index = data.get("face_index").and_then(Value::as_u64).unwrap_or(0) as usize;

    let faces = match state.face.detect_faces(
        &pixels,
        w,
        h,
        &FaceMetricsConfig::defaults(),
        FacePass::Full,
    ) {
        Ok(f) => f,
        Err(e) => {
            log::error!("Face detection failed: {e}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
                .into_response();
        }
    };
    if faces.is_empty() {
        return Json(json!({"status": "no_face", "results": []})).into_response();
    }
    if face_index >= faces.len() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": format!("face_index must be 0..{}", faces.len() - 1)})),
        )
            .into_response();
    }
    let query_emb = &faces[face_index].embedding;

    let Some(store) = state.store() else {
        return Json(json!({"status": "ok", "results": []})).into_response();
    };
    let records = match store.scan_all(FACE_TABLE).await {
        Ok(r) => r,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
                .into_response()
        }
    };

    let mut scored: Vec<(f64, &StoreRecord)> = records
        .iter()
        .filter_map(|r| {
            let v = r.vector.as_ref()?;
            Some((cosine_distance(query_emb, v), r))
        })
        .collect();
    scored.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    scored.truncate(n_results);

    let results: Vec<Value> = scored
        .into_iter()
        .map(|(dist, r)| {
            let photo_id = r
                .metadata
                .get("photo_id")
                .or_else(|| r.metadata.get("photo_uuid"))
                .cloned()
                .unwrap_or(Value::Null);
            json!({
                "face_id": r.id,
                "photo_id": photo_id,
                "photo_uuid": r.metadata.get("photo_uuid").or_else(|| r.metadata.get("photo_id")).cloned().unwrap_or(Value::Null),
                "thumbnail": r.metadata.get("thumbnail").cloned().unwrap_or(json!("")),
                "person_id": r.metadata.get("person_id").cloned().unwrap_or(json!("")),
                "distance": dist,
            })
        })
        .collect();
    Json(json!({"status": "ok", "results": results})).into_response()
}

fn cosine_distance(a: &[f32], b: &[f32]) -> f64 {
    let dot: f64 = a.iter().zip(b).map(|(x, y)| *x as f64 * *y as f64).sum();
    let na: f64 = a
        .iter()
        .map(|x| (*x as f64) * (*x as f64))
        .sum::<f64>()
        .sqrt();
    let nb: f64 = b
        .iter()
        .map(|x| (*x as f64) * (*x as f64))
        .sum::<f64>()
        .sqrt();
    if na <= 0.0 || nb <= 0.0 {
        1.0
    } else {
        1.0 - dot / (na * nb)
    }
}

fn person_names_path(state: &AppState) -> Option<std::path::PathBuf> {
    state.lance_root().map(|r| r.join("person_names.json"))
}

fn load_person_names(state: &AppState) -> HashMap<String, String> {
    let Some(path) = person_names_path(state) else {
        return HashMap::new();
    };
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str::<HashMap<String, String>>(&s).ok())
        .unwrap_or_default()
}

fn save_person_names(state: &AppState, names: &HashMap<String, String>) -> std::io::Result<()> {
    let Some(path) = person_names_path(state) else {
        return Ok(());
    };
    std::fs::write(&path, serde_json::to_string_pretty(names).unwrap())
}

/// The bucket every face without a person lands in, whatever spelling it
/// arrived with. Reported to the plugin as an empty `person_id`, and accepted
/// back in a URL path (which cannot carry an empty segment) under this name.
const UNASSIGNED_KEY: &str = "_unassigned";

/// The spellings of "no person this belongs to": the clusterer writes
/// `person_unassigned`, a row that has never been clustered has an empty
/// `person_id`, and a URL path says `_unassigned`.
fn is_unassigned(person_id: &str) -> bool {
    person_id.is_empty() || person_id == UNASSIGNED_KEY || person_id == "person_unassigned"
}

fn person_of(meta: &Map<String, Value>) -> &str {
    meta.get("person_id").and_then(Value::as_str).unwrap_or("")
}

fn is_manual(meta: &Map<String, Value>) -> bool {
    meta.get("person_manual")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn photo_of(meta: &Map<String, Value>) -> &str {
    meta.get("photo_id")
        .or_else(|| meta.get("photo_uuid"))
        .and_then(Value::as_str)
        .unwrap_or("")
}

fn number_at(meta: &Map<String, Value>, key: &str) -> f64 {
    meta.get(key).and_then(Value::as_f64).unwrap_or(0.0)
}

/// The same, but keeping "the row does not have this" distinct from "the row
/// says zero". The quality gate needs the difference: a face indexed before
/// these measurements existed must not be judged as if every one of them came
/// back at its worst.
fn measured_at(meta: &Map<String, Value>, key: &str) -> Option<f64> {
    meta.get(key).and_then(Value::as_f64)
}

/// The next free `person_N`, given every id already in use.
fn next_person_index<'a>(existing: impl Iterator<Item = &'a str>) -> i64 {
    let mut max_idx = -1i64;
    for id in existing {
        if let Some(rest) = id.strip_prefix("person_") {
            if let Ok(n) = rest.parse::<i64>() {
                max_idx = max_idx.max(n);
            }
        }
    }
    max_idx + 1
}

// ---------------------------------------------------------------- clustering

/// Overridable knobs for one clustering run. Everything has a default in
/// [`ClusterParams`]; the People page sends the threshold and, when the user
/// opens the quality controls, the gate.
#[derive(Debug, Deserialize, Default)]
struct ClusterRequest {
    distance_threshold: Option<f64>,
    min_faces_per_person: Option<serde_json::Value>,
    linkage: Option<String>,
    /// `"dbscan"` still reaches the old density-based path. It is kept because
    /// it is occasionally the right tool on a catalog of near-duplicates, but
    /// it chains, so it is no longer the default.
    algorithm: Option<String>,
    min_det_score: Option<f64>,
    min_area_ratio: Option<f64>,
    min_sharpness: Option<f64>,
    max_occlusion: Option<f64>,
    attach_factor: Option<f64>,
    /// When false, faces the user assigned by hand are re-clustered like any
    /// other. Off by default — undoing someone's corrections needs asking for.
    ignore_manual: Option<bool>,
}

async fn cluster(State(state): State<Arc<AppState>>, body: Option<Json<Value>>) -> Response {
    log::info!("Faces cluster request received");
    let raw = body.map(|Json(v)| v).unwrap_or_else(|| json!({}));
    let request: ClusterRequest = match serde_json::from_value(raw) {
        Ok(r) => r,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": format!("Invalid clustering options: {e}")})),
            )
                .into_response()
        }
    };

    let defaults = ClusterParams::defaults();
    let gate_defaults = QualityGate::defaults();
    let params = ClusterParams {
        cosine_threshold: request
            .distance_threshold
            .unwrap_or(defaults.cosine_threshold),
        // `null` used to mean "no minimum" and picked a different algorithm.
        // It now means what it says: keep every cluster, singletons included.
        min_faces_per_person: match &request.min_faces_per_person {
            None => defaults.min_faces_per_person,
            Some(Value::Null) => 1,
            Some(v) => v
                .as_i64()
                .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
                .unwrap_or(defaults.min_faces_per_person as i64)
                .max(1) as usize,
        },
        linkage: if request.linkage.as_deref() == Some("average") {
            clustering::Linkage::Average
        } else {
            clustering::Linkage::Complete
        },
        gate: QualityGate {
            min_det_score: request.min_det_score.unwrap_or(gate_defaults.min_det_score),
            min_area_ratio: request
                .min_area_ratio
                .unwrap_or(gate_defaults.min_area_ratio),
            min_sharpness: request.min_sharpness.unwrap_or(gate_defaults.min_sharpness),
            max_occlusion: request.max_occlusion.unwrap_or(gate_defaults.max_occlusion),
        },
        attach_factor: request.attach_factor.unwrap_or(defaults.attach_factor),
    };
    let use_dbscan = request.algorithm.as_deref() == Some("dbscan");
    let honour_manual = !request.ignore_manual.unwrap_or(false);

    let empty_summary = json!({
        "status": "ok", "person_count": 0, "face_count": 0, "updated": 0, "unassigned": 0,
        "warnings": [],
    });
    let Some(store) = state.store() else {
        return Json(empty_summary).into_response();
    };

    let result: Result<Value, String> = async {
        let records = store
            .scan_all(FACE_TABLE)
            .await
            .map_err(|e| e.to_string())?;
        if records.is_empty() {
            return Ok(empty_summary.clone());
        }
        let total = records.len();
        let mut warnings: Vec<String> = Vec::new();

        // A row with no vector — or one of the wrong width — used to become an
        // empty `Vec` here, and the distance functions zip vectors of unequal
        // length, so its distance to everything came out as 0 and it merged
        // whole clusters together. Such rows are set aside and reported as
        // unassigned instead.
        //
        // Rows from a superseded model are set aside the same way, and this is
        // the case the width check cannot catch: the retired ArcFace
        // embeddings are 512-d unit vectors exactly like FaceNet's, so mixing
        // them produces no error at all, just clusters built from distances
        // between two unrelated spaces. Photos in this state are re-queued for
        // detection by `/v1/index/unprocessed`; until that runs, leaving
        // their faces unassigned is the honest answer.
        let current_model = state.face.model_id();
        let expected_dim = records
            .iter()
            .filter_map(|r| r.vector.as_ref().map(Vec::len))
            .find(|&len| len > 0);
        let (records, unclusterable): (Vec<StoreRecord>, Vec<StoreRecord>) =
            records.into_iter().partition(|r| {
                r.vector
                    .as_ref()
                    .is_some_and(|v| Some(v.len()) == expected_dim)
                    && r.metadata.get("face_model").and_then(Value::as_str) == Some(current_model)
            });
        if !unclusterable.is_empty() {
            let stale = unclusterable
                .iter()
                .filter(|r| {
                    r.metadata.get("face_model").and_then(Value::as_str) != Some(current_model)
                })
                .count();
            let message = format!(
                "{} of {total} faces could not be clustered ({stale} were detected by an older \
                 face model, {} have no usable embedding) and are listed under Unassigned. \
                 Re-index those photos to bring them back.",
                unclusterable.len(),
                unclusterable.len() - stale,
            );
            log::warn!("Face clustering: {message}");
            warnings.push(message);
        }

        let inputs: Vec<FaceInput> = records
            .iter()
            .map(|r| FaceInput {
                id: r.id.clone(),
                embedding: r.vector.clone().unwrap_or_default(),
                det_score: measured_at(&r.metadata, "face_det_score"),
                area_ratio: measured_at(&r.metadata, "face_area_ratio"),
                sharpness: measured_at(&r.metadata, "face_sharpness"),
                occlusion: measured_at(&r.metadata, "face_occlusion"),
                pinned: (honour_manual && is_manual(&r.metadata)).then(|| {
                    let pid = person_of(&r.metadata);
                    if is_unassigned(pid) {
                        String::new()
                    } else {
                        pid.to_string()
                    }
                }),
            })
            .collect();

        let outcome = if use_dbscan {
            dbscan_outcome(&inputs, &params)
        } else {
            face_cluster::cluster_faces(&inputs, &params)
        };
        warnings.extend(outcome.stats.warnings.iter().cloned());

        // Clusters the user pinned already know who they are. The rest go
        // through overlap matching against the previous run so that a person
        // who did not change keeps their id (and therefore their name).
        let mut old_person_faces: HashMap<String, HashSet<String>> = HashMap::new();
        for r in &records {
            let pid = person_of(&r.metadata);
            if !is_unassigned(pid) {
                old_person_faces
                    .entry(pid.to_string())
                    .or_default()
                    .insert(r.id.clone());
            }
        }
        let mut cluster_person: Vec<Option<String>> = outcome
            .clusters
            .iter()
            .map(|c| c.pinned_person.clone())
            .collect();
        // Pinned ids stay in the pool as empty sets: overlap is then always
        // zero so nothing else can claim them, while `next_person_index` still
        // sees them and cannot hand out an id that is already taken.
        for person in cluster_person.iter().flatten() {
            old_person_faces.insert(person.clone(), HashSet::new());
        }

        let mut new_label_faces: HashMap<i64, HashSet<String>> = HashMap::new();
        for (i, label) in outcome.labels.iter().enumerate() {
            if let Some(c) = label {
                if cluster_person[*c].is_none() {
                    new_label_faces
                        .entry(*c as i64)
                        .or_default()
                        .insert(inputs[i].id.clone());
                }
            }
        }
        let label_to_person = persons::match_labels_to_persons(&old_person_faces, &new_label_faces);
        for (label, person) in &label_to_person {
            cluster_person[*label as usize] = Some(person.clone());
        }

        let mut unassigned_count = 0usize;
        let mut updated: Vec<StoreRecord> = Vec::with_capacity(total);
        for record in unclusterable {
            unassigned_count += 1;
            let mut metadata = record.metadata;
            metadata.insert("person_id".into(), json!("person_unassigned"));
            updated.push(StoreRecord {
                id: record.id,
                vector: record.vector,
                metadata,
            });
        }
        for (i, record) in records.into_iter().enumerate() {
            let person_id = match outcome.labels[i].and_then(|c| cluster_person[c].clone()) {
                Some(pid) => pid,
                None => {
                    unassigned_count += 1;
                    "person_unassigned".to_string()
                }
            };
            let mut metadata = record.metadata;
            metadata.insert("person_id".into(), Value::String(person_id));
            updated.push(StoreRecord {
                id: record.id,
                vector: record.vector,
                metadata,
            });
        }
        store
            .upsert(FACE_TABLE, &updated)
            .await
            .map_err(|e| e.to_string())?;

        let stats = &outcome.stats;
        // A person built only out of faces that could not pass the quality gate
        // is exactly the kind of nonsense the gate exists to prevent, so say
        // when the gate is doing a lot of work.
        if stats.low_quality * 3 > stats.total && stats.total > 0 {
            warnings.push(format!(
                "{} of {} faces were too small, blurry or hidden to help decide who is who, so \
                 they were only attached to people found from the sharper ones. Indexing at a \
                 larger preview size, or loosening the quality limits here, would use more of them.",
                stats.low_quality, stats.total,
            ));
        }

        if stats.unmeasured > 0 {
            warnings.push(format!(
                "{} faces were indexed before this version measured face quality, so they helped \
                 decide who is who without being checked. Re-index those photos to get the \
                 clustering the quality limits are meant to give.",
                stats.unmeasured,
            ));
        }

        let tight: Vec<&face_cluster::ClusterInfo> = outcome
            .clusters
            .iter()
            .filter(|c| c.nearest_other.is_finite() && c.nearest_other < params.cosine_threshold * 1.2)
            .collect();
        let person_count = cluster_person.iter().flatten().count()
            + usize::from(unassigned_count > 0);
        Ok(json!({
            "status": "ok",
            "person_count": person_count,
            "face_count": total,
            "updated": total,
            "unassigned": unassigned_count,
            "warnings": warnings,
            "diagnostics": {
                "threshold": params.cosine_threshold,
                "algorithm": if use_dbscan { "dbscan" } else { "complete-linkage" },
                "clusters": stats.cluster_count,
                "pinned_faces": stats.pinned_faces,
                "low_quality": stats.low_quality,
                "unmeasured": stats.unmeasured,
                "attached": stats.attached,
                "dropped_small": stats.dropped_small,
                "canopies": stats.canopies,
                "mean_spread": mean_spread(&outcome.clusters),
                // How many people ended up close enough to each other that a
                // slightly higher threshold would have merged them. This is
                // the number to watch when deciding whether the threshold is
                // reasonable.
                "near_duplicates": tight.len(),
            },
        }))
    }
    .await;

    match result {
        Ok(v) => Json(v).into_response(),
        Err(e) => {
            log::error!("Face clustering failed: {e}");
            (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e}))).into_response()
        }
    }
}

fn mean_spread(clusters: &[face_cluster::ClusterInfo]) -> f64 {
    if clusters.is_empty() {
        return 0.0;
    }
    let total: f64 = clusters.iter().map(|c| c.spread * c.size as f64).sum();
    let faces: usize = clusters.iter().map(|c| c.size).sum();
    if faces == 0 {
        0.0
    } else {
        total / faces as f64
    }
}

/// The pre-existing density-based path, reachable with `"algorithm": "dbscan"`.
///
/// Kept behind the same [`ClusterOutcome`](face_cluster::ClusterOutcome) shape
/// so the caller has one code path. It honours neither the quality gate nor
/// pinned faces — that is the point of it being opt-in.
fn dbscan_outcome(inputs: &[FaceInput], params: &ClusterParams) -> face_cluster::ClusterOutcome {
    let embeddings: Vec<Vec<f32>> = inputs.iter().map(|f| f.embedding.clone()).collect();
    let eps = clustering::cosine_to_l2(params.cosine_threshold);
    let labels = clustering::dbscan(&embeddings, eps, params.min_faces_per_person.max(2));
    let cluster_count = labels
        .iter()
        .copied()
        .max()
        .map_or(0, |m| (m + 1).max(0) as usize);
    let mut sizes = vec![0usize; cluster_count];
    for &l in &labels {
        if l >= 0 {
            sizes[l as usize] += 1;
        }
    }
    face_cluster::ClusterOutcome {
        labels: labels
            .iter()
            .map(|&l| if l < 0 { None } else { Some(l as usize) })
            .collect(),
        clusters: sizes
            .into_iter()
            .map(|size| face_cluster::ClusterInfo {
                pinned_person: None,
                size,
                spread: f64::NAN,
                nearest_other: f64::INFINITY,
            })
            .collect(),
        stats: face_cluster::ClusterStats {
            total: inputs.len(),
            cluster_count,
            canopies: 1,
            unassigned: labels.iter().filter(|&&l| l < 0).count(),
            warnings: vec![
                "DBSCAN was used because the request asked for it. It joins people through \
                 chains of borderline faces, so different people can end up as one — the \
                 default algorithm does not do that."
                    .to_string(),
            ],
            ..Default::default()
        },
    }
}

// ------------------------------------------------------------------ listings

async fn list_persons(State(state): State<Arc<AppState>>) -> Response {
    log::info!("List persons request received");
    let Some(store) = state.store() else {
        return Json(json!({"status": "ok", "persons": []})).into_response();
    };
    let rows = match store.scan_meta(FACE_TABLE).await {
        Ok(r) => r,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
                .into_response()
        }
    };
    let names = load_person_names(&state);

    #[derive(Default)]
    struct Info {
        face_count: usize,
        manual_count: usize,
        photo_ids: HashSet<String>,
        /// One representative face thumbnail, picked here because this scan
        /// already has every row in hand. The plugin used to fetch these one
        /// person at a time from `/faces/persons/{id}/thumbnail`, and each of
        /// those calls scanned the whole FACE_TABLE — base64-decoding every
        /// thumbnail in the catalog to return exactly one.
        thumbnail: Option<String>,
        /// The quality score of the face that thumbnail came from, so the card
        /// shows the person's best face rather than whichever row was first.
        thumbnail_quality: f64,
    }
    let mut by_person: HashMap<String, Info> = HashMap::new();
    for (_, meta) in &rows {
        let pid = person_of(meta);
        // One key for "no person", not two. The clusterer writes
        // `person_unassigned` while rows that were never clustered carry an
        // empty string, and bucketing those separately listed the same
        // non-person twice in the People dialog.
        let key = if is_unassigned(pid) {
            UNASSIGNED_KEY.to_string()
        } else {
            pid.to_string()
        };
        let entry = by_person.entry(key).or_default();
        entry.face_count += 1;
        if is_manual(meta) {
            entry.manual_count += 1;
        }
        entry.photo_ids.insert(photo_of(meta).to_string());
        let quality = number_at(meta, "face_det_score") + number_at(meta, "face_sharpness");
        if entry.thumbnail.is_none() || quality > entry.thumbnail_quality {
            if let Some(thumb) = meta
                .get("thumbnail")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
            {
                entry.thumbnail = Some(thumb.to_string());
                entry.thumbnail_quality = quality;
            }
        }
    }

    let mut entries: Vec<(String, Info)> = by_person.into_iter().collect();
    entries.sort_by(|(a_pid, a), (b_pid, b)| {
        let a_unassigned = is_unassigned(a_pid);
        let b_unassigned = is_unassigned(b_pid);
        a_unassigned
            .cmp(&b_unassigned)
            .then(b.photo_ids.len().cmp(&a.photo_ids.len()))
            .then(a_pid.cmp(b_pid))
    });

    let persons: Vec<Value> = entries
        .into_iter()
        .map(|(pid, info)| {
            let person_id = if pid == UNASSIGNED_KEY { String::new() } else { pid };
            json!({
                "person_id": person_id,
                "name": if person_id.is_empty() { String::new() } else { names.get(&person_id).cloned().unwrap_or_default() },
                "face_count": info.face_count,
                "photo_count": info.photo_ids.len(),
                "manual_count": info.manual_count,
                "thumbnail": info.thumbnail.unwrap_or_default(),
            })
        })
        .collect();
    Json(json!({"status": "ok", "persons": persons})).into_response()
}

/// Kept for compatibility with older plugin builds; `list_persons` now returns
/// the representative thumbnail inline, so nothing should call this in a loop.
async fn person_thumbnail(
    State(state): State<Arc<AppState>>,
    UrlPath(person_id): UrlPath<String>,
) -> Response {
    log::info!("Get person thumbnail request received for person_id={person_id}");
    let Some(store) = state.store() else {
        return Json(json!({"status": "ok", "person_id": person_id, "thumbnail": ""}))
            .into_response();
    };
    let thumb = match store.scan_meta(FACE_TABLE).await {
        Ok(rows) => rows
            .into_iter()
            .find(|(_, m)| person_of(m) == person_id)
            .and_then(|(_, m)| {
                m.get("thumbnail")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .unwrap_or_default(),
        Err(_) => String::new(),
    };
    Json(json!({"status": "ok", "person_id": person_id, "thumbnail": thumb})).into_response()
}

async fn set_person_name(
    State(state): State<Arc<AppState>>,
    UrlPath(person_id): UrlPath<String>,
    body: Option<Json<Value>>,
) -> Response {
    log::info!("Set person name request received for person_id={person_id}");
    let name = body
        .as_ref()
        .and_then(|Json(v)| v.get("name"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let mut names = load_person_names(&state);
    let trimmed = name.trim().to_string();
    if trimmed.is_empty() {
        names.remove(&person_id);
    } else {
        names.insert(person_id.clone(), trimmed.clone());
    }
    if let Err(e) = save_person_names(&state, &names) {
        log::error!("Set person name failed: {e}");
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        )
            .into_response();
    }
    Json(json!({"status": "ok", "person_id": person_id, "name": trimmed})).into_response()
}

async fn person_photos(
    State(state): State<Arc<AppState>>,
    UrlPath(person_id): UrlPath<String>,
) -> Response {
    log::info!("Get photos for person request received for person_id={person_id}");
    let Some(store) = state.store() else {
        return Json(
            json!({"status": "ok", "person_id": person_id, "photo_ids": [], "photo_uuids": []}),
        )
        .into_response();
    };
    let rows = match store.scan_meta(FACE_TABLE).await {
        Ok(r) => r,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
                .into_response()
        }
    };
    let wants_unassigned = is_unassigned(&person_id);
    let mut photo_ids: Vec<String> = rows
        .into_iter()
        .filter(|(_, m)| {
            if wants_unassigned {
                is_unassigned(person_of(m))
            } else {
                person_of(m) == person_id
            }
        })
        .map(|(_, m)| photo_of(&m).to_string())
        .filter(|id| !id.is_empty())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    photo_ids.sort();
    Json(json!({
        "status": "ok",
        "person_id": person_id,
        "photo_ids": photo_ids.clone(),
        "photo_uuids": photo_ids,
    }))
    .into_response()
}

// ------------------------------------------------------- the person detail view

#[derive(Debug, Deserialize)]
struct PersonFacesQuery {
    limit: Option<usize>,
    offset: Option<usize>,
    /// `typical` (closest to the person's average face first), `outlier` (the
    /// reverse — where wrong faces surface), or `quality`.
    sort: Option<String>,
}

/// How many faces one request will return. A face carries its 112x112 JPEG
/// thumbnail as base64, so an unbounded person would be a multi-megabyte
/// response; the page pages through instead.
const PERSON_FACES_MAX_LIMIT: usize = 500;

async fn person_faces(
    State(state): State<Arc<AppState>>,
    UrlPath(person_id): UrlPath<String>,
    Query(params): Query<PersonFacesQuery>,
) -> Response {
    log::info!("Get faces for person request received for person_id={person_id}");
    let limit = params
        .limit
        .unwrap_or(PERSON_FACES_MAX_LIMIT)
        .clamp(1, PERSON_FACES_MAX_LIMIT);
    let offset = params.offset.unwrap_or(0);
    let sort = params.sort.as_deref().unwrap_or("typical").to_string();

    let names = load_person_names(&state);
    let Some(store) = state.store() else {
        return Json(json!({
            "status": "ok", "person_id": person_id, "faces": [], "total": 0,
            "offset": offset, "limit": limit,
        }))
        .into_response();
    };

    let wants_unassigned = is_unassigned(&person_id);
    let rows = match store.scan_meta(FACE_TABLE).await {
        Ok(r) => r,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
                .into_response()
        }
    };
    let mut mine: Vec<(String, Map<String, Value>)> = rows
        .into_iter()
        .filter(|(_, m)| {
            if wants_unassigned {
                is_unassigned(person_of(m))
            } else {
                person_of(m) == person_id
            }
        })
        .collect();
    let total = mine.len();

    // Vectors for this person only — the whole point of not calling
    // `scan_all`, which would deserialize every embedding in the catalog to
    // answer a question about one person.
    let ids: Vec<String> = mine.iter().map(|(id, _)| id.clone()).collect();
    let vectors: HashMap<String, Vec<f32>> = match store.get(FACE_TABLE, &ids).await {
        Ok(records) => records
            .into_iter()
            .filter_map(|r| r.vector.map(|v| (r.id, v)))
            .collect(),
        Err(e) => {
            // Distances are a diagnostic, not the content: losing them should
            // not cost the user the face grid they asked for.
            log::warn!("Could not load embeddings for {person_id}: {e}");
            HashMap::new()
        }
    };

    // The person's average face, and each face's distance from it. For the
    // unassigned bucket there is no such thing — it is not a person — so
    // distances are simply absent there.
    let centroid = if wants_unassigned || vectors.is_empty() {
        Vec::new()
    } else {
        let mut sum = vec![0.0f64; vectors.values().next().map_or(0, Vec::len)];
        for v in vectors.values() {
            for (slot, x) in sum.iter_mut().zip(v.iter()) {
                *slot += *x as f64;
            }
        }
        let norm = sum.iter().map(|x| x * x).sum::<f64>().sqrt();
        if norm > 0.0 {
            sum.into_iter().map(|x| (x / norm) as f32).collect()
        } else {
            Vec::new()
        }
    };
    let distance_of = |id: &str| -> Option<f64> {
        let v = vectors.get(id)?;
        if centroid.is_empty() {
            return None;
        }
        Some(cosine_distance(v, &centroid))
    };

    let gate = QualityGate::defaults();
    let quality_score = |m: &Map<String, Value>| {
        0.4 * number_at(m, "face_det_score")
            + 0.3 * number_at(m, "face_sharpness")
            + 0.2 * (1.0 - number_at(m, "face_occlusion"))
            + 0.1 * (number_at(m, "face_area_ratio") / 0.12).clamp(0.0, 1.0)
    };
    match sort.as_str() {
        "quality" => mine.sort_by(|(_, a), (_, b)| {
            quality_score(b)
                .partial_cmp(&quality_score(a))
                .unwrap_or(std::cmp::Ordering::Equal)
        }),
        "outlier" => mine.sort_by(|(a_id, a), (b_id, b)| {
            distance_of(b_id)
                .unwrap_or(-1.0)
                .partial_cmp(&distance_of(a_id).unwrap_or(-1.0))
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| {
                    quality_score(a)
                        .partial_cmp(&quality_score(b))
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
        }),
        _ => mine.sort_by(|(a_id, a), (b_id, b)| {
            distance_of(a_id)
                .unwrap_or(f64::MAX)
                .partial_cmp(&distance_of(b_id).unwrap_or(f64::MAX))
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| {
                    quality_score(b)
                        .partial_cmp(&quality_score(a))
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
        }),
    }

    let spread = {
        let distances: Vec<f64> = mine.iter().filter_map(|(id, _)| distance_of(id)).collect();
        if distances.is_empty() {
            Value::Null
        } else {
            json!(distances.iter().sum::<f64>() / distances.len() as f64)
        }
    };

    let faces: Vec<Value> = mine
        .iter()
        .skip(offset)
        .take(limit)
        .map(|(id, m)| {
            let input = FaceInput {
                id: id.clone(),
                embedding: Vec::new(),
                det_score: measured_at(m, "face_det_score"),
                area_ratio: measured_at(m, "face_area_ratio"),
                sharpness: measured_at(m, "face_sharpness"),
                occlusion: measured_at(m, "face_occlusion"),
                pinned: None,
            };
            json!({
                "face_id": id,
                "photo_id": photo_of(m),
                "thumbnail": m.get("thumbnail").and_then(Value::as_str).unwrap_or(""),
                "distance": distance_of(id),
                "manual": is_manual(m),
                "det_score": input.det_score,
                "area_ratio": input.area_ratio,
                "sharpness": input.sharpness,
                "occlusion": input.occlusion,
                // Why this face was not allowed to help decide who is who.
                // Shown on the tile so "why is this blurry stranger in here"
                // has a visible answer.
                "quality_note": gate.reject(&input),
            })
        })
        .collect();

    Json(json!({
        "status": "ok",
        "person_id": if wants_unassigned { String::new() } else { person_id.clone() },
        "name": names.get(&person_id).cloned().unwrap_or_default(),
        "total": total,
        "offset": offset,
        "limit": limit,
        "returned": faces.len(),
        "spread": spread,
        "faces": faces,
    }))
    .into_response()
}

// ------------------------------------------------------------------- editing

#[derive(Debug, Deserialize)]
struct AssignRequest {
    face_ids: Vec<String>,
    /// Where the faces go. Empty (or absent, with `new_person` unset) detaches
    /// them from whoever they are on now.
    person_id: Option<String>,
    /// Put them on a person that does not exist yet.
    new_person: Option<bool>,
    /// Name for the target person. Only applied when non-empty.
    name: Option<String>,
}

/// Moves faces onto a person, onto a new person, or off everybody.
///
/// Everything this touches is marked `person_manual`, which is what makes the
/// change survive the next clustering run.
async fn assign_faces(State(state): State<Arc<AppState>>, body: Option<Json<Value>>) -> Response {
    let raw = body.map(|Json(v)| v).unwrap_or_else(|| json!({}));
    let request: AssignRequest = match serde_json::from_value(raw) {
        Ok(r) => r,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": format!("Invalid assign request: {e}")})),
            )
                .into_response()
        }
    };
    if request.face_ids.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "face_ids must not be empty"})),
        )
            .into_response();
    }
    let Some(store) = state.store() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"error": "No catalog is open, so there are no faces to change."})),
        )
            .into_response();
    };

    // Resolve the target person first, because a new one needs an id nothing
    // else is using — including ids that exist only in the names file.
    let mut names = load_person_names(&state);
    let target = if request.new_person.unwrap_or(false) {
        let used = match store.scan_meta(FACE_TABLE).await {
            Ok(rows) => rows
                .iter()
                .map(|(_, m)| person_of(m).to_string())
                .collect::<HashSet<_>>(),
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({"error": e.to_string()})),
                )
                    .into_response()
            }
        };
        let index = next_person_index(
            used.iter()
                .map(String::as_str)
                .chain(names.keys().map(String::as_str)),
        );
        format!("person_{index}")
    } else {
        match request.person_id.as_deref() {
            None | Some("") => String::new(),
            Some(pid) if is_unassigned(pid) => String::new(),
            Some(pid) => pid.to_string(),
        }
    };

    let records = match store.get(FACE_TABLE, &request.face_ids).await {
        Ok(r) => r,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
                .into_response()
        }
    };
    // Silently updating fewer rows than asked for would look like success and
    // leave faces where they were, so the count is reported either way.
    let missing = request.face_ids.len() - records.len();

    let updated: Vec<StoreRecord> = records
        .into_iter()
        .map(|r| {
            let mut metadata = r.metadata;
            metadata.insert(
                "person_id".into(),
                json!(if target.is_empty() {
                    "person_unassigned"
                } else {
                    target.as_str()
                }),
            );
            metadata.insert("person_manual".into(), json!(true));
            StoreRecord {
                id: r.id,
                vector: r.vector,
                metadata,
            }
        })
        .collect();
    let count = updated.len();
    if let Err(e) = store.upsert(FACE_TABLE, &updated).await {
        log::error!("Assigning faces failed: {e}");
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        )
            .into_response();
    }

    if let Some(name) = request
        .name
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
    {
        if !target.is_empty() {
            names.insert(target.clone(), name.to_string());
            if let Err(e) = save_person_names(&state, &names) {
                log::error!("Saving the person name failed: {e}");
                return Json(json!({
                    "status": "ok",
                    "person_id": target,
                    "updated": count,
                    "warnings": [format!("The faces moved, but the name could not be saved: {e}")],
                }))
                .into_response();
            }
        }
    }

    let mut warnings: Vec<String> = Vec::new();
    if missing > 0 {
        warnings.push(format!(
            "{missing} of the {} selected faces are no longer in the catalog and were not moved. \
             Refresh the page to see what is actually there.",
            request.face_ids.len()
        ));
    }
    log::info!("Assigned {count} face(s) to person_id=\"{target}\"");
    Json(json!({
        "status": "ok",
        "person_id": target,
        "name": names.get(&target).cloned().unwrap_or_default(),
        "updated": count,
        "warnings": warnings,
    }))
    .into_response()
}

#[derive(Debug, Deserialize)]
struct MergeRequest {
    person_ids: Vec<String>,
    /// Which of them survives. Defaults to the one with the most faces.
    into: Option<String>,
    /// The name the merged person keeps. Absent leaves whatever `into` had.
    name: Option<String>,
}

/// Folds several people into one.
///
/// The merged faces are marked `person_manual`, so the next clustering run
/// treats them as one pre-formed person it may grow but never split — which is
/// what makes a merge stick instead of being undone by the next re-cluster.
async fn merge_persons(State(state): State<Arc<AppState>>, body: Option<Json<Value>>) -> Response {
    let raw = body.map(|Json(v)| v).unwrap_or_else(|| json!({}));
    let request: MergeRequest = match serde_json::from_value(raw) {
        Ok(r) => r,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": format!("Invalid merge request: {e}")})),
            )
                .into_response()
        }
    };
    let sources: Vec<String> = request
        .person_ids
        .iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && !is_unassigned(s))
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    if sources.len() < 2 {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": "Merging needs at least two different people. Unassigned faces are not a \
                          person — move those with /v1/faces/assign instead."
            })),
        )
            .into_response();
    }
    let Some(store) = state.store() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"error": "No catalog is open, so there is nothing to merge."})),
        )
            .into_response();
    };

    let rows = match store.scan_meta(FACE_TABLE).await {
        Ok(r) => r,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
                .into_response()
        }
    };
    let source_set: HashSet<&str> = sources.iter().map(String::as_str).collect();
    let mut by_person: HashMap<String, Vec<String>> = HashMap::new();
    for (id, meta) in &rows {
        let pid = person_of(meta);
        if source_set.contains(pid) {
            by_person
                .entry(pid.to_string())
                .or_default()
                .push(id.clone());
        }
    }
    if by_person.is_empty() {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "None of those people have any faces left. Refresh the page."})),
        )
            .into_response();
    }

    let target = match request.into.as_deref().map(str::trim) {
        Some(t) if source_set.contains(t) => t.to_string(),
        _ => by_person
            .iter()
            .max_by_key(|(pid, faces)| (faces.len(), std::cmp::Reverse((*pid).clone())))
            .map(|(pid, _)| pid.clone())
            .unwrap_or_else(|| sources[0].clone()),
    };

    let face_ids: Vec<String> = by_person
        .iter()
        .filter(|(pid, _)| *pid != &target)
        .flat_map(|(_, ids)| ids.clone())
        .collect();
    // The target's own faces are marked manual too: a merge is a statement
    // about the whole resulting person, and leaving half of it unpinned would
    // let the next run split the target back out from underneath the merge.
    let mut all_ids = face_ids.clone();
    all_ids.extend(by_person.get(&target).cloned().unwrap_or_default());

    let records = match store.get(FACE_TABLE, &all_ids).await {
        Ok(r) => r,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
                .into_response()
        }
    };
    let updated: Vec<StoreRecord> = records
        .into_iter()
        .map(|r| {
            let mut metadata = r.metadata;
            metadata.insert("person_id".into(), json!(target.clone()));
            metadata.insert("person_manual".into(), json!(true));
            StoreRecord {
                id: r.id,
                vector: r.vector,
                metadata,
            }
        })
        .collect();
    let moved = face_ids.len();
    let touched = updated.len();
    if let Err(e) = store.upsert(FACE_TABLE, &updated).await {
        log::error!("Merging persons failed: {e}");
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        )
            .into_response();
    }

    let mut names = load_person_names(&state);
    let chosen = request
        .name
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(str::to_string)
        .or_else(|| names.get(&target).cloned())
        // Falling back to any name the merged-away people had beats losing it:
        // merging an unnamed cluster into "Anna" must not end up nameless.
        .or_else(|| {
            sources
                .iter()
                .filter_map(|pid| names.get(pid).cloned())
                .next()
        });
    for pid in &sources {
        if pid != &target {
            names.remove(pid);
        }
    }
    let mut warnings: Vec<String> = Vec::new();
    match chosen {
        Some(name) => {
            names.insert(target.clone(), name);
        }
        None => {
            names.remove(&target);
        }
    }
    if let Err(e) = save_person_names(&state, &names) {
        log::error!("Saving names after a merge failed: {e}");
        warnings.push(format!(
            "The people were merged, but the name could not be saved: {e}"
        ));
    }

    let merged_from: Vec<&String> = sources.iter().filter(|p| *p != &target).collect();
    log::info!(
        "Merged {} person(s) into {target}: {moved} face(s) moved, {touched} pinned.",
        merged_from.len()
    );
    Json(json!({
        "status": "ok",
        "person_id": target,
        "name": names.get(&target).cloned().unwrap_or_default(),
        "moved": moved,
        "updated": touched,
        "merged_from": merged_from,
        "warnings": warnings,
    }))
    .into_response()
}
