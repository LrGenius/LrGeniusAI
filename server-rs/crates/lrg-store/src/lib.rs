//! LanceDB-backed vector store replacing the embedded ChromaDB.
//!
//! Schema per table: `id: Utf8` (unique), `vector: FixedSizeList<Float32, D>`
//! (nullable — metadata-only photos store NULL instead of Chroma's zero-vector
//! hack), `metadata: Utf8` (JSON object, byte-compatible with the Chroma
//! metadata dict including `catalog_ids` as a JSON-list string and
//! `has_embedding`). Keeping metadata as one JSON column preserves exact API
//! parity; promoting hot fields to typed columns for filter pushdown is a
//! planned M6 optimization.

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use arrow_array::builder::{FixedSizeListBuilder, Float32Builder, StringBuilder};
use arrow_array::{
    Array, FixedSizeListArray, Float32Array, RecordBatch, RecordBatchIterator, StringArray,
};
use arrow_schema::{DataType, Field, Schema};
use futures::TryStreamExt;
use lancedb::index::scalar::BTreeIndexBuilder;
use lancedb::index::Index;
use lancedb::query::{ExecutableQuery, QueryBase};
use lancedb::table::{CompactionOptions, Duration, OptimizeAction, OptimizeOptions};
use serde_json::{Map, Value};

pub mod meta;
pub mod migrate;

/// Table names match the Chroma collection names 1:1 so the migration and
/// the service layer share vocabulary.
pub const IMAGE_TABLE: &str = "image_embeddings";
pub const FACE_TABLE: &str = "face_embeddings";
pub const VERTEX_TABLE: &str = "image_embeddings_vertex";
pub const TRAINING_TABLE: &str = "edit_training";

pub const TABLES: [(&str, i32); 4] = [
    (IMAGE_TABLE, 1152),
    (FACE_TABLE, 512),
    (VERTEX_TABLE, 1408),
    (TRAINING_TABLE, 1152),
];

/// Chunk size for `id IN (...)` predicates, mirroring the Python
/// GET_IDS_CHUNK_SIZE rationale (bounded query size on large catalogs).
const GET_IDS_CHUNK_SIZE: usize = 1000;

/// How many write ops must pile up before the maintenance loop compacts.
///
/// LanceDB's own guidance ("compact after ~20+ write ops") assumes each
/// write op is a bulk load; ours is a single photo, so 25 meant compacting
/// roughly every twelve photos. Since compaction reads and rewrites every
/// fragment it selects, doing it that often turned indexing into a
/// continuous rewrite of the whole table — see `compaction_options` for
/// the other half of that story. At the observed ~4 photos/sec this
/// threshold lands compaction near the 30s maintenance tick instead.
pub const COMPACT_WRITE_THRESHOLD: u64 = 200;

/// Rows per fragment that compaction should aim for, and the point past
/// which a fragment is left alone.
///
/// This is the important one. Lance's default is 1,048,576 rows — far more
/// than any Lightroom catalog puts in one of our tables, so *every*
/// fragment always qualified as a compaction candidate and each run
/// re-read and rewrote the entire table. Work per run therefore grew with
/// the catalog while running every dozen photos, which is what kept RSS
/// climbing through a long indexing session. With a reachable target,
/// fragments get sealed and skipped, and each run only touches the small
/// uncompacted tail.
const TARGET_ROWS_PER_FRAGMENT: usize = 4096;

/// Version retention for the prune half of `optimize_all`. Every
/// `merge_insert` mints a dataset version, so a big indexing run leaves
/// thousands behind; LanceDB's default retention (7 days) means none of
/// them are reclaimable while the run is still going. Half an hour is well
/// past any in-flight read on a single-process desktop backend.
const PRUNE_OLDER_THAN_MINUTES: i64 = 30;

/// Cap for LanceDB's per-session index cache. Lance's own default is
/// **6 GiB** (`lance::dataset::DEFAULT_INDEX_CACHE_SIZE`), sized for
/// server deployments; we run alongside Lightroom on a user's desktop,
/// where a cache that big is indistinguishable from a leak — RSS just
/// climbs until the OS kills the process. The cache is a pure
/// speed/memory tradeoff (a miss re-reads the index from disk), and our
/// only index is the small scalar BTree on `id`, so a modest cap costs
/// almost nothing.
const INDEX_CACHE_BYTES: usize = 128 * 1024 * 1024;

/// Cap for LanceDB's per-session file-metadata cache (Lance's default is
/// 1 GiB). Same reasoning as `INDEX_CACHE_BYTES`; this one grows with the
/// number of data files, which a long indexing run mints constantly.
const METADATA_CACHE_BYTES: usize = 128 * 1024 * 1024;

fn cache_bytes_from_env(var: &str, default: usize) -> usize {
    std::env::var(var)
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .map(|mb| mb * 1024 * 1024)
        .unwrap_or(default)
}

fn compaction_options(target_rows: usize) -> CompactionOptions {
    CompactionOptions {
        target_rows_per_fragment: target_rows,
        ..Default::default()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("lancedb error: {0}")]
    Lance(#[from] lancedb::Error),
    #[error("arrow error: {0}")]
    Arrow(#[from] arrow_schema::ArrowError),
    #[error("unknown table: {0}")]
    UnknownTable(String),
    #[error("dimension mismatch in {table}: expected {expected}, got {got} for id {id}")]
    DimensionMismatch {
        table: String,
        expected: i32,
        got: usize,
        id: String,
    },
}

pub type Result<T> = std::result::Result<T, StoreError>;

/// What one `Store::optimize_all` pass actually did, aggregated over all
/// tables. Exists so the maintenance loop can log whether compaction is
/// doing bounded work (a handful of fragments per pass) or has regressed
/// to rewriting the whole catalog again.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct OptimizeSummary {
    pub fragments_removed: usize,
    pub fragments_added: usize,
    pub old_versions_removed: usize,
    /// Bytes held by LanceDB's index + file-metadata caches afterwards.
    pub cache_bytes: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StoreRecord {
    pub id: String,
    /// None = metadata-only record (Chroma's zero-vector + has_embedding=False).
    pub vector: Option<Vec<f32>>,
    pub metadata: Map<String, Value>,
}

pub struct Store {
    conn: lancedb::Connection,
    /// The session whose caches `conn` uses — held so `session_bytes()`
    /// can report how much memory LanceDB's caches are currently holding.
    session: Arc<lancedb::Session>,
    /// Counts `upsert`/`append` calls since the last `optimize_all`. Each
    /// call mints a new fragment + dataset version regardless of how many
    /// rows it carries, so this is a proxy for how much unpruned/
    /// uncompacted state has piled up. Checked by the maintenance loop
    /// (`lrg-server::maintenance_loop` via `AppState::run_maintenance`)
    /// against `COMPACT_WRITE_THRESHOLD` — a purely wall-clock interval
    /// isn't enough on its own: a fast indexing run can mint thousands of
    /// fragments/versions well before the next tick, which is what let
    /// RSS climb unbounded (OOM around ~3600 photos) even with the
    /// interval-only version of this maintenance loop.
    write_ops: AtomicU64,
}

fn table_dim(table: &str) -> Result<i32> {
    TABLES
        .iter()
        .find(|(name, _)| *name == table)
        .map(|(_, dim)| *dim)
        .ok_or_else(|| StoreError::UnknownTable(table.to_string()))
}

fn table_schema(dim: i32) -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new(
            "vector",
            DataType::FixedSizeList(Arc::new(Field::new("item", DataType::Float32, true)), dim),
            true,
        ),
        Field::new("metadata", DataType::Utf8, true),
    ]))
}

fn sql_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

fn records_to_batch(table: &str, records: &[StoreRecord]) -> Result<RecordBatch> {
    let dim = table_dim(table)?;
    let mut ids = StringBuilder::new();
    let mut vectors = FixedSizeListBuilder::new(Float32Builder::new(), dim);
    let mut metadatas = StringBuilder::new();

    for record in records {
        ids.append_value(&record.id);
        match &record.vector {
            Some(v) => {
                if v.len() != dim as usize {
                    return Err(StoreError::DimensionMismatch {
                        table: table.to_string(),
                        expected: dim,
                        got: v.len(),
                        id: record.id.clone(),
                    });
                }
                vectors.values().append_slice(v);
                vectors.append(true);
            }
            None => {
                vectors.values().append_nulls(dim as usize);
                vectors.append(false);
            }
        }
        metadatas.append_value(Value::Object(record.metadata.clone()).to_string());
    }

    Ok(RecordBatch::try_new(
        table_schema(dim),
        vec![
            Arc::new(ids.finish()),
            Arc::new(vectors.finish()),
            Arc::new(metadatas.finish()),
        ],
    )?)
}

fn batch_to_records(batch: &RecordBatch) -> Vec<StoreRecord> {
    let ids = batch
        .column_by_name("id")
        .and_then(|c| c.as_any().downcast_ref::<StringArray>());
    let vectors = batch
        .column_by_name("vector")
        .and_then(|c| c.as_any().downcast_ref::<FixedSizeListArray>());
    let metadatas = batch
        .column_by_name("metadata")
        .and_then(|c| c.as_any().downcast_ref::<StringArray>());

    let Some(ids) = ids else { return Vec::new() };
    let mut out = Vec::with_capacity(ids.len());
    for row in 0..ids.len() {
        let vector = vectors.and_then(|va| {
            if va.is_null(row) {
                None
            } else {
                let values = va.value(row);
                values
                    .as_any()
                    .downcast_ref::<Float32Array>()
                    .map(|f| f.values().to_vec())
            }
        });
        let metadata = metadatas
            .filter(|m| !m.is_null(row))
            .and_then(|m| serde_json::from_str::<Value>(m.value(row)).ok())
            .and_then(|v| match v {
                Value::Object(map) => Some(map),
                _ => None,
            })
            .unwrap_or_default();
        out.push(StoreRecord {
            id: ids.value(row).to_string(),
            vector,
            metadata,
        });
    }
    out
}

impl Store {
    /// Open (or create) the store at `root`, ensuring all four tables exist.
    pub async fn open(root: &Path) -> Result<Store> {
        // Explicit session: without one, lancedb builds a `Session::default()`
        // whose caches are capped at 6 GiB + 1 GiB. See `INDEX_CACHE_BYTES`.
        let session = Arc::new(lancedb::Session::new(
            cache_bytes_from_env("GENIUSAI_LANCE_INDEX_CACHE_MB", INDEX_CACHE_BYTES),
            cache_bytes_from_env("GENIUSAI_LANCE_METADATA_CACHE_MB", METADATA_CACHE_BYTES),
            Arc::new(lancedb::ObjectStoreRegistry::default()),
        ));
        let conn = lancedb::connect(&root.to_string_lossy())
            .session(session.clone())
            .execute()
            .await?;
        let existing = conn.table_names().execute().await?;
        for (name, dim) in TABLES {
            if !existing.iter().any(|t| t == name) {
                conn.create_empty_table(name, table_schema(dim))
                    .execute()
                    .await?;
            }
        }
        let store = Store {
            conn,
            session,
            write_ops: AtomicU64::new(0),
        };
        store.ensure_id_indices().await?;
        Ok(store)
    }

    async fn table(&self, name: &str) -> Result<lancedb::Table> {
        table_dim(name)?; // validate name
        Ok(self.conn.open_table(name).execute().await?)
    }

    /// `upsert`'s `merge_insert` matches existing rows by scanning for the
    /// `id` key; without an index that's a full table scan on every single
    /// call. As a catalog grows, each subsequent per-photo upsert during a
    /// big indexing run gets more expensive than the last — a scalar index
    /// turns that into a lookup. Idempotent (skips tables that already have
    /// one), so re-binding an already-indexed catalog is a no-op.
    async fn ensure_id_indices(&self) -> Result<()> {
        for (name, _) in TABLES {
            let tbl = self.table(name).await?;
            let has_id_index = tbl
                .list_indices()
                .await?
                .iter()
                .any(|idx| idx.columns.iter().any(|c| c == "id"));
            if !has_id_index {
                tbl.create_index(&["id"], Index::BTree(BTreeIndexBuilder::default()))
                    .execute()
                    .await?;
            }
        }
        Ok(())
    }

    /// Compacts small fragments, folds new rows into the `id` index, and
    /// prunes old dataset versions on every table, then resets the
    /// `write_ops` counter. `merge_insert`/`add` each mint a new fragment +
    /// dataset version, so an indexing-heavy catalog accumulates both
    /// quickly. Called periodically from a background task (see
    /// `lrg-server`'s maintenance loop), not per-request — compaction
    /// scans and rewrites files, so it's too costly to run inline with
    /// every batch.
    ///
    /// This deliberately does not use `OptimizeAction::All`: that runs
    /// compaction with `CompactionOptions::default()`, whose 1M-row
    /// fragment target no Lightroom catalog reaches, so no fragment is
    /// ever considered "done" and every run rewrites the whole table.
    /// Spelling the three steps out lets us bound the compaction target
    /// and shorten version retention.
    pub async fn optimize_all(&self) -> Result<OptimizeSummary> {
        self.optimize_all_with_target(TARGET_ROWS_PER_FRAGMENT)
            .await
    }

    /// `optimize_all` with an explicit fragment target, so tests can
    /// exercise the seal-and-skip behaviour on a handful of rows instead
    /// of thousands.
    async fn optimize_all_with_target(&self, target_rows: usize) -> Result<OptimizeSummary> {
        let mut summary = OptimizeSummary::default();
        for (name, _) in TABLES {
            let tbl = self.table(name).await?;
            let compaction = tbl
                .optimize(OptimizeAction::Compact {
                    options: compaction_options(target_rows),
                    remap_options: None,
                })
                .await?;
            // Rows written since the last run aren't in the BTree yet;
            // without this they'd be found by a scan of the unindexed
            // tail, which is exactly what the index exists to avoid.
            tbl.optimize(OptimizeAction::Index(OptimizeOptions::default()))
                .await?;
            let prune = tbl
                .optimize(OptimizeAction::Prune {
                    older_than: Some(Duration::minutes(PRUNE_OLDER_THAN_MINUTES)),
                    delete_unverified: None,
                    error_if_tagged_old_versions: None,
                })
                .await?;
            if let Some(metrics) = &compaction.compaction {
                summary.fragments_removed += metrics.fragments_removed;
                summary.fragments_added += metrics.fragments_added;
            }
            if let Some(removal) = &prune.prune {
                summary.old_versions_removed += removal.old_versions as usize;
            }
            log::debug!(
                "LanceDB optimize({name}): compaction={:?} prune={:?}",
                compaction.compaction,
                prune.prune
            );
        }
        summary.cache_bytes = self.session_bytes();
        self.write_ops.store(0, Ordering::Relaxed);
        Ok(summary)
    }

    /// Number of `upsert`/`append` calls since the last successful
    /// `optimize_all` — see the `write_ops` field doc for why this exists.
    pub fn pending_write_ops(&self) -> u64 {
        self.write_ops.load(Ordering::Relaxed)
    }

    /// Bytes currently held by LanceDB's index + file-metadata caches.
    /// Walks the caches, so it's for diagnostics (`/db/stats`, the
    /// maintenance log line) — not for a hot path.
    pub fn session_bytes(&self) -> u64 {
        self.session.size_bytes()
    }

    /// Insert-or-replace by id (Chroma add/update semantics).
    pub async fn upsert(&self, table: &str, records: &[StoreRecord]) -> Result<()> {
        if records.is_empty() {
            return Ok(());
        }
        let batch = records_to_batch(table, records)?;
        let schema = batch.schema();
        let reader = RecordBatchIterator::new([Ok(batch)], schema);
        let tbl = self.table(table).await?;
        let mut builder = tbl.merge_insert(&["id"]);
        builder
            .when_matched_update_all(None)
            .when_not_matched_insert_all();
        builder
            .execute(Box::new(reader) as Box<dyn arrow_array::RecordBatchReader + Send>)
            .await?;
        self.write_ops.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// Fast path for bulk-loading into an empty table (migration).
    pub async fn append(&self, table: &str, records: &[StoreRecord]) -> Result<()> {
        if records.is_empty() {
            return Ok(());
        }
        let batch = records_to_batch(table, records)?;
        let tbl = self.table(table).await?;
        tbl.add(batch).execute().await?;
        self.write_ops.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    pub async fn get(&self, table: &str, ids: &[String]) -> Result<Vec<StoreRecord>> {
        let tbl = self.table(table).await?;
        let mut out = Vec::new();
        for chunk in ids.chunks(GET_IDS_CHUNK_SIZE) {
            let list = chunk
                .iter()
                .map(|s| sql_quote(s))
                .collect::<Vec<_>>()
                .join(",");
            let batches: Vec<RecordBatch> = tbl
                .query()
                .only_if(format!("id IN ({list})"))
                .execute()
                .await?
                .try_collect()
                .await?;
            for batch in &batches {
                out.extend(batch_to_records(batch));
            }
        }
        Ok(out)
    }

    pub async fn delete(&self, table: &str, ids: &[String]) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        let tbl = self.table(table).await?;
        for chunk in ids.chunks(GET_IDS_CHUNK_SIZE) {
            let list = chunk
                .iter()
                .map(|s| sql_quote(s))
                .collect::<Vec<_>>()
                .join(",");
            tbl.delete(&format!("id IN ({list})")).await?;
        }
        Ok(())
    }

    /// Deletes every row whose `id` starts with `prefix` (a plain `DELETE
    /// ... WHERE id LIKE 'prefix%'`) without ever pulling the matched rows'
    /// `metadata` into the Rust process. Used to clear a photo's stale
    /// FACE_TABLE rows (ids are `{photo_id}_{n}`) before re-inserting fresh
    /// ones — the previous `scan_meta` + filter approach decoded every
    /// face's metadata blob (including its base64 thumbnail) for the whole
    /// table on every single indexed photo, which is what drove unbounded
    /// RSS growth on large face-indexing runs.
    pub async fn delete_by_id_prefix(&self, table: &str, prefix: &str) -> Result<()> {
        let tbl = self.table(table).await?;
        let escaped = prefix
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_")
            .replace('\'', "''");
        tbl.delete(&format!("id LIKE '{escaped}%' ESCAPE '\\'"))
            .await?;
        Ok(())
    }

    /// Like `get`, but returns only id + metadata (no vector deserialization).
    pub async fn get_meta(
        &self,
        table: &str,
        ids: &[String],
    ) -> Result<Vec<(String, Map<String, Value>)>> {
        let tbl = self.table(table).await?;
        let mut out = Vec::new();
        for chunk in ids.chunks(GET_IDS_CHUNK_SIZE) {
            let list = chunk
                .iter()
                .map(|s| sql_quote(s))
                .collect::<Vec<_>>()
                .join(",");
            let batches: Vec<RecordBatch> = tbl
                .query()
                .only_if(format!("id IN ({list})"))
                .select(lancedb::query::Select::Columns(vec![
                    "id".to_string(),
                    "metadata".to_string(),
                ]))
                .execute()
                .await?
                .try_collect()
                .await?;
            for batch in &batches {
                for record in batch_to_records(batch) {
                    out.push((record.id, record.metadata));
                }
            }
        }
        Ok(out)
    }

    /// Every row's id, and nothing else — no vector, no metadata (so no
    /// base64 thumbnail decode on FACE_TABLE). For callers that only need
    /// id membership/existence, this is far cheaper than `scan_meta` on a
    /// table whose metadata blobs are large.
    pub async fn scan_ids(&self, table: &str) -> Result<Vec<String>> {
        let tbl = self.table(table).await?;
        let batches: Vec<RecordBatch> = tbl
            .query()
            .select(lancedb::query::Select::Columns(vec!["id".to_string()]))
            .execute()
            .await?
            .try_collect()
            .await?;
        let mut out = Vec::new();
        for batch in &batches {
            if let Some(ids) = batch
                .column_by_name("id")
                .and_then(|c| c.as_any().downcast_ref::<StringArray>())
            {
                for i in 0..ids.len() {
                    out.push(ids.value(i).to_string());
                }
            }
        }
        Ok(out)
    }

    /// Stream id + metadata for every row (no vectors), the workhorse behind
    /// get_all_image_ids / sync_cleanup / stats.
    pub async fn scan_meta(&self, table: &str) -> Result<Vec<(String, Map<String, Value>)>> {
        let tbl = self.table(table).await?;
        let batches: Vec<RecordBatch> = tbl
            .query()
            .select(lancedb::query::Select::Columns(vec![
                "id".to_string(),
                "metadata".to_string(),
            ]))
            .execute()
            .await?
            .try_collect()
            .await?;
        let mut out = Vec::new();
        for batch in &batches {
            for record in batch_to_records(batch) {
                out.push((record.id, record.metadata));
            }
        }
        Ok(out)
    }

    /// Full table scan including vectors — used by face clustering and
    /// face-similarity queries, both brute-force over what is normally a
    /// few hundred to a few thousand rows per catalog.
    pub async fn scan_all(&self, table: &str) -> Result<Vec<StoreRecord>> {
        let tbl = self.table(table).await?;
        let batches: Vec<RecordBatch> = tbl.query().execute().await?.try_collect().await?;
        let mut out = Vec::new();
        for batch in &batches {
            out.extend(batch_to_records(batch));
        }
        Ok(out)
    }

    pub async fn count(&self, table: &str) -> Result<usize> {
        let tbl = self.table(table).await?;
        Ok(tbl.count_rows(None).await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn record(id: &str, vector: Option<Vec<f32>>, extra: Value) -> StoreRecord {
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

    #[tokio::test]
    async fn roundtrip_upsert_get_delete() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).await.unwrap();

        let v: Vec<f32> = (0..512).map(|i| i as f32 * 0.5).collect();
        let records = vec![
            record("face_a", Some(v.clone()), json!({"det_score": 0.9})),
            record("face_b", None, json!({"has_embedding": false})),
        ];
        store.upsert(FACE_TABLE, &records).await.unwrap();

        let got = store
            .get(
                FACE_TABLE,
                &["face_a".into(), "face_b".into(), "missing".into()],
            )
            .await
            .unwrap();
        assert_eq!(got.len(), 2);
        let a = got.iter().find(|r| r.id == "face_a").unwrap();
        assert_eq!(a.vector.as_ref().unwrap(), &v);
        assert_eq!(a.metadata["det_score"], json!(0.9));
        let b = got.iter().find(|r| r.id == "face_b").unwrap();
        assert!(
            b.vector.is_none(),
            "metadata-only record must have NULL vector"
        );

        // Upsert replaces in place.
        store
            .upsert(
                FACE_TABLE,
                &[record("face_a", Some(v.clone()), json!({"det_score": 0.1}))],
            )
            .await
            .unwrap();
        assert_eq!(store.count(FACE_TABLE).await.unwrap(), 2);
        let got = store.get(FACE_TABLE, &["face_a".into()]).await.unwrap();
        assert_eq!(got[0].metadata["det_score"], json!(0.1));

        store.delete(FACE_TABLE, &["face_a".into()]).await.unwrap();
        assert_eq!(store.count(FACE_TABLE).await.unwrap(), 1);
    }

    #[tokio::test]
    async fn write_ops_counts_calls_not_rows_and_resets_on_optimize() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).await.unwrap();
        assert_eq!(store.pending_write_ops(), 0);

        // One append call carrying many rows still counts as a single
        // write op — fragments are minted per call, not per row.
        let records: Vec<StoreRecord> = (0..10)
            .map(|i| record(&format!("p{i}"), Some(vec![0.0; 1152]), json!({})))
            .collect();
        store.append(IMAGE_TABLE, &records).await.unwrap();
        assert_eq!(store.pending_write_ops(), 1);

        for i in 0..5 {
            store
                .upsert(
                    IMAGE_TABLE,
                    &[record(&format!("q{i}"), Some(vec![0.0; 1152]), json!({}))],
                )
                .await
                .unwrap();
        }
        assert_eq!(store.pending_write_ops(), 6);

        store.optimize_all().await.unwrap();
        assert_eq!(store.pending_write_ops(), 0);
    }

    /// Simulates a slow indexing run at a given fragment target: seed the
    /// table with `rows` single-row appends (what per-photo indexing
    /// actually produces), then alternate "three more photos, one
    /// maintenance pass" a few times to reach steady state. Returns how
    /// many fragments the table ends up split across.
    async fn fragments_after_indexing_run(target_rows: usize, rows: usize) -> usize {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).await.unwrap();
        let mut next_id = 0;
        let append = async |n: usize, next_id: &mut usize| {
            for _ in 0..n {
                store
                    .append(
                        FACE_TABLE,
                        &[record(
                            &format!("f{next_id}"),
                            Some(vec![0.0; 512]),
                            json!({}),
                        )],
                    )
                    .await
                    .unwrap();
                *next_id += 1;
            }
        };

        append(rows, &mut next_id).await;
        store.optimize_all_with_target(target_rows).await.unwrap();
        for _ in 0..3 {
            append(3, &mut next_id).await;
            store.optimize_all_with_target(target_rows).await.unwrap();
        }
        assert_eq!(store.count(FACE_TABLE).await.unwrap(), next_id);
        store
            .table(FACE_TABLE)
            .await
            .unwrap()
            .stats()
            .await
            .unwrap()
            .fragment_stats
            .num_fragments
    }

    /// Guards the fix for the indexing memory blowup: compaction must
    /// leave already-compacted fragments alone.
    ///
    /// Lance's default fragment target is 1,048,576 rows. No Lightroom
    /// catalog puts that many rows in one of our tables, so under the
    /// default *every* fragment stayed a compaction candidate forever and
    /// each maintenance pass re-read and rewrote the entire table — work
    /// that grew with the catalog, ran every dozen photos, and drove RSS
    /// up until the OS killed the process.
    ///
    /// How many fragments the table ends up split across is the visible
    /// proxy: sealed fragments survive a pass, so with a reachable target
    /// their number tracks the row count, while under the default target
    /// the table is repeatedly collapsed back into a single fragment —
    /// which is only possible by re-reading and rewriting all of it.
    #[tokio::test]
    async fn compaction_leaves_sealed_fragments_alone() {
        // Sealing at 4 rows: ~24 rows have to live in several fragments,
        // and each maintenance pass leaves the full ones alone.
        let bounded = fragments_after_indexing_run(4, 24).await;
        assert!(
            bounded >= 6,
            "with a reachable fragment target, sealed fragments should \
             survive each pass; found only {bounded} fragment(s), meaning \
             compaction is still rewriting everything"
        );

        // Same run at Lance's default target: nothing ever reaches it, so
        // every pass folds the accumulated data back into one fragment.
        // That repeated re-reading of already-compacted data is what
        // scaled with catalog size and blew up memory.
        let default = fragments_after_indexing_run(1024 * 1024, 24).await;
        assert!(
            default < bounded,
            "sanity check on the unfixed behaviour: the default target \
             should keep collapsing the table (got {default} fragment(s) \
             vs {bounded}) — if it no longer does, the bounded target above \
             is not what makes the difference"
        );
    }

    #[tokio::test]
    async fn scan_meta_returns_all_rows_without_vectors() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).await.unwrap();
        let records: Vec<StoreRecord> = (0..25)
            .map(|i| {
                record(
                    &format!("p{i}"),
                    Some(vec![0.0; 1152]),
                    json!({"catalog_ids": "[\"cat_1\"]", "n": i}),
                )
            })
            .collect();
        store.append(IMAGE_TABLE, &records).await.unwrap();

        let rows = store.scan_meta(IMAGE_TABLE).await.unwrap();
        assert_eq!(rows.len(), 25);
        assert!(rows
            .iter()
            .all(|(_, m)| m["catalog_ids"] == json!("[\"cat_1\"]")));

        let mut ids = store.scan_ids(IMAGE_TABLE).await.unwrap();
        ids.sort();
        let mut expected: Vec<String> = (0..25).map(|i| format!("p{i}")).collect();
        expected.sort();
        assert_eq!(ids, expected);
    }

    #[tokio::test]
    async fn ids_with_quotes_are_escaped() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).await.unwrap();
        let odd_id = "photo'; DROP TABLE x --";
        store
            .upsert(IMAGE_TABLE, &[record(odd_id, None, json!({}))])
            .await
            .unwrap();
        let got = store.get(IMAGE_TABLE, &[odd_id.into()]).await.unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].id, odd_id);
        store.delete(IMAGE_TABLE, &[odd_id.into()]).await.unwrap();
        assert_eq!(store.count(IMAGE_TABLE).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn delete_by_id_prefix_matches_only_the_prefix_and_treats_underscore_literally() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).await.unwrap();
        // "photo_1" as a photo_id, plus a real face id under it, a
        // lookalike face id under "photo_10" (must NOT be caught by the
        // "photo_1_" prefix), and an unrelated row.
        store
            .append(
                FACE_TABLE,
                &[
                    record(
                        "photo_1_0",
                        Some(vec![0.0; 512]),
                        json!({"photo_id": "photo_1"}),
                    ),
                    record(
                        "photo_1_1",
                        Some(vec![0.0; 512]),
                        json!({"photo_id": "photo_1"}),
                    ),
                    record(
                        "photo_10_0",
                        Some(vec![0.0; 512]),
                        json!({"photo_id": "photo_10"}),
                    ),
                    record(
                        "other_0",
                        Some(vec![0.0; 512]),
                        json!({"photo_id": "other"}),
                    ),
                ],
            )
            .await
            .unwrap();

        store
            .delete_by_id_prefix(FACE_TABLE, "photo_1_")
            .await
            .unwrap();

        let remaining: Vec<String> = store
            .scan_meta(FACE_TABLE)
            .await
            .unwrap()
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        assert_eq!(remaining.len(), 2);
        assert!(remaining.contains(&"photo_10_0".to_string()));
        assert!(remaining.contains(&"other_0".to_string()));
    }
}
