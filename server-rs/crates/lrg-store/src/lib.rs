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
use std::sync::Arc;

use arrow_array::builder::{FixedSizeListBuilder, Float32Builder, StringBuilder};
use arrow_array::{
    Array, FixedSizeListArray, Float32Array, RecordBatch, RecordBatchIterator, StringArray,
};
use arrow_schema::{DataType, Field, Schema};
use futures::TryStreamExt;
use lancedb::query::{ExecutableQuery, QueryBase};
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

#[derive(Debug, Clone, PartialEq)]
pub struct StoreRecord {
    pub id: String,
    /// None = metadata-only record (Chroma's zero-vector + has_embedding=False).
    pub vector: Option<Vec<f32>>,
    pub metadata: Map<String, Value>,
}

pub struct Store {
    conn: lancedb::Connection,
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
        let conn = lancedb::connect(&root.to_string_lossy()).execute().await?;
        let existing = conn.table_names().execute().await?;
        for (name, dim) in TABLES {
            if !existing.iter().any(|t| t == name) {
                conn.create_empty_table(name, table_schema(dim))
                    .execute()
                    .await?;
            }
        }
        Ok(Store { conn })
    }

    async fn table(&self, name: &str) -> Result<lancedb::Table> {
        table_dim(name)?; // validate name
        Ok(self.conn.open_table(name).execute().await?)
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
}
