//! DB blueprint — port of `routes/db.py`: /db/stats, /db/backup.
//! /db/migrate-photo-ids is legacy (uuid -> photo_id one-time migration)
//! and isn't carried over to the Rust backend.

use std::collections::BTreeSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::{
    body::Bytes,
    extract::{Query, State},
    http::{
        header::{CONTENT_DISPOSITION, CONTENT_TYPE},
        StatusCode,
    },
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use chrono::Utc;
use serde_json::{json, Value};

use lrg_store::{meta, FACE_TABLE, IMAGE_TABLE, SPECIES_TABLE, VERTEX_TABLE};

use crate::state::AppState;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/db/stats", get(database_stats))
        .route("/db/backup", get(backup_database))
}

#[derive(serde::Deserialize)]
struct StatsQuery {
    catalog_id: Option<String>,
    #[allow(dead_code)]
    db_path: Option<String>,
}

async fn database_stats(
    State(state): State<Arc<AppState>>,
    Query(query): Query<StatsQuery>,
) -> Response {
    let empty = json!({
        "photos": {"total": 0, "with_embedding": 0, "with_title": 0,
                    "with_caption": 0, "with_keywords": 0, "with_vertexai": 0,
                    "with_species": 0, "with_species_id": 0},
        "faces": {"total": 0},
        "persons": {"total": 0},
    });
    let Some(store) = state.store() else {
        return Json(empty).into_response();
    };

    let result: Result<Value, String> = async {
        let vertex_ids: BTreeSet<String> = store
            .scan_meta(VERTEX_TABLE)
            .await
            .map_err(|e| e.to_string())?
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        let catalog_id = query
            .catalog_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());

        let non_empty = |m: &serde_json::Map<String, Value>, key: &str| {
            m.get(key)
                .and_then(Value::as_str)
                .is_some_and(|s| !s.trim().is_empty())
        };

        // Only the ids are needed, unlike VERTEX_TABLE above whose metadata
        // this route used to want.
        let species_ids: BTreeSet<String> = store
            .scan_ids(SPECIES_TABLE)
            .await
            .map_err(|e| e.to_string())?
            .into_iter()
            .collect();

        let (mut total, mut with_embedding, mut with_title) = (0u64, 0u64, 0u64);
        let (mut with_caption, mut with_keywords, mut with_vertexai) = (0u64, 0u64, 0u64);
        // `with_species` is "BioCLIP has looked at it", `with_species_id` is
        // "and it came back with something". The gap between the two is how
        // much of the catalog is being scanned for nothing, which is the
        // number that says whether the organism pre-filter is earning its
        // keep.
        let (mut with_species, mut with_species_id) = (0u64, 0u64);
        for (id, m) in store
            .scan_meta(IMAGE_TABLE)
            .await
            .map_err(|e| e.to_string())?
        {
            if let Some(cat) = catalog_id {
                if !meta::parse_catalog_ids(&m).contains(cat) {
                    continue;
                }
            }
            total += 1;
            if meta::has_embedding(&m) {
                with_embedding += 1;
            }
            if non_empty(&m, "title") {
                with_title += 1;
            }
            if non_empty(&m, "caption") {
                with_caption += 1;
            }
            if non_empty(&m, "keywords") || non_empty(&m, "flattened_keywords") {
                with_keywords += 1;
            }
            if vertex_ids.contains(&id) {
                with_vertexai += 1;
            }
            if species_ids.contains(&id) {
                with_species += 1;
            }
            if m.get("species_rank")
                .and_then(Value::as_str)
                .is_some_and(|r| r != "none")
            {
                with_species_id += 1;
            }
        }

        let face_rows = store
            .scan_meta(FACE_TABLE)
            .await
            .map_err(|e| e.to_string())?;
        let faces_total = face_rows.len();
        let persons: BTreeSet<String> = face_rows
            .iter()
            .filter_map(|(_, m)| m.get("person_id").and_then(Value::as_str))
            .filter(|p| !p.is_empty() && *p != "person_unassigned")
            .map(str::to_string)
            .collect();

        Ok(json!({
            "photos": {"total": total, "with_embedding": with_embedding,
                        "with_title": with_title, "with_caption": with_caption,
                        "with_keywords": with_keywords, "with_vertexai": with_vertexai,
                        "with_species": with_species, "with_species_id": with_species_id},
            "faces": {"total": faces_total},
            "persons": {"total": persons.len()},
        }))
    }
    .await;

    match result {
        Ok(v) => Json(v).into_response(),
        Err(e) => {
            log::error!("Error computing database stats: {e}");
            (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e}))).into_response()
        }
    }
}

/// Port of `services/db.py::build_backup_zip`. Zips the LanceDB root
/// (the Rust equivalent of Python's Chroma `db_path` — see
/// `AppState::lance_root`), excluding its own top-level `backups/`
/// directory to avoid self-embedding, with archive paths relative to
/// the root's parent so the zip preserves the `lrgenius-lance/...`
/// prefix. Also saves a persistent copy under `<lance_root>/backups/`,
/// matching Python's server-side retention (though the periodic
/// prune-old-backups scheduler itself isn't ported yet — see
/// server-rs/README.md).
async fn backup_database(State(state): State<Arc<AppState>>) -> Response {
    let Some(lance_root) = state.lance_root() else {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": "Database path is not bound yet"})),
        )
            .into_response();
    };
    if !lance_root.is_dir() {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": format!("Database path does not exist or is not a directory: {}", lance_root.display())})),
        )
            .into_response();
    }

    let backup_name = format!(
        "lrgeniusai-backend-backup-{}.zip",
        Utc::now().format("%Y%m%d-%H%M%S")
    );

    let task_result = tokio::task::spawn_blocking({
        let lance_root = lance_root.clone();
        let backup_name = backup_name.clone();
        move || build_backup_zip(&lance_root, &backup_name)
    })
    .await;

    let bytes = match task_result {
        Ok(Ok(bytes)) => bytes,
        Ok(Err(e)) => {
            log::error!("Database backup failed: {e}");
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e}))).into_response();
        }
        Err(e) => {
            log::error!("Database backup task panicked: {e}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
                .into_response();
        }
    };

    (
        [
            (CONTENT_TYPE, "application/zip".to_string()),
            (
                CONTENT_DISPOSITION,
                format!("attachment; filename=\"{backup_name}\""),
            ),
        ],
        Bytes::from(bytes),
    )
        .into_response()
}

fn build_backup_zip(lance_root: &Path, backup_name: &str) -> Result<Vec<u8>, String> {
    let root_parent = lance_root.parent().unwrap_or_else(|| Path::new("."));

    let mut entries = Vec::new();
    collect_backup_files(lance_root, lance_root, &mut entries)
        .map_err(|e| format!("failed to walk {}: {e}", lance_root.display()))?;
    entries.sort();

    let cursor = std::io::Cursor::new(Vec::new());
    let mut archive = zip::ZipWriter::new(cursor);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);

    let mut included = 0usize;
    for full_path in &entries {
        let arcname = full_path
            .strip_prefix(root_parent)
            .unwrap_or(full_path)
            .to_string_lossy()
            .replace('\\', "/");
        let data = std::fs::read(full_path)
            .map_err(|e| format!("failed to read {}: {e}", full_path.display()))?;
        archive
            .start_file(&arcname, options)
            .map_err(|e| e.to_string())?;
        archive.write_all(&data).map_err(|e| e.to_string())?;
        included += 1;
    }
    let cursor = archive.finish().map_err(|e| e.to_string())?;
    let bytes = cursor.into_inner();
    log::info!(
        "Created DB backup zip ({included} files, {} bytes) from {}",
        bytes.len(),
        lance_root.display()
    );

    let backups_dir = lance_root.join("backups");
    if let Err(e) = std::fs::create_dir_all(&backups_dir) {
        log::warn!(
            "Could not create backups dir {}: {e}",
            backups_dir.display()
        );
    } else {
        let persistent_path = backups_dir.join(backup_name);
        if let Err(e) = std::fs::write(&persistent_path, &bytes) {
            log::warn!(
                "Could not save backup to {}: {e}",
                persistent_path.display()
            );
        } else {
            log::info!(
                "DB backup saved server-side to {}",
                persistent_path.display()
            );
        }
    }

    Ok(bytes)
}

/// Recursively collects file paths under `dir`, skipping `lance_root`'s
/// own top-level `backups` directory (self-embedding guard, matching
/// Python's `dirs[:] = [... if not (current_root == db_path and d ==
/// "backups")]`).
fn collect_backup_files(
    lance_root: &Path,
    dir: &Path,
    out: &mut Vec<PathBuf>,
) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            if dir == lance_root && entry.file_name() == "backups" {
                continue;
            }
            collect_backup_files(lance_root, &path, out)?;
        } else if file_type.is_file() {
            out.push(path);
        }
    }
    Ok(())
}
