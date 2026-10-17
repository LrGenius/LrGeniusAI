//! One-time upgrades to rows already in the LanceDB store, run when a catalog
//! database is bound.
//!
//! [`crate::migrate`] moves data *into* LanceDB from the retired Chroma
//! backend; this changes data that is already here, for the day a field is
//! added that existing rows ought to carry. Each step has a stable name, and
//! `upgrades.json` in the lance root records which steps have run, so a step
//! costs its scan once rather than on every bind.
//!
//! Steps must be idempotent anyway. The marker is written after a step
//! finishes, so a crash part-way through re-runs it on the next bind, and it
//! has to pick up where it left off rather than redo or double-apply anything.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};

use crate::{meta, Store, IMAGE_TABLE};

/// Rows read and rewritten per write. Each carries a 1152-float vector, so a
/// batch is a few MB rather than the whole table at once.
const BATCH_SIZE: usize = 500;

/// Stamps `embedding_model` on image embeddings stored before the field
/// existed.
pub const STAMP_EMBEDDING_MODEL: &str = "0001-stamp-embedding-model";

/// Facts about this build that a step needs and the store crate cannot know:
/// it does not depend on `lrg-ml`, which owns the model identities.
pub struct UpgradeContext<'a> {
    /// `lrg_ml::siglip::MODEL_ID`, the embedding function this build writes.
    pub embedding_model: &'a str,
}

/// What [`ensure_upgraded`] did on this bind.
#[derive(Debug, Default)]
pub struct UpgradeReport {
    /// Steps that ran now, each with its summary.
    pub applied: Vec<(String, Value)>,
    /// The step that failed and why. Later steps are not attempted: one may
    /// depend on the one before it.
    pub failed: Option<(String, String)>,
}

pub fn upgrade_marker(lance_root: &Path) -> PathBuf {
    lance_root.join("upgrades.json")
}

/// Runs every step the marker does not list yet, in order.
///
/// A failed step is reported rather than returned as an error, and the bind
/// goes on: no step so far is something the store cannot work without, and
/// the marker stays unwritten for it, so the next bind tries again.
pub async fn ensure_upgraded(
    lance_root: &Path,
    store: &Store,
    ctx: &UpgradeContext<'_>,
) -> UpgradeReport {
    let marker_path = upgrade_marker(lance_root);
    let mut marker = read_marker(&marker_path);
    let mut report = UpgradeReport::default();

    let applied: BTreeSet<String> = marker
        .get("applied")
        .and_then(Value::as_object)
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default();

    if !applied.contains(STAMP_EMBEDDING_MODEL) {
        match stamp_embedding_model(store, ctx.embedding_model).await {
            Ok(stamped) => {
                let summary = json!({
                    "applied_at": chrono::Utc::now().to_rfc3339(),
                    "embedding_model": ctx.embedding_model,
                    "stamped": stamped,
                });
                record_applied(&mut marker, STAMP_EMBEDDING_MODEL, summary.clone());
                if let Err(e) = write_marker(&marker_path, &marker) {
                    // The rows are stamped either way; without the marker the
                    // next bind only scans again and finds nothing to do.
                    log::warn!("Could not record store upgrade {STAMP_EMBEDDING_MODEL}: {e}");
                }
                report
                    .applied
                    .push((STAMP_EMBEDDING_MODEL.to_string(), summary));
            }
            Err(e) => {
                report.failed = Some((STAMP_EMBEDDING_MODEL.to_string(), e));
                return report;
            }
        }
    }

    report
}

/// Sets `embedding_model` to `model_id` on every image embedding that has a
/// vector and no identity yet. Returns how many rows it stamped.
///
/// Only rows that predate the field are touched. A row that already names a
/// model keeps it — even a different one, since that difference is exactly
/// what `/v1/index/unprocessed` re-queues on — and a row with no vector has no
/// embedding to describe.
///
/// Stamping the absent identity as *this* build's is safe because there has
/// only ever been one SigLIP2 export: the assets are pinned to one release
/// tag, so every stored vector came from the function `model_id` names.
pub async fn stamp_embedding_model(store: &Store, model_id: &str) -> Result<usize, String> {
    let candidates: Vec<String> = store
        .scan_meta(IMAGE_TABLE)
        .await
        .map_err(|e| format!("scan of {IMAGE_TABLE} failed: {e}"))?
        .into_iter()
        .filter(|(_, m)| needs_stamp(m))
        .map(|(id, _)| id)
        .collect();

    let mut stamped = 0usize;
    for chunk in candidates.chunks(BATCH_SIZE) {
        // Read back with vectors: `upsert` replaces the whole row, so the
        // vector has to travel with the new metadata or it would be lost.
        let mut records = store
            .get(IMAGE_TABLE, chunk)
            .await
            .map_err(|e| format!("read of {IMAGE_TABLE} failed: {e}"))?;
        records.retain(|r| r.vector.is_some() && needs_stamp(&r.metadata));
        for record in &mut records {
            record
                .metadata
                .insert(meta::EMBEDDING_MODEL_FIELD.into(), json!(model_id));
        }
        store
            .upsert(IMAGE_TABLE, &records)
            .await
            .map_err(|e| format!("write to {IMAGE_TABLE} failed: {e}"))?;
        stamped += records.len();
    }
    Ok(stamped)
}

fn needs_stamp(metadata: &Map<String, Value>) -> bool {
    meta::has_embedding(metadata) && !metadata.contains_key(meta::EMBEDDING_MODEL_FIELD)
}

/// The marker's contents, or an empty object when there is none.
///
/// An unreadable marker is treated as empty rather than as an error: every
/// step is idempotent, so running them again costs a scan and nothing else.
fn read_marker(path: &Path) -> Map<String, Value> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Map::new();
    };
    match serde_json::from_str::<Value>(&text) {
        Ok(Value::Object(map)) => map,
        _ => {
            log::warn!(
                "Store upgrade marker {} is not valid JSON; re-checking every step",
                path.display()
            );
            Map::new()
        }
    }
}

fn record_applied(marker: &mut Map<String, Value>, step: &str, summary: Value) {
    let applied = marker
        .entry("applied")
        .or_insert_with(|| Value::Object(Map::new()));
    if !applied.is_object() {
        *applied = Value::Object(Map::new());
    }
    if let Some(map) = applied.as_object_mut() {
        map.insert(step.to_string(), summary);
    }
}

/// Written beside and then renamed over the old marker, so a crash mid-write
/// cannot leave half a file that would read as "nothing applied".
fn write_marker(path: &Path, marker: &Map<String, Value>) -> std::io::Result<()> {
    let tmp = path.with_extension("json.tmp");
    let text = serde_json::to_string_pretty(&Value::Object(marker.clone()))
        .map_err(std::io::Error::other)?;
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::StoreRecord;

    const MODEL: &str = "siglip2-test/fp16";

    fn image_row(id: &str, vector: Option<Vec<f32>>, extra: Value) -> StoreRecord {
        let mut metadata = Map::new();
        metadata.insert("photo_id".into(), json!(id));
        if let Value::Object(map) = extra {
            metadata.extend(map);
        }
        StoreRecord {
            id: id.to_string(),
            vector,
            metadata,
        }
    }

    async fn model_of(store: &Store, id: &str) -> Option<String> {
        store
            .get(IMAGE_TABLE, &[id.to_string()])
            .await
            .unwrap()
            .pop()
            .and_then(|r| {
                r.metadata
                    .get(meta::EMBEDDING_MODEL_FIELD)
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
    }

    async fn seeded_store(dir: &Path) -> Store {
        let store = Store::open(dir).await.unwrap();
        store
            .upsert(
                IMAGE_TABLE,
                &[
                    // Predates the field: the one this step exists for.
                    image_row(
                        "legacy",
                        Some(vec![0.1; 1152]),
                        json!({"has_embedding": true, "title": "kept"}),
                    ),
                    // Older still: no `has_embedding` either, which reads as true.
                    image_row("ancient", Some(vec![0.2; 1152]), json!({})),
                    // Already names a model — a different one must survive, it
                    // is what makes the photo re-queue.
                    image_row(
                        "other",
                        Some(vec![0.3; 1152]),
                        json!({"has_embedding": true, "embedding_model": "older-model"}),
                    ),
                    // Metadata only: nothing to describe.
                    image_row("no_vector", None, json!({"has_embedding": false})),
                ],
            )
            .await
            .unwrap();
        store
    }

    #[tokio::test]
    async fn stamps_only_rows_that_hold_an_unlabelled_vector() {
        let dir = tempfile::tempdir().unwrap();
        let store = seeded_store(dir.path()).await;

        let stamped = stamp_embedding_model(&store, MODEL).await.unwrap();

        assert_eq!(stamped, 2);
        assert_eq!(model_of(&store, "legacy").await.as_deref(), Some(MODEL));
        assert_eq!(model_of(&store, "ancient").await.as_deref(), Some(MODEL));
        assert_eq!(
            model_of(&store, "other").await.as_deref(),
            Some("older-model")
        );
        assert_eq!(model_of(&store, "no_vector").await, None);
    }

    /// The vector and every other field ride through the rewrite unchanged:
    /// `upsert` replaces whole rows, so dropping either would lose data.
    #[tokio::test]
    async fn stamping_keeps_the_vector_and_the_rest_of_the_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let store = seeded_store(dir.path()).await;

        stamp_embedding_model(&store, MODEL).await.unwrap();

        let row = store
            .get(IMAGE_TABLE, &["legacy".to_string()])
            .await
            .unwrap()
            .pop()
            .unwrap();
        assert_eq!(row.vector, Some(vec![0.1; 1152]));
        assert_eq!(row.metadata.get("title"), Some(&json!("kept")));
        assert_eq!(store.count(IMAGE_TABLE).await.unwrap(), 4);
    }

    #[tokio::test]
    async fn runs_once_and_records_it_in_the_marker() {
        let dir = tempfile::tempdir().unwrap();
        let store = seeded_store(dir.path()).await;
        let ctx = UpgradeContext {
            embedding_model: MODEL,
        };

        let first = ensure_upgraded(dir.path(), &store, &ctx).await;
        assert!(first.failed.is_none());
        assert_eq!(first.applied.len(), 1);
        assert_eq!(first.applied[0].1["stamped"], json!(2));

        let marker: Value =
            serde_json::from_str(&std::fs::read_to_string(upgrade_marker(dir.path())).unwrap())
                .unwrap();
        assert_eq!(
            marker["applied"][STAMP_EMBEDDING_MODEL]["embedding_model"],
            json!(MODEL)
        );

        // A row written by an older build after the first bind is left alone:
        // the step has run, and an absent identity still reads as current.
        store
            .upsert(
                IMAGE_TABLE,
                &[image_row(
                    "late",
                    Some(vec![0.4; 1152]),
                    json!({"has_embedding": true}),
                )],
            )
            .await
            .unwrap();
        let second = ensure_upgraded(dir.path(), &store, &ctx).await;
        assert!(second.applied.is_empty());
        assert!(second.failed.is_none());
        assert_eq!(model_of(&store, "late").await, None);
    }

    #[tokio::test]
    async fn an_unreadable_marker_re_runs_the_steps_harmlessly() {
        let dir = tempfile::tempdir().unwrap();
        let store = seeded_store(dir.path()).await;
        std::fs::write(upgrade_marker(dir.path()), "not json").unwrap();

        let report = ensure_upgraded(
            dir.path(),
            &store,
            &UpgradeContext {
                embedding_model: MODEL,
            },
        )
        .await;

        assert_eq!(report.applied.len(), 1);
        assert_eq!(model_of(&store, "legacy").await.as_deref(), Some(MODEL));
    }
}
