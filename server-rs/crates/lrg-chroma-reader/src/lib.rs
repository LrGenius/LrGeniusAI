//! Read-only extraction of photo records (ids, vectors, metadata) from a
//! ChromaDB 1.x persistent directory, used by the one-time migration to
//! LanceDB. Never writes to the source directory.
//!
//! Verified against chromadb 1.5.8 (the version pinned in server/uv.lock):
//!
//! - `chroma.sqlite3` holds collections, segments, the live id set
//!   (`embeddings` joined per METADATA segment), typed metadata
//!   (`embedding_metadata`), and the write-ahead log (`embeddings_queue`)
//!   with raw FLOAT32 little-endian vector blobs.
//! - Flushed vectors live in the VECTOR segment directory in Chroma's
//!   hnswlib-fork split persistence: `header.bin` (fixed 100-byte field
//!   block), `data_level0.bin` (fixed-stride records: links, vector,
//!   u64 label), and `index_metadata.pickle` (`id_to_label` map).
//! - The WAL is purged only up to the last flush, so a real database is
//!   always HNSW records + a WAL tail. Vector resolution order:
//!   WAL (latest op per id) first, HNSW second.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::Connection;
use serde_json::{Map, Value};

#[derive(Debug, thiserror::Error)]
pub enum ChromaReadError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("io error reading {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("pickle error in {path}: {reason}")]
    Pickle { path: PathBuf, reason: String },
    #[error("invalid hnsw header in {path}: {reason}")]
    HnswHeader { path: PathBuf, reason: String },
    #[error("chroma.sqlite3 not found in {0}")]
    NotAChromaDir(PathBuf),
}

pub struct ChromaRecord {
    pub id: String,
    /// None when no vector could be recovered from either the WAL or the
    /// HNSW segment (the caller decides whether that needs re-embedding).
    pub embedding: Option<Vec<f32>>,
    pub metadata: Map<String, Value>,
}

pub struct ChromaCollection {
    pub name: String,
    pub dimension: Option<u32>,
    pub records: Vec<ChromaRecord>,
}

/// Read every collection in a Chroma persistent directory.
pub fn read_chroma_dir(db_dir: &Path) -> Result<Vec<ChromaCollection>, ChromaReadError> {
    let sqlite_path = db_dir.join("chroma.sqlite3");
    if !sqlite_path.is_file() {
        return Err(ChromaReadError::NotAChromaDir(db_dir.to_path_buf()));
    }
    let conn =
        Connection::open_with_flags(&sqlite_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;

    let collections: Vec<(String, String, Option<u32>)> = conn
        .prepare("SELECT id, name, dimension FROM collections")?
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
        .collect::<Result<_, _>>()?;

    let mut result = Vec::new();
    for (collection_id, name, dimension) in collections {
        result.push(read_collection(
            &conn,
            db_dir,
            &collection_id,
            name,
            dimension,
        )?);
    }
    Ok(result)
}

fn read_collection(
    conn: &Connection,
    db_dir: &Path,
    collection_id: &str,
    name: String,
    dimension: Option<u32>,
) -> Result<ChromaCollection, ChromaReadError> {
    // Segment ids: METADATA scopes the live id set, VECTOR names the HNSW dir.
    let mut metadata_segment = None;
    let mut vector_segment = None;
    {
        let mut stmt = conn.prepare("SELECT id, scope FROM segments WHERE collection = ?1")?;
        let rows = stmt.query_map([collection_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        for row in rows {
            let (id, scope) = row?;
            match scope.as_str() {
                "METADATA" => metadata_segment = Some(id),
                "VECTOR" => vector_segment = Some(id),
                _ => {}
            }
        }
    }

    // Live records: rowid (metadata join key) + user-facing embedding_id.
    let mut records: Vec<(i64, String)> = Vec::new();
    if let Some(seg) = &metadata_segment {
        let mut stmt = conn
            .prepare("SELECT id, embedding_id FROM embeddings WHERE segment_id = ?1 ORDER BY id")?;
        let rows = stmt.query_map([seg], |row| Ok((row.get(0)?, row.get(1)?)))?;
        for row in rows {
            records.push(row?);
        }
    }

    // Typed metadata per rowid.
    let mut metadata_by_rowid: HashMap<i64, Map<String, Value>> = HashMap::new();
    if let Some(seg) = &metadata_segment {
        let mut stmt = conn.prepare(
            "SELECT em.id, em.key, em.string_value, em.int_value, em.float_value, em.bool_value
             FROM embedding_metadata em
             JOIN embeddings e ON e.id = em.id
             WHERE e.segment_id = ?1",
        )?;
        let rows = stmt.query_map([seg], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<i64>>(3)?,
                row.get::<_, Option<f64>>(4)?,
                row.get::<_, Option<i64>>(5)?,
            ))
        })?;
        for row in rows {
            let (rowid, key, s, i, f, b) = row?;
            if key.starts_with("chroma:") {
                continue;
            }
            let value = if let Some(s) = s {
                Value::String(s)
            } else if let Some(b) = b {
                Value::Bool(b != 0)
            } else if let Some(i) = i {
                Value::from(i)
            } else if let Some(f) = f {
                Value::from(f)
            } else {
                Value::Null
            };
            metadata_by_rowid
                .entry(rowid)
                .or_default()
                .insert(key, value);
        }
    }

    // Vector sources: WAL tail first (newest state), HNSW segment for the rest.
    let wal_vectors = read_wal_vectors(conn, collection_id)?;
    let hnsw_vectors = match &vector_segment {
        Some(seg) => read_hnsw_vectors(&db_dir.join(seg))?,
        None => HashMap::new(),
    };

    let records = records
        .into_iter()
        .map(|(rowid, embedding_id)| {
            let embedding = wal_vectors
                .get(&embedding_id)
                .cloned()
                .flatten()
                .or_else(|| hnsw_vectors.get(&embedding_id).cloned());
            ChromaRecord {
                id: embedding_id,
                embedding,
                metadata: metadata_by_rowid.remove(&rowid).unwrap_or_default(),
            }
        })
        .collect();

    Ok(ChromaCollection {
        name,
        dimension,
        records,
    })
}

/// Replay the `embeddings_queue` WAL in sequence order. Returns the latest
/// vector state per id: `Some(vec)` after an add/update/upsert with a
/// vector, `None` after a delete (or a metadata-only operation).
fn read_wal_vectors(
    conn: &Connection,
    collection_id: &str,
) -> Result<HashMap<String, Option<Vec<f32>>>, ChromaReadError> {
    const OP_DELETE: i64 = 3;

    let mut map: HashMap<String, Option<Vec<f32>>> = HashMap::new();
    let mut stmt = conn.prepare(
        "SELECT operation, id, vector, encoding FROM embeddings_queue
         WHERE topic LIKE '%' || ?1 ORDER BY seq_id",
    )?;
    let rows = stmt.query_map([collection_id], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Option<Vec<u8>>>(2)?,
            row.get::<_, Option<String>>(3)?,
        ))
    })?;
    for row in rows {
        let (op, id, blob, encoding) = row?;
        if op == OP_DELETE {
            map.insert(id, None);
            continue;
        }
        if let Some(blob) = blob {
            if encoding.as_deref() != Some("FLOAT32") {
                continue; // unknown encoding: leave resolution to the HNSW path
            }
            map.insert(id, Some(f32_le_vec(&blob)));
        }
    }
    Ok(map)
}

fn f32_le_vec(blob: &[u8]) -> Vec<f32> {
    blob.chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

/// Fixed 100-byte header written by Chroma's hnswlib fork. Field offsets
/// verified against chromadb 1.5.8 output (see crate docs).
struct HnswHeader {
    cur_element_count: u64,
    size_data_per_element: u64,
    label_offset: u64,
    offset_data: u64,
}

fn parse_hnsw_header(path: &Path, bytes: &[u8]) -> Result<HnswHeader, ChromaReadError> {
    let err = |reason: String| ChromaReadError::HnswHeader {
        path: path.to_path_buf(),
        reason,
    };
    if bytes.len() < 100 {
        return Err(err(format!("expected 100 bytes, got {}", bytes.len())));
    }
    let u64_at = |off: usize| u64::from_le_bytes(bytes[off..off + 8].try_into().unwrap());

    let header = HnswHeader {
        cur_element_count: u64_at(20),
        size_data_per_element: u64_at(28),
        label_offset: u64_at(36),
        offset_data: u64_at(44),
    };
    // Record layout must be [links][vector][u64 label]; anything else means
    // a format we haven't verified — fail loudly rather than misread floats.
    if header.label_offset + 8 != header.size_data_per_element {
        return Err(err(
            "label is not the trailing 8 bytes of each record".to_string()
        ));
    }
    if header.offset_data >= header.label_offset {
        return Err(err("vector data does not precede the label".to_string()));
    }
    Ok(header)
}

/// Read flushed vectors from a VECTOR segment dir, keyed by user id via the
/// `id_to_label` map in index_metadata.pickle. An empty/never-flushed
/// segment (no pickle, cur_element_count == 0) yields an empty map.
fn read_hnsw_vectors(segment_dir: &Path) -> Result<HashMap<String, Vec<f32>>, ChromaReadError> {
    let pickle_path = segment_dir.join("index_metadata.pickle");
    let header_path = segment_dir.join("header.bin");
    let data_path = segment_dir.join("data_level0.bin");
    if !pickle_path.is_file() || !header_path.is_file() || !data_path.is_file() {
        return Ok(HashMap::new());
    }

    let io_err = |path: &Path| {
        let path = path.to_path_buf();
        move |source: std::io::Error| ChromaReadError::Io { path, source }
    };

    let header_bytes = fs::read(&header_path).map_err(io_err(&header_path))?;
    let header = parse_hnsw_header(&header_path, &header_bytes)?;
    let data = fs::read(&data_path).map_err(io_err(&data_path))?;

    let id_to_label = read_id_to_label(&pickle_path)?;
    let label_to_id: HashMap<u64, &String> = id_to_label.iter().map(|(id, l)| (*l, id)).collect();

    let stride = header.size_data_per_element as usize;
    let vec_bytes = (header.label_offset - header.offset_data) as usize;
    let mut result = HashMap::with_capacity(label_to_id.len());
    for i in 0..header.cur_element_count as usize {
        let base = i * stride;
        if base + stride > data.len() {
            break;
        }
        let label = u64::from_le_bytes(
            data[base + header.label_offset as usize..base + stride]
                .try_into()
                .unwrap(),
        );
        let Some(id) = label_to_id.get(&label) else {
            continue; // deleted element still occupying its slot
        };
        let start = base + header.offset_data as usize;
        result.insert((*id).clone(), f32_le_vec(&data[start..start + vec_bytes]));
    }
    Ok(result)
}

fn read_id_to_label(path: &Path) -> Result<HashMap<String, u64>, ChromaReadError> {
    let pickle_err = |reason: String| ChromaReadError::Pickle {
        path: path.to_path_buf(),
        reason,
    };
    let bytes = fs::read(path).map_err(|e| pickle_err(e.to_string()))?;
    let value: serde_pickle::Value =
        serde_pickle::value_from_slice(&bytes, serde_pickle::DeOptions::new())
            .map_err(|e| pickle_err(e.to_string()))?;

    let serde_pickle::Value::Dict(dict) = value else {
        return Err(pickle_err("top-level pickle is not a dict".into()));
    };
    let id_to_label = dict
        .iter()
        .find(|(k, _)| matches!(k, serde_pickle::HashableValue::String(s) if s == "id_to_label"))
        .map(|(_, v)| v)
        .ok_or_else(|| pickle_err("missing id_to_label".into()))?;
    let serde_pickle::Value::Dict(map) = id_to_label else {
        return Err(pickle_err("id_to_label is not a dict".into()));
    };

    let mut result = HashMap::with_capacity(map.len());
    for (k, v) in map {
        let serde_pickle::HashableValue::String(id) = k else {
            return Err(pickle_err("non-string id in id_to_label".into()));
        };
        let label = match v {
            serde_pickle::Value::I64(n) => *n as u64,
            other => return Err(pickle_err(format!("non-integer label for {id}: {other:?}"))),
        };
        result.insert(id.clone(), label);
    }
    Ok(result)
}
