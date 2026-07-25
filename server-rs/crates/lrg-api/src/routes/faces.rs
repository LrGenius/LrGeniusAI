//! Faces blueprint — port of `routes/faces.py`.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use axum::{
    extract::{Path as UrlPath, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post, put},
    Json, Router,
};
use base64::Engine;
use serde_json::{json, Value};

use lrg_analysis::{clustering, persons};
use lrg_store::{StoreRecord, FACE_TABLE};

use crate::state::AppState;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/faces/detect", post(detect))
        .route("/faces/query", post(query))
        .route("/faces/cluster", post(cluster))
        .route("/faces/persons", get(list_persons))
        .route(
            "/faces/persons/{person_id}/thumbnail",
            get(person_thumbnail),
        )
        .route("/faces/persons/{person_id}", put(set_person_name))
        .route("/faces/persons/{person_id}/photos", get(person_photos))
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
    match state.face.detect_faces(&pixels, w, h) {
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

    let faces = match state.face.detect_faces(&pixels, w, h) {
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

async fn cluster(State(state): State<Arc<AppState>>, body: Option<Json<Value>>) -> Response {
    log::info!("Faces cluster request received");
    let data = body.map(|Json(v)| v).unwrap_or_else(|| json!({}));
    let threshold = data
        .get("distance_threshold")
        .and_then(Value::as_f64)
        .unwrap_or(0.5);
    let min_faces: Option<i64> = match data.get("min_faces_per_person") {
        None => Some(3),
        Some(Value::Null) => None,
        Some(v) => v
            .as_i64()
            .or_else(|| v.as_str().and_then(|s| s.parse().ok())),
    };
    let linkage_str = data
        .get("linkage")
        .and_then(Value::as_str)
        .unwrap_or("complete")
        .to_lowercase();
    let linkage = if linkage_str == "average" {
        clustering::Linkage::Average
    } else {
        clustering::Linkage::Complete
    };

    let empty_summary =
        json!({"status": "ok", "person_count": 0, "face_count": 0, "updated": 0, "unassigned": 0});
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
        let ids: Vec<String> = records.iter().map(|r| r.id.clone()).collect();
        let embeddings: Vec<Vec<f32>> = records
            .iter()
            .map(|r| r.vector.clone().unwrap_or_default())
            .collect();
        let n = records.len();

        let l2_threshold = clustering::cosine_to_l2(threshold);
        let labels: Vec<i64> = if let Some(min_faces) = min_faces.filter(|&m| m >= 2) {
            if n == 1 {
                vec![-1]
            } else {
                clustering::dbscan(&embeddings, l2_threshold, min_faces as usize)
            }
        } else if n == 1 {
            vec![0]
        } else {
            clustering::agglomerative(&embeddings, l2_threshold, linkage)
        };

        let mut old_person_faces: HashMap<String, HashSet<String>> = HashMap::new();
        for r in &records {
            if let Some(pid) = r.metadata.get("person_id").and_then(Value::as_str) {
                if !pid.is_empty() && pid != "person_unassigned" {
                    old_person_faces
                        .entry(pid.to_string())
                        .or_default()
                        .insert(r.id.clone());
                }
            }
        }
        let mut new_label_faces: HashMap<i64, HashSet<String>> = HashMap::new();
        for (i, &lb) in labels.iter().enumerate() {
            if lb >= 0 {
                new_label_faces
                    .entry(lb)
                    .or_default()
                    .insert(ids[i].clone());
            }
        }
        let label_to_person = persons::match_labels_to_persons(&old_person_faces, &new_label_faces);

        let mut unassigned_count = 0usize;
        let mut updated: Vec<StoreRecord> = Vec::with_capacity(n);
        for (i, record) in records.into_iter().enumerate() {
            let lb = labels[i];
            let person_id = if lb < 0 {
                unassigned_count += 1;
                "person_unassigned".to_string()
            } else {
                label_to_person
                    .get(&lb)
                    .cloned()
                    .unwrap_or_else(|| "person_unassigned".to_string())
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

        let person_count = label_to_person.len() + usize::from(unassigned_count > 0);
        Ok(json!({
            "status": "ok",
            "person_count": person_count,
            "face_count": n,
            "updated": n,
            "unassigned": unassigned_count,
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

    struct Info {
        face_count: usize,
        photo_ids: HashSet<String>,
    }
    let mut by_person: HashMap<String, Info> = HashMap::new();
    for (_, meta) in &rows {
        let pid = meta
            .get("person_id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let key = if pid.is_empty() {
            "_unassigned".to_string()
        } else {
            pid
        };
        let photo_id = meta
            .get("photo_id")
            .or_else(|| meta.get("photo_uuid"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let entry = by_person.entry(key).or_insert_with(|| Info {
            face_count: 0,
            photo_ids: HashSet::new(),
        });
        entry.face_count += 1;
        entry.photo_ids.insert(photo_id);
    }

    let mut entries: Vec<(String, Info)> = by_person.into_iter().collect();
    entries.sort_by(|(a_pid, a), (b_pid, b)| {
        let a_unassigned = a_pid == "_unassigned" || a_pid == "person_unassigned";
        let b_unassigned = b_pid == "_unassigned" || b_pid == "person_unassigned";
        a_unassigned
            .cmp(&b_unassigned)
            .then(b.photo_ids.len().cmp(&a.photo_ids.len()))
            .then(a_pid.cmp(b_pid))
    });

    let persons: Vec<Value> = entries
        .into_iter()
        .map(|(pid, info)| {
            let person_id = if pid == "_unassigned" { String::new() } else { pid };
            json!({
                "person_id": person_id,
                "name": if person_id_is_empty(&person_id) { String::new() } else { names.get(&person_id).cloned().unwrap_or_default() },
                "face_count": info.face_count,
                "photo_count": info.photo_ids.len(),
            })
        })
        .collect();
    Json(json!({"status": "ok", "persons": persons})).into_response()
}

fn person_id_is_empty(s: &str) -> bool {
    s.is_empty()
}

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
            .find(|(_, m)| m.get("person_id").and_then(Value::as_str) == Some(person_id.as_str()))
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
    let mut photo_ids: Vec<String> = rows
        .into_iter()
        .filter(|(_, m)| m.get("person_id").and_then(Value::as_str) == Some(person_id.as_str()))
        .filter_map(|(_, m)| {
            m.get("photo_id")
                .or_else(|| m.get("photo_uuid"))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
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
