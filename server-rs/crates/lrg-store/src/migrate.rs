//! One-time migration from a ChromaDB persistent dir into the LanceDB
//! store. The chroma dir is never modified (rollback = reinstall the old
//! backend). A `migration.json` marker in the lance root gates re-runs.

use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};

use crate::{
    meta, Store, StoreRecord, FACE_TABLE, IMAGE_TABLE, TABLES, TRAINING_TABLE, VERTEX_TABLE,
};

const BATCH_SIZE: usize = 1000;

pub fn migration_marker(lance_root: &Path) -> PathBuf {
    lance_root.join("migration.json")
}

/// Lance store root for a given db_path (`<dir>/lrgenius.db` ->
/// `<dir>/lrgenius-lance`).
pub fn lance_root_for_db(db_path: &Path) -> PathBuf {
    db_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("lrgenius-lance")
}

/// Run the migration if a chroma dir exists at `db_path` and no marker is
/// present yet. Returns the summary when a migration ran, None otherwise.
pub async fn ensure_migrated(
    db_path: &Path,
    lance_root: &Path,
    store: &Store,
) -> Result<Option<Value>, String> {
    if !db_path.join("chroma.sqlite3").is_file() {
        return Ok(None); // fresh install, nothing to migrate
    }
    if migration_marker(lance_root).is_file() {
        return Ok(None);
    }
    let summary = migrate_from_chroma(db_path, store).await?;
    std::fs::write(
        migration_marker(lance_root),
        serde_json::to_string_pretty(&summary).unwrap_or_default(),
    )
    .map_err(|e| format!("failed to write migration marker: {e}"))?;
    Ok(Some(summary))
}

pub async fn migrate_from_chroma(chroma_dir: &Path, store: &Store) -> Result<Value, String> {
    let chroma_dir_owned = chroma_dir.to_path_buf();
    let collections =
        tokio::task::spawn_blocking(move || lrg_chroma_reader::read_chroma_dir(&chroma_dir_owned))
            .await
            .map_err(|e| format!("migration task panicked: {e}"))?
            .map_err(|e| format!("failed to read chroma dir: {e}"))?;

    let mut tables = Map::new();
    let mut skipped: Vec<String> = Vec::new();
    let mut needs_reembedding = 0usize;

    for collection in collections {
        let table = match collection.name.as_str() {
            IMAGE_TABLE => IMAGE_TABLE,
            FACE_TABLE => FACE_TABLE,
            VERTEX_TABLE => VERTEX_TABLE,
            TRAINING_TABLE => TRAINING_TABLE,
            other => {
                log::warn!("migration: skipping unknown collection {other}");
                skipped.push(other.to_string());
                continue;
            }
        };
        let expected_dim = TABLES.iter().find(|(n, _)| *n == table).unwrap().1 as usize;

        let source_count = collection.records.len();
        let mut migrated = 0usize;
        let mut batch: Vec<StoreRecord> = Vec::with_capacity(BATCH_SIZE);
        for record in collection.records {
            // Chroma's zero-vector hack becomes a NULL vector; a missing
            // vector for a record that claims one is flagged for re-embedding.
            let claims_embedding = meta::has_embedding(&record.metadata);
            let vector = match record.embedding {
                Some(v) if !claims_embedding => {
                    debug_assert!(v.iter().all(|x| *x == 0.0) || !v.is_empty());
                    None
                }
                Some(v) if v.len() == expected_dim => Some(v),
                Some(v) => {
                    log::warn!(
                        "migration: {table}/{} has dim {} (expected {expected_dim}); flagged for re-embedding",
                        record.id,
                        v.len()
                    );
                    needs_reembedding += 1;
                    None
                }
                None => {
                    if claims_embedding {
                        needs_reembedding += 1;
                    }
                    None
                }
            };
            batch.push(StoreRecord {
                id: record.id,
                vector,
                metadata: record.metadata,
            });
            if batch.len() >= BATCH_SIZE {
                store
                    .append(table, &batch)
                    .await
                    .map_err(|e| format!("append to {table} failed: {e}"))?;
                migrated += batch.len();
                batch.clear();
            }
        }
        store
            .append(table, &batch)
            .await
            .map_err(|e| format!("append to {table} failed: {e}"))?;
        migrated += batch.len();

        // Verify counts before declaring success.
        let target_count = store
            .count(table)
            .await
            .map_err(|e| format!("count of {table} failed: {e}"))?;
        if target_count < migrated {
            return Err(format!(
                "verification failed for {table}: migrated {migrated} but table holds {target_count}"
            ));
        }
        log::info!("migration: {table}: {migrated}/{source_count} records migrated");
        tables.insert(
            table.to_string(),
            json!({"source": source_count, "migrated": migrated}),
        );
    }

    Ok(json!({
        "migrated_at": chrono::Utc::now().to_rfc3339(),
        "tables": tables,
        "needs_reembedding": needs_reembedding,
        "skipped_collections": skipped,
    }))
}
