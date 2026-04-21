import chromadb
from chromadb.config import Settings
import json
import numpy as np
from config import logger, CULLING_CONFIG, get_culling_config


# --- ChromaDB Client and Collection Initialization (Lazy) ---
chroma_client = None
collection = None
face_collection = None
vertex_collection = None


class DatabaseNotReadyError(Exception):
    """Raised when a database modification is attempted but the DB_PATH is not yet set."""

    pass


# InsightFace embeddings are 512-dimensional
FACE_EMBEDDING_DIM = 512
# Vertex AI Multimodal Embeddings (image) default dimension
VERTEX_EMBEDDING_DIM = 1408

# Max limit for get() when counting; Chroma may apply a default limit otherwise
STATS_GET_LIMIT = 2_000_000

PHOTO_ID_FIELD = "photo_id"
LEGACY_UUID_FIELD = "uuid"
CATALOG_IDS_FIELD = "catalog_ids"


def _parse_catalog_ids(metadata):
    """Parse catalog_ids from metadata (JSON list string). Return set of catalog id strings."""
    if not metadata:
        return set()
    raw = metadata.get(CATALOG_IDS_FIELD)
    if not raw:
        return set()
    if isinstance(raw, list):
        return set(str(x) for x in raw if x)
    try:
        parsed = json.loads(raw) if isinstance(raw, str) else raw
        return set(str(x) for x in parsed if x) if isinstance(parsed, list) else set()
    except (TypeError, ValueError):
        return set()


def _serialize_catalog_ids(catalog_ids_set):
    """Serialize set of catalog ids to JSON list string for ChromaDB metadata."""
    return json.dumps(sorted(catalog_ids_set)) if catalog_ids_set else "[]"


def _add_catalog_id(photo_id, catalog_id):
    """Ensure catalog_id is in the photo's catalog_ids list; update metadata only."""
    if not catalog_id or not photo_id:
        return
    _ensure_initialized()
    if collection is None:
        return
    data = collection.get(ids=[photo_id], include=["metadatas", "embeddings"])
    if not data or not data.get("ids"):
        return
    meta = dict(data["metadatas"][0]) if data.get("metadatas") else {}
    ids_set = _parse_catalog_ids(meta)
    ids_set.add(str(catalog_id).strip())
    meta[CATALOG_IDS_FIELD] = _serialize_catalog_ids(ids_set)
    meta = _ensure_photo_metadata(photo_id, meta)
    embedding = _first_result_item(data.get("embeddings"))
    if embedding is not None:
        collection.update(ids=[photo_id], metadatas=[meta], embeddings=[embedding])
    else:
        collection.update(ids=[photo_id], metadatas=[meta])


def _remove_catalog_id(photo_id, catalog_id):
    """Remove catalog_id from the photo's catalog_ids list; update metadata only. Does not delete the photo."""
    if not catalog_id or not photo_id:
        return
    _ensure_initialized()
    if collection is None:
        return
    data = collection.get(ids=[photo_id], include=["metadatas", "embeddings"])
    if not data or not data.get("ids"):
        return
    meta = dict(data["metadatas"][0]) if data.get("metadatas") else {}
    ids_set = _parse_catalog_ids(meta)
    ids_set.discard(str(catalog_id).strip())
    meta[CATALOG_IDS_FIELD] = _serialize_catalog_ids(ids_set)
    meta = _ensure_photo_metadata(photo_id, meta)
    embedding = _first_result_item(data.get("embeddings"))
    if embedding is not None:
        collection.update(ids=[photo_id], metadatas=[meta], embeddings=[embedding])
    else:
        collection.update(ids=[photo_id], metadatas=[meta])


def _normalize_photo_id(photo_id=None, legacy_uuid=None):
    pid = photo_id or legacy_uuid
    if pid is None:
        return None
    pid = str(pid).strip()
    return pid or None


def _ensure_photo_metadata(photo_id, metadata, legacy_uuid=None):
    out = dict(metadata or {})
    out[PHOTO_ID_FIELD] = photo_id
    # Keep legacy field for older clients/filters.
    out.setdefault(LEGACY_UUID_FIELD, legacy_uuid or photo_id)
    return out


def _first_result_item(values, default=None):
    """Return first item from Chroma results without truthiness checks."""
    if values is None:
        return default
    if isinstance(values, np.ndarray):
        if values.size == 0:
            return default
        return values[0]
    try:
        return values[0]
    except (IndexError, KeyError, TypeError):
        return default


def _ensure_initialized():
    """Initialize ChromaDB client and collections on first use (lazy loading)."""
    global chroma_client, collection, face_collection, vertex_collection
    if chroma_client is not None:
        return

    import config

    if not config.DB_PATH:
        logger.debug("ChromaDB initialization skipped: DB_PATH not set yet.")
        return

    logger.info(f"Initializing ChromaDB client at {config.DB_PATH} (lazy)...")
    chroma_client = chromadb.PersistentClient(
        path=config.DB_PATH, settings=Settings(anonymized_telemetry=False)
    )

    # Initialize image_embeddings collection
    try:
        collection = chroma_client.get_collection(name="image_embeddings")
        logger.info("Loaded existing ChromaDB image_embeddings collection.")
    except Exception:
        collection = chroma_client.create_collection(name="image_embeddings")
        logger.info("Created new ChromaDB image_embeddings collection.")

    # Initialize face_embeddings collection (second collection for face vectors)
    try:
        face_collection = chroma_client.get_collection(name="face_embeddings")
        logger.info("Loaded existing ChromaDB face_embeddings collection.")
    except Exception:
        face_collection = chroma_client.create_collection(name="face_embeddings")
        logger.info("Created new ChromaDB face_embeddings collection.")

    # Initialize image_embeddings_vertex (Google Vertex AI multimodal embeddings)
    try:
        vertex_collection = chroma_client.get_collection(name="image_embeddings_vertex")
        logger.info("Loaded existing ChromaDB image_embeddings_vertex collection.")
    except Exception:
        vertex_collection = chroma_client.create_collection(
            name="image_embeddings_vertex"
        )
        logger.info("Created new ChromaDB image_embeddings_vertex collection.")


def reset_chroma_client():
    """Reset the global ChromaDB client and collections so they can be re-initialized with a new DB_PATH."""
    global chroma_client, collection, face_collection, vertex_collection
    logger.info("Resetting ChromaDB client for re-initialization.")
    chroma_client = None
    collection = None
    face_collection = None
    vertex_collection = None


def unload_collections():
    """Unload the ChromaDB collections and client to free memory."""
    global chroma_client, collection, face_collection, vertex_collection
    if chroma_client is None:
        return
    logger.info("Unloading ChromaDB collections...")
    chroma_client = None
    collection = None
    face_collection = None
    vertex_collection = None
    import gc

    gc.collect()
    logger.info("Unloaded ChromaDB collections.")


def add_image(photo_id, embedding, metadata, *, legacy_uuid=None, catalog_id=None):
    """Add a new image record to the Chroma collection.

    embedding may be None for metadata-only records; in that case we add
    a dummy zero vector with the expected dimensionality (1152) to satisfy
    ChromaDB's requirements while still allowing metadata-only storage.

    Note: Metadata-only entries are marked with has_embedding=False in their
    metadata and are filtered out of semantic search results in service_search.py.
    They can still be found via metadata keyword searches.

    If catalog_id is provided, the photo is associated with that catalog (soft state).
    """
    _ensure_initialized()
    if collection is None:
        raise DatabaseNotReadyError(
            "Cannot add image: database not initialized (DB_PATH missing)."
        )
    photo_id = _normalize_photo_id(photo_id, legacy_uuid)
    if not photo_id:
        raise ValueError("photo_id is required")
    metadata = _ensure_photo_metadata(photo_id, metadata, legacy_uuid=legacy_uuid)
    if catalog_id:
        metadata[CATALOG_IDS_FIELD] = _serialize_catalog_ids({str(catalog_id).strip()})
    try:
        if embedding is None:
            # Add metadata-only record with a dummy zero embedding
            # The collection expects 1152-dimensional embeddings (from vision model)
            dummy_embedding = np.zeros(1152, dtype=np.float32).tolist()
            collection.add(
                embeddings=[dummy_embedding], metadatas=[metadata], ids=[photo_id]
            )
        else:
            collection.add(embeddings=[embedding], metadatas=[metadata], ids=[photo_id])
    except Exception as e:
        # Surface a helpful log message and re-raise so callers can decide what to do.
        logger.error(
            f"Failed to add image {photo_id} to ChromaDB (embedding provided: {embedding is not None}): {e}",
            exc_info=True,
        )
        raise


def update_image(
    photo_id, metadata, embedding=None, *, legacy_uuid=None, catalog_id=None
):
    _ensure_initialized()
    if collection is None:
        raise DatabaseNotReadyError(
            "Cannot update image: database not initialized (DB_PATH missing)."
        )
    photo_id = _normalize_photo_id(photo_id, legacy_uuid)
    if not photo_id:
        raise ValueError("photo_id is required")
    metadata = _ensure_photo_metadata(photo_id, metadata, legacy_uuid=legacy_uuid)
    if embedding is not None:
        collection.update(ids=[photo_id], metadatas=[metadata], embeddings=[embedding])
    else:
        collection.update(ids=[photo_id], metadatas=[metadata])
    if catalog_id:
        _add_catalog_id(photo_id, catalog_id)


def get_image(photo_id, *, legacy_uuid=None, catalog_id=None):
    _ensure_initialized()
    if collection is None:
        return {"ids": [], "metadatas": [], "embeddings": []}
    photo_id = _normalize_photo_id(photo_id, legacy_uuid)
    if not photo_id:
        return {"ids": [], "metadatas": [], "embeddings": []}
    data = collection.get(ids=[photo_id], include=["metadatas", "embeddings"])
    if catalog_id and data and data.get("ids"):
        meta = (data.get("metadatas") or [None])[0]
        ids_set = _parse_catalog_ids(meta)
        if str(catalog_id).strip() not in ids_set:
            return {"ids": [], "metadatas": [], "embeddings": []}
    return data


def delete_image(photo_id, *, legacy_uuid=None):
    _ensure_initialized()
    if collection is None:
        return
    photo_id = _normalize_photo_id(photo_id, legacy_uuid)
    if not photo_id:
        return
    collection.delete(ids=[photo_id])
    try:
        delete_vertex_image(photo_id)
    except Exception:
        pass


# Keys that hold AI-generated metadata; cleared by clear_image_metadata so the photo stays indexed.
AI_METADATA_KEYS = frozenset(
    {
        "title",
        "caption",
        "keywords",
        "alt_text",
        "model",
        "run_date",
        "tokens_used",
        "flattened_keywords",
        "edit_recipe",
        "edit_summary",
        "edit_warnings",
        "edit_model",
        "edit_provider",
        "edit_run_date",
    }
)


def clear_image_metadata(photo_id, *, legacy_uuid=None):
    """
    Clear only AI-generated metadata for an image. Keeps the document and embedding
    in both main and Vertex collections so the photo remains searchable; use when
    the user discards a suggestion and may regenerate later.
    Returns True if the main collection had the photo (and metadata was cleared), False otherwise.
    """
    _ensure_initialized()
    if collection is None:
        return False
    photo_id = _normalize_photo_id(photo_id, legacy_uuid)
    if not photo_id:
        return False
    # Main collection: get current, strip AI fields, update (keep embedding)
    data = collection.get(ids=[photo_id], include=["metadatas", "embeddings"])
    if not data or not data.get("ids"):
        logger.debug(
            "clear_image_metadata: photo_id %s not in main collection", photo_id
        )
        return False
    meta = dict(data["metadatas"][0]) if data.get("metadatas") else {}
    embedding = _first_result_item(data.get("embeddings"))
    for key in AI_METADATA_KEYS:
        meta.pop(key, None)
    meta = _ensure_photo_metadata(photo_id, meta, legacy_uuid=legacy_uuid)
    if embedding is not None:
        collection.update(ids=[photo_id], metadatas=[meta], embeddings=[embedding])
    else:
        collection.update(ids=[photo_id], metadatas=[meta])
    # Vertex collection: same if present
    try:
        vdata = vertex_collection.get(
            ids=[photo_id], include=["metadatas", "embeddings"]
        )
        if vdata and vdata.get("ids"):
            vmeta = dict(vdata["metadatas"][0]) if vdata.get("metadatas") else {}
            vemb = _first_result_item(vdata.get("embeddings"))
            for key in AI_METADATA_KEYS:
                vmeta.pop(key, None)
            vmeta = _ensure_photo_metadata(photo_id, vmeta, legacy_uuid=legacy_uuid)
            if vemb is not None:
                vertex_collection.update(
                    ids=[photo_id], metadatas=[vmeta], embeddings=[vemb]
                )
            else:
                vertex_collection.update(ids=[photo_id], metadatas=[vmeta])
    except Exception as e:
        logger.debug("clear_image_metadata vertex %s: %s", photo_id, e)
    return True


# --- Vertex AI image embeddings collection API ---


def add_vertex_image(photo_id, embedding, metadata=None, *, legacy_uuid=None):
    """Add or overwrite Vertex AI embedding for an image."""
    _ensure_initialized()
    if vertex_collection is None:
        raise DatabaseNotReadyError(
            "Cannot add vertex image: database not initialized (DB_PATH missing)."
        )
    photo_id = _normalize_photo_id(photo_id, legacy_uuid)
    if not photo_id:
        raise ValueError("photo_id is required")
    if metadata is None:
        metadata = {}
    metadata = _ensure_photo_metadata(photo_id, metadata, legacy_uuid=legacy_uuid)
    existing = vertex_collection.get(ids=[photo_id], include=[])
    if existing and existing.get("ids"):
        vertex_collection.update(
            ids=[photo_id], embeddings=[embedding], metadatas=[metadata]
        )
    else:
        vertex_collection.add(
            ids=[photo_id], embeddings=[embedding], metadatas=[metadata]
        )


def update_vertex_image(photo_id, embedding=None, metadata=None, *, legacy_uuid=None):
    """Update Vertex AI embedding and/or metadata for an existing document."""
    _ensure_initialized()
    if vertex_collection is None:
        raise DatabaseNotReadyError(
            "Cannot update vertex image: database not initialized (DB_PATH missing)."
        )
    photo_id = _normalize_photo_id(photo_id, legacy_uuid)
    if not photo_id:
        raise ValueError("photo_id is required")
    if metadata is not None:
        metadata = _ensure_photo_metadata(photo_id, metadata, legacy_uuid=legacy_uuid)
    if embedding is not None and metadata is not None:
        vertex_collection.update(
            ids=[photo_id], embeddings=[embedding], metadatas=[metadata]
        )
    elif embedding is not None:
        vertex_collection.update(ids=[photo_id], embeddings=[embedding])
    elif metadata is not None:
        vertex_collection.update(ids=[photo_id], metadatas=[metadata])


def get_vertex_image(photo_id, *, legacy_uuid=None):
    """Get Vertex AI embedding record for an image. Returns Chroma get() result or empty."""
    _ensure_initialized()
    if vertex_collection is None:
        return {"ids": [], "metadatas": [], "embeddings": []}
    photo_id = _normalize_photo_id(photo_id, legacy_uuid)
    if not photo_id:
        return {"ids": [], "metadatas": [], "embeddings": []}
    return vertex_collection.get(ids=[photo_id], include=["metadatas", "embeddings"])


def delete_vertex_image(photo_id, *, legacy_uuid=None):
    """Remove Vertex AI embedding for an image."""
    _ensure_initialized()
    if vertex_collection is None:
        return
    photo_id = _normalize_photo_id(photo_id, legacy_uuid)
    if not photo_id:
        return
    try:
        vertex_collection.delete(ids=[photo_id])
    except Exception as e:
        logger.debug("delete_vertex_image %s: %s", photo_id, e)


def has_vertex_embedding(photo_id, *, legacy_uuid=None):
    """Return True if this image has a Vertex AI embedding in the vertex collection."""
    _ensure_initialized()
    if vertex_collection is None:
        return False
    photo_id = _normalize_photo_id(photo_id, legacy_uuid)
    if not photo_id:
        return False
    try:
        r = vertex_collection.get(ids=[photo_id], include=[])
        return len(r.get("ids", [])) > 0
    except Exception:
        return False


def query_vertex_images(query_embedding, n_results, where_clause=None, catalog_id=None):
    """Query the Vertex AI image embeddings collection by embedding. Returns ids, distances, metadatas.
    If catalog_id is set, results are filtered to photo_ids that belong to that catalog (main collection).
    """
    _ensure_initialized()
    if vertex_collection is None:
        return {"ids": [[]], "distances": [[]], "metadatas": [[]]}
    try:
        n_fetch = (int(n_results) * 2 + 100) if catalog_id else n_results
        result = vertex_collection.query(
            where=where_clause,
            query_embeddings=[query_embedding],
            n_results=min(n_fetch, STATS_GET_LIMIT),
            include=["metadatas", "distances"],
        )
        if (
            not catalog_id
            or not result
            or not result.get("ids")
            or not result["ids"][0]
        ):
            return result
        allowed = set(get_all_image_ids(catalog_id=catalog_id))
        ids0 = result["ids"][0]
        dist0 = result["distances"][0] if result.get("distances") else []
        meta0 = result["metadatas"][0] if result.get("metadatas") else []
        keep = [i for i, pid in enumerate(ids0) if pid in allowed][:n_results]
        result["ids"] = [[ids0[j] for j in keep]]
        result["distances"] = [[dist0[j] for j in keep]] if dist0 else [[]]
        result["metadatas"] = [[meta0[j] for j in keep]] if meta0 else [[]]
        return result
    except Exception as e:
        logger.error(f"Error querying Vertex images: {e}", exc_info=True)
        return {"ids": [[]], "distances": [[]], "metadatas": [[]]}


def get_all_vertex_image_ids():
    """Return all image UUIDs that have a Vertex AI embedding (for search fallback)."""
    _ensure_initialized()
    if vertex_collection is None:
        return []
    return vertex_collection.get(include=[], limit=STATS_GET_LIMIT)["ids"]


def query_images(query_embedding, n_results, where_clause=None, catalog_id=None):
    _ensure_initialized()
    if collection is None:
        return {"ids": [[]], "distances": [[]], "metadatas": [[]]}
    try:
        # Over-fetch when filtering by catalog so we have enough after post-filter
        n_fetch = (int(n_results) * 2 + 100) if catalog_id else n_results
        result = collection.query(
            where=where_clause,
            query_embeddings=query_embedding,
            n_results=min(n_fetch, STATS_GET_LIMIT),
            include=["metadatas", "distances"],
        )
        if (
            not catalog_id
            or not result
            or not result.get("ids")
            or not result["ids"][0]
        ):
            return result
        catalog_id_str = str(catalog_id).strip()
        keep = []
        ids0 = result["ids"][0]
        dist0 = result["distances"][0] if result.get("distances") else []
        meta0 = result["metadatas"][0] if result.get("metadatas") else []
        for i, pid in enumerate(ids0):
            m = meta0[i] if i < len(meta0) else {}
            if catalog_id_str in _parse_catalog_ids(m):
                keep.append(i)
            if len(keep) >= n_results:
                break
        result["ids"] = [[ids0[j] for j in keep]]
        result["distances"] = [[dist0[j] for j in keep]] if dist0 else [[]]
        result["metadatas"] = [[meta0[j] for j in keep]] if meta0 else [[]]
        return result
    except Exception as e:
        logger.error(f"Error querying images: {e}", exc_info=True)
        return {"ids": [[]], "distances": [[]], "metadatas": [[]]}


def get_image_count():
    """Return total number of indexed images (photos) in the collection."""
    _ensure_initialized()
    if collection is None:
        return 0
    return len(collection.get(include=[], limit=STATS_GET_LIMIT)["ids"])


def get_face_count():
    """Return total number of face embeddings in the face collection."""
    _ensure_initialized()
    if face_collection is None:
        return 0
    return len(face_collection.get(include=[], limit=STATS_GET_LIMIT)["ids"])


def get_image_metadata_stats(catalog_id=None):
    """
    Return counts of images by metadata presence (no embeddings loaded).
    Returns dict: total, with_embedding, with_title, with_caption, with_keywords, with_vertexai.
    If catalog_id is set, only count photos whose catalog_ids contain that catalog.
    """
    _ensure_initialized()
    if collection is None:
        return {
            "total": 0,
            "with_embedding": 0,
            "with_title": 0,
            "with_caption": 0,
            "with_keywords": 0,
            "with_vertexai": 0,
        }
    result = collection.get(include=["metadatas"], limit=STATS_GET_LIMIT)
    ids = result.get("ids", [])
    metadatas = result.get("metadatas", []) or []
    catalog_id_str = str(catalog_id).strip() if catalog_id else None
    vertex_ids = set(get_all_vertex_image_ids())
    total = 0
    with_embedding = 0
    with_title = 0
    with_caption = 0
    with_keywords = 0
    with_vertexai = 0
    for idx, m in enumerate(metadatas):
        if catalog_id_str is not None:
            ids_set = _parse_catalog_ids(m)
            if catalog_id_str not in ids_set:
                continue
        total += 1
        if m.get("has_embedding", True):
            with_embedding += 1
        if (m.get("title") or "").strip():
            with_title += 1
        if (m.get("caption") or "").strip():
            with_caption += 1
        if (m.get("keywords") or m.get("flattened_keywords") or "").strip():
            with_keywords += 1
        if idx < len(ids) and ids[idx] in vertex_ids:
            with_vertexai += 1
    return {
        "total": total,
        "with_embedding": with_embedding,
        "with_title": with_title,
        "with_caption": with_caption,
        "with_keywords": with_keywords,
        "with_vertexai": with_vertexai,
    }


# Batch size for sync_claim: one get + one or two updates per batch instead of per photo
SYNC_CLAIM_BATCH_SIZE = 200


def sync_claim(catalog_id, photo_ids):
    """Add catalog_id to each photo's catalog_ids (claim existing backend photos for this catalog).
    Used for migration: unclaimed photos become visible to this catalog.
    Returns {"claimed": N, "errors": M}. Uses batched get/update for speed.
    Deduplicates photo_ids so Chroma get() is not given duplicate IDs (e.g. virtual copies share file-based id).
    """
    _ensure_initialized()
    if collection is None:
        return {"claimed": 0, "errors": 0}
    if not catalog_id:
        return {"claimed": 0, "errors": 0}
    catalog_id_str = str(catalog_id).strip()
    # Deduplicate: same photo_id can appear multiple times (virtual copies, same file)
    seen = set()
    unique = []
    for pid in photo_ids or []:
        pid = str(pid).strip()
        if not pid or pid in seen:
            continue
        seen.add(pid)
        unique.append(pid)
    photo_ids = unique
    claimed = 0
    errors = 0
    for start in range(0, len(photo_ids), SYNC_CLAIM_BATCH_SIZE):
        chunk = photo_ids[start : start + SYNC_CLAIM_BATCH_SIZE]
        try:
            data = collection.get(ids=chunk, include=["metadatas", "embeddings"])
            if not data or not data.get("ids"):
                continue
            ids = data["ids"]
            metadatas = data.get("metadatas") or [{}] * len(ids)
            embeddings = data.get("embeddings")
            if embeddings is not None and isinstance(embeddings, np.ndarray):
                embeddings = list(embeddings)
            elif embeddings is None:
                embeddings = [None] * len(ids)
            update_ids = []
            update_metadatas = []
            update_embeddings = []
            no_emb_ids = []
            no_emb_metadatas = []
            for i, pid in enumerate(ids):
                meta = dict(metadatas[i]) if i < len(metadatas) else {}
                ids_set = _parse_catalog_ids(meta)
                ids_set.add(catalog_id_str)
                meta[CATALOG_IDS_FIELD] = _serialize_catalog_ids(ids_set)
                meta = _ensure_photo_metadata(pid, meta)
                emb = embeddings[i] if i < len(embeddings) else None
                if emb is not None:
                    update_ids.append(pid)
                    update_metadatas.append(meta)
                    update_embeddings.append(
                        emb if not isinstance(emb, np.ndarray) else emb.tolist()
                    )
                else:
                    no_emb_ids.append(pid)
                    no_emb_metadatas.append(meta)
            if update_ids:
                collection.update(
                    ids=update_ids,
                    metadatas=update_metadatas,
                    embeddings=update_embeddings,
                )
                claimed += len(update_ids)
            if no_emb_ids:
                collection.update(ids=no_emb_ids, metadatas=no_emb_metadatas)
                claimed += len(no_emb_ids)
        except Exception as e:
            logger.warning(
                "sync_claim batch failed for chunk %s..%s: %s",
                start,
                start + len(chunk),
                e,
            )
            errors += len(chunk)
    return {"claimed": claimed, "errors": errors}


def sync_cleanup(catalog_id, active_photo_ids):
    """Disassociate catalog_id from photos that are no longer in active_photo_ids.
    Does not delete any documents; only updates catalog_ids metadata.
    Returns {"checked": N, "disassociated": M}.
    """
    _ensure_initialized()
    if collection is None:
        return {"checked": 0, "disassociated": 0}
    if not catalog_id:
        return {"checked": 0, "disassociated": 0}
    active = set(active_photo_ids) if active_photo_ids is not None else set()
    result = collection.get(include=["metadatas"], limit=STATS_GET_LIMIT)
    ids = result.get("ids") or []
    metadatas = result.get("metadatas") or []
    checked = 0
    disassociated = 0
    catalog_id_str = str(catalog_id).strip()
    for i, meta in enumerate(metadatas):
        pid = ids[i] if i < len(ids) else None
        if not pid:
            continue
        ids_set = _parse_catalog_ids(meta)
        if catalog_id_str not in ids_set:
            continue
        checked += 1
        if pid not in active:
            _remove_catalog_id(pid, catalog_id_str)
            disassociated += 1
    return {"checked": checked, "disassociated": disassociated}


def get_all_image_ids(has_embedding=None, catalog_id=None):
    """Get all image IDs, optionally filtered by embedding status and/or catalog_id.

    Args:
        has_embedding: If True, only return IDs with real embeddings.
                      If False, only return IDs with dummy embeddings.
                      If None, return all IDs.
        catalog_id: If set, only return IDs whose catalog_ids metadata contains this catalog.
    """
    _ensure_initialized()
    if collection is None:
        return []
    need_metadata = has_embedding is not None or catalog_id is not None
    if not need_metadata:
        result = collection.get(include=[], limit=STATS_GET_LIMIT)
        return result["ids"]
    result = collection.get(include=["metadatas"], limit=STATS_GET_LIMIT)
    filtered_ids = []
    catalog_id_str = str(catalog_id).strip() if catalog_id else None
    for i, metadata in enumerate(result["metadatas"]):
        if has_embedding is not None:
            has_emb = metadata.get("has_embedding", True) if metadata else True
            if has_emb != has_embedding:
                continue
        if catalog_id_str is not None:
            ids_set = _parse_catalog_ids(metadata)
            if catalog_id_str not in ids_set:
                continue
        filtered_ids.append(result["ids"][i])
    return filtered_ids


def _safe_float(value, default=None):
    try:
        if value is None or value == "":
            return default
        return float(value)
    except (TypeError, ValueError):
        return default


def _embedding_to_array(embedding):
    if embedding is None:
        return None
    try:
        arr = np.asarray(embedding, dtype=np.float32)
    except Exception:
        return None
    if arr.size == 0 or np.allclose(arr, 0.0):
        return None
    return arr


def _cosine_distance(embedding_a, embedding_b):
    if embedding_a is None or embedding_b is None:
        return None
    norm_a = np.linalg.norm(embedding_a)
    norm_b = np.linalg.norm(embedding_b)
    if norm_a == 0.0 or norm_b == 0.0:
        return None
    similarity = float(np.dot(embedding_a, embedding_b) / (norm_a * norm_b))
    similarity = max(-1.0, min(1.0, similarity))
    return 1.0 - similarity


def _phash_to_int(value):
    if value is None:
        return None
    text = str(value).strip().lower()
    if not text:
        return None
    try:
        return int(text, 16)
    except ValueError:
        return None


def _phash_hamming_distance(left_hash, right_hash):
    if left_hash is None or right_hash is None:
        return None
    return int((left_hash ^ right_hash).bit_count())


def _derive_grouping_thresholds(
    phash_threshold, clip_threshold, time_delta, culling_config=None
):
    culling_config = culling_config or CULLING_CONFIG
    try:
        time_window_seconds = max(0, int(time_delta))
    except (TypeError, ValueError):
        time_window_seconds = culling_config["grouping"]["time_window_default_seconds"]

    if clip_threshold == "auto":
        burst_distance_threshold = culling_config["grouping"]["burst_distance_auto"]
    else:
        try:
            burst_distance_threshold = max(0.0, float(clip_threshold))
        except (TypeError, ValueError):
            burst_distance_threshold = culling_config["grouping"]["burst_distance_auto"]

    if phash_threshold == "auto":
        phash_hamming_threshold = int(culling_config["grouping"]["phash_hamming_auto"])
    else:
        try:
            phash_hamming_threshold = int(
                max(
                    0.0,
                    min(
                        float(phash_threshold), culling_config["grouping"]["phash_max"]
                    ),
                )
            )
        except (TypeError, ValueError):
            phash_hamming_threshold = int(
                culling_config["grouping"]["phash_hamming_auto"]
            )

    phash_max = culling_config["grouping"]["phash_max"]
    normalized = max(0.0, min(float(phash_hamming_threshold), phash_max)) / phash_max
    duplicate_distance_threshold = culling_config["grouping"][
        "duplicate_distance_min"
    ] + (normalized * culling_config["grouping"]["duplicate_distance_span"])

    duplicate_time_window_seconds = max(
        time_window_seconds
        * culling_config["grouping"]["duplicate_time_window_multiplier"],
        culling_config["grouping"]["duplicate_time_window_min_seconds"],
    )
    return (
        phash_hamming_threshold,
        duplicate_distance_threshold,
        burst_distance_threshold,
        duplicate_time_window_seconds,
        time_window_seconds,
    )


def _record_sort_key(item):
    return (
        item["capture_time"] is None,
        item["capture_time"] if item["capture_time"] is not None else float("inf"),
        item["filename"],
        item["photo_id"],
    )


def _extract_culling_metric(metadata, key, default):
    value = _safe_float((metadata or {}).get(key), default)
    if value is None:
        return default
    return max(0.0, min(1.0, value))


def _explanation_from_reason_codes(reason_codes):
    labels = {
        "sharpest_in_group": "sharpest in group",
        "blurred": "noticeably blurred",
        "underexposed": "darker than stronger alternatives",
        "overexposed": "brighter than stronger alternatives",
        "low_aesthetic": "weaker aesthetic impression than alternatives",
        "best_face_quality": "best face quality in group",
        "weak_face_quality": "weaker face quality than alternatives",
        "no_face_detected_in_group": "no clear face detected while alternatives have faces",
        "possible_occlusion": "possible facial occlusion or weak visibility",
        "eyes_open_best": "best eyes-open result in group",
        "possible_blink": "possible blink or eyes less open",
        "near_duplicate_weaker": "weaker duplicate or burst alternative",
    }
    if not reason_codes:
        return "single image in group"
    return "; ".join(labels.get(code, code.replace("_", " ")) for code in reason_codes)


def _rank_group_records(component_records, group_type, culling_config=None):
    culling_config = culling_config or CULLING_CONFIG
    scored_records = []
    for record in component_records:
        metadata = record["metadata"]
        sharpness = _extract_culling_metric(metadata, "cull_sharpness", 0.0)
        exposure = _extract_culling_metric(metadata, "cull_exposure", 0.0)
        noise_penalty = _extract_culling_metric(metadata, "cull_noise", 1.0)
        highlight_clip = _extract_culling_metric(metadata, "cull_highlight_clip", 0.0)
        shadow_clip = _extract_culling_metric(metadata, "cull_shadow_clip", 0.0)
        clipping_penalty = max(0.0, min(1.0, highlight_clip + shadow_clip))
        technical_score = _extract_culling_metric(
            metadata,
            "cull_technical_score",
            (0.5 * sharpness)
            + (0.3 * exposure)
            + (0.1 * (1.0 - noise_penalty))
            + (0.1 * (1.0 - clipping_penalty)),
        )
        aesthetic_score = _extract_culling_metric(metadata, "cull_aesthetic", 0.0)
        face_count = int(_safe_float((metadata or {}).get("cull_face_count"), 0) or 0)
        face_sharpness = _extract_culling_metric(metadata, "cull_face_sharpness", 0.0)
        face_prominence = _extract_culling_metric(metadata, "cull_face_prominence", 0.0)
        face_visibility = _extract_culling_metric(metadata, "cull_face_visibility", 0.0)
        occlusion_penalty = _extract_culling_metric(metadata, "cull_occlusion", 0.0)
        eye_openness = _extract_culling_metric(metadata, "cull_eye_openness", 0.0)
        face_score = _extract_culling_metric(
            metadata,
            "cull_face_score",
            (
                (
                    culling_config["face_metrics"]["score_weight_sharpness"]
                    * face_sharpness
                    + culling_config["face_metrics"]["score_weight_prominence"]
                    * face_prominence
                    + culling_config["face_metrics"]["score_weight_visibility"]
                    * face_visibility
                    + culling_config["face_metrics"]["score_weight_eye_openness"]
                    * eye_openness
                    + culling_config["face_metrics"]["score_weight_occlusion"]
                    * (1.0 - occlusion_penalty)
                )
                / max(
                    1e-6,
                    culling_config["face_metrics"]["score_weight_sharpness"]
                    + culling_config["face_metrics"]["score_weight_prominence"]
                    + culling_config["face_metrics"]["score_weight_visibility"]
                    + culling_config["face_metrics"]["score_weight_eye_openness"]
                    + culling_config["face_metrics"]["score_weight_occlusion"],
                )
            ),
        )
        blink_penalty = _extract_culling_metric(metadata, "cull_blink_penalty", 1.0)

        scored_records.append(
            {
                **record,
                "cull_sharpness": sharpness,
                "cull_exposure": exposure,
                "cull_noise": noise_penalty,
                "cull_highlight_clip": highlight_clip,
                "cull_shadow_clip": shadow_clip,
                "cull_technical_score": technical_score,
                "cull_aesthetic": aesthetic_score,
                "cull_face_count": face_count,
                "cull_face_sharpness": face_sharpness,
                "cull_face_prominence": face_prominence,
                "cull_face_visibility": face_visibility,
                "cull_face_score": face_score,
                "cull_occlusion": occlusion_penalty,
                "cull_eye_openness": eye_openness,
                "cull_blink_penalty": blink_penalty,
            }
        )

    group_has_faces = any(item["cull_face_count"] > 0 for item in scored_records)
    for item in scored_records:
        if group_has_faces:
            if item["cull_face_count"] > 0:
                weighted_score = (
                    culling_config["ranking"]["face_group_weight_technical"]
                    * item["cull_technical_score"]
                    + culling_config["ranking"]["face_group_weight_face"]
                    * item["cull_face_score"]
                    + culling_config["ranking"]["face_group_weight_aesthetic"]
                    * item["cull_aesthetic"]
                )
                weight_sum = (
                    culling_config["ranking"]["face_group_weight_technical"]
                    + culling_config["ranking"]["face_group_weight_face"]
                    + culling_config["ranking"]["face_group_weight_aesthetic"]
                )
                item["cull_score"] = max(
                    0.0, min(1.0, weighted_score / max(1e-6, weight_sum))
                )
                item["cull_score"] = max(
                    0.0,
                    min(
                        1.0,
                        item["cull_score"]
                        - (
                            culling_config["ranking"]["face_group_blink_penalty_weight"]
                            * item["cull_blink_penalty"]
                            + culling_config["ranking"][
                                "face_group_occlusion_penalty_weight"
                            ]
                            * item["cull_occlusion"]
                        ),
                    ),
                )
            else:
                # Penalize face-missing shots in face-heavy groups.
                item["cull_score"] = max(
                    0.0,
                    (
                        culling_config["ranking"]["face_missing_technical_weight"]
                        * item["cull_technical_score"]
                    )
                    - culling_config["ranking"]["face_missing_penalty"],
                )
        else:
            weighted_score = item["cull_technical_score"] + (
                culling_config["ranking"]["no_face_group_weight_aesthetic"]
                * item["cull_aesthetic"]
            )
            weight_sum = (
                1.0 + culling_config["ranking"]["no_face_group_weight_aesthetic"]
            )
            item["cull_score"] = max(
                0.0, min(1.0, weighted_score / max(1e-6, weight_sum))
            )

    scored_records.sort(
        key=lambda item: (
            -item["cull_score"],
            -item["cull_face_score"],
            -item["cull_sharpness"],
            -item["cull_exposure"],
            item["cull_noise"],
            item["photo_id"],
        )
    )

    if not scored_records:
        return []

    max_sharpness = max(item["cull_sharpness"] for item in scored_records)
    max_face_score = max(item["cull_face_score"] for item in scored_records)
    max_eye_openness = max(item["cull_eye_openness"] for item in scored_records)
    max_aesthetic = max(item["cull_aesthetic"] for item in scored_records)
    winner_score = scored_records[0]["cull_score"]

    for index, item in enumerate(scored_records, start=1):
        reason_codes = []
        if item["cull_sharpness"] < culling_config["ranking"]["reason_blur_threshold"]:
            reason_codes.append("blurred")
        if (
            item["cull_exposure"]
            < culling_config["ranking"]["reason_exposure_threshold"]
        ):
            if item["cull_shadow_clip"] >= item["cull_highlight_clip"]:
                reason_codes.append("underexposed")
            else:
                reason_codes.append("overexposed")
        if item["cull_aesthetic"] < culling_config["ranking"][
            "reason_low_aesthetic_threshold"
        ] and item["cull_aesthetic"] < max(0.0, max_aesthetic - 0.08):
            reason_codes.append("low_aesthetic")
        if (
            index == 1
            and len(scored_records) > 1
            and item["cull_sharpness"]
            >= (max_sharpness - culling_config["ranking"]["reason_sharpest_delta"])
        ):
            reason_codes.append("sharpest_in_group")
        if group_has_faces:
            if item["cull_face_count"] == 0:
                reason_codes.append("no_face_detected_in_group")
            elif (
                item["cull_face_score"]
                >= (
                    max_face_score - culling_config["ranking"]["reason_best_face_delta"]
                )
                and index == 1
            ):
                reason_codes.append("best_face_quality")
            elif item["cull_face_score"] < max(
                0.0,
                max_face_score - culling_config["ranking"]["reason_weak_face_delta"],
            ):
                reason_codes.append("weak_face_quality")
            if (
                item["cull_occlusion"]
                > culling_config["ranking"]["reason_occlusion_threshold"]
            ):
                reason_codes.append("possible_occlusion")
            if (
                item["cull_eye_openness"]
                >= max(
                    0.0,
                    max_eye_openness
                    - culling_config["ranking"]["reason_eyes_open_delta"],
                )
                and index == 1
            ):
                reason_codes.append("eyes_open_best")
            elif (
                item["cull_blink_penalty"]
                > culling_config["ranking"]["reason_possible_blink_threshold"]
            ):
                reason_codes.append("possible_blink")
        if index > 1 and group_type != "single":
            reason_codes.append("near_duplicate_weaker")

        reject_candidate = False
        if len(scored_records) > 1:
            reject_candidate = (
                item["cull_score"]
                <= max(
                    0.0, winner_score - culling_config["ranking"]["reject_score_delta"]
                )
                or item["cull_sharpness"]
                < culling_config["ranking"]["reason_blur_threshold"]
                or item["cull_exposure"]
                < culling_config["ranking"]["reject_exposure_threshold"]
                or (
                    group_has_faces
                    and item["cull_face_count"] > 0
                    and item["cull_face_score"]
                    < culling_config["ranking"]["reject_face_score_threshold"]
                )
                or (
                    group_has_faces
                    and item["cull_face_count"] > 0
                    and item["cull_blink_penalty"]
                    > culling_config["ranking"]["reject_blink_penalty_threshold"]
                )
                or (
                    group_has_faces
                    and item["cull_face_count"] > 0
                    and item["cull_occlusion"]
                    > culling_config["ranking"]["reject_occlusion_threshold"]
                )
            )

        item["cull_group_rank"] = index
        item["cull_group_winner"] = index == 1
        item["cull_reject_candidate"] = reject_candidate and index != 1
        item["cull_reason_codes"] = reason_codes
        item["cull_explanation"] = _explanation_from_reason_codes(reason_codes)

    return scored_records


def group_and_sort_images(
    uuids, phash_threshold, clip_threshold, time_delta, culling_preset="default"
):
    """
    Group indexed images into stable similarity clusters for culling workflows.

    Uses stored capture times, perceptual hash (pHash) hamming distance, and
    image embedding similarity as a fallback/secondary duplicate signal.
    """
    _ensure_initialized()
    if collection is None:
        return []

    if not uuids:
        return []

    culling_config = get_culling_config(culling_preset)

    (
        phash_hamming_threshold,
        duplicate_distance_threshold,
        burst_distance_threshold,
        duplicate_time_window_seconds,
        time_window_seconds,
    ) = _derive_grouping_thresholds(
        phash_threshold, clip_threshold, time_delta, culling_config=culling_config
    )

    unique_photo_ids = []
    seen_ids = set()
    for photo_id in uuids:
        normalized_id = _normalize_photo_id(photo_id)
        if normalized_id and normalized_id not in seen_ids:
            unique_photo_ids.append(normalized_id)
            seen_ids.add(normalized_id)

    if not unique_photo_ids:
        return []

    raw = collection.get(ids=unique_photo_ids, include=["metadatas", "embeddings"])
    metadata_by_id = {}
    embedding_by_id = {}
    for idx, photo_id in enumerate(raw.get("ids", [])):
        metadata_list = raw.get("metadatas", [])
        embedding_list = raw.get("embeddings", [])
        metadata_by_id[photo_id] = (
            metadata_list[idx] if idx < len(metadata_list) else {}
        )
        embedding_by_id[photo_id] = (
            embedding_list[idx] if idx < len(embedding_list) else None
        )

    records = []
    for photo_id in unique_photo_ids:
        metadata = metadata_by_id.get(photo_id, {}) or {}
        capture_time = _safe_float(metadata.get("capture_time"))
        filename = str(metadata.get("filename") or "")
        records.append(
            {
                "photo_id": photo_id,
                "filename": filename,
                "capture_time": capture_time,
                "embedding": _embedding_to_array(embedding_by_id.get(photo_id)),
                "phash": _phash_to_int(
                    metadata.get("cull_phash") or metadata.get("phash")
                ),
                "metadata": metadata,
            }
        )

    records.sort(key=_record_sort_key)

    adjacency = {item["photo_id"]: set() for item in records}
    edge_kinds = {}

    for left_index in range(len(records)):
        left = records[left_index]
        for right_index in range(left_index + 1, len(records)):
            right = records[right_index]
            distance = _cosine_distance(left["embedding"], right["embedding"])
            phash_distance = _phash_hamming_distance(left["phash"], right["phash"])

            time_gap = None
            if left["capture_time"] is not None and right["capture_time"] is not None:
                time_gap = abs(right["capture_time"] - left["capture_time"])
                if (
                    time_gap > duplicate_time_window_seconds
                    and distance is None
                    and phash_distance is None
                ):
                    break

            is_near_duplicate = (
                (
                    phash_distance is not None
                    and phash_distance <= phash_hamming_threshold
                )
                or (distance is not None and distance <= duplicate_distance_threshold)
            ) and (time_gap is None or time_gap <= duplicate_time_window_seconds)
            is_burst_neighbor = (
                distance is not None
                and distance <= burst_distance_threshold
                and time_gap is not None
                and time_gap <= time_window_seconds
            )

            if not is_near_duplicate and not is_burst_neighbor:
                continue

            left_id = left["photo_id"]
            right_id = right["photo_id"]
            adjacency[left_id].add(right_id)
            adjacency[right_id].add(left_id)
            edge_kinds[tuple(sorted((left_id, right_id)))] = (
                "near_duplicate" if is_near_duplicate else "burst"
            )

    groups = []
    visited = set()
    group_counter = 1
    metadata_updates = []

    for record in records:
        start_id = record["photo_id"]
        if start_id in visited:
            continue

        stack = [start_id]
        component_ids = []
        while stack:
            current_id = stack.pop()
            if current_id in visited:
                continue
            visited.add(current_id)
            component_ids.append(current_id)
            for neighbor_id in sorted(adjacency[current_id], reverse=True):
                if neighbor_id not in visited:
                    stack.append(neighbor_id)

        component_id_set = set(component_ids)
        component_records = [
            item for item in records if item["photo_id"] in component_id_set
        ]
        component_records.sort(key=_record_sort_key)

        group_photo_ids = [item["photo_id"] for item in component_records]
        capture_times = [
            item["capture_time"]
            for item in component_records
            if item["capture_time"] is not None
        ]
        time_span_seconds = 0.0
        if len(capture_times) >= 2:
            time_span_seconds = float(max(capture_times) - min(capture_times))

        pair_distances = []
        pair_phash_distances = []
        group_edge_types = set()
        for left_index in range(len(component_records)):
            for right_index in range(left_index + 1, len(component_records)):
                left = component_records[left_index]
                right = component_records[right_index]
                distance = _cosine_distance(left["embedding"], right["embedding"])
                phash_distance = _phash_hamming_distance(left["phash"], right["phash"])
                if distance is not None:
                    pair_distances.append(round(distance, 4))
                if phash_distance is not None:
                    pair_phash_distances.append(phash_distance)
                edge_type = edge_kinds.get(
                    tuple(sorted((left["photo_id"], right["photo_id"])))
                )
                if edge_type:
                    group_edge_types.add(edge_type)

        if len(group_photo_ids) == 1:
            group_type = "single"
        elif "near_duplicate" in group_edge_types and "burst" not in group_edge_types:
            group_type = "near_duplicate"
        elif time_span_seconds <= time_window_seconds:
            group_type = "burst"
        else:
            group_type = "near_duplicate"

        ranked_records = _rank_group_records(
            component_records, group_type, culling_config=culling_config
        )
        group_id = f"group_{group_counter:04d}"
        winner_photo_id = (
            ranked_records[0]["photo_id"] if ranked_records else group_photo_ids[0]
        )
        alternate_photo_ids = [
            item["photo_id"]
            for item in ranked_records[1:]
            if not item["cull_reject_candidate"]
        ]
        reject_candidate_photo_ids = [
            item["photo_id"] for item in ranked_records if item["cull_reject_candidate"]
        ]

        for ranked in ranked_records:
            updated_metadata = dict(ranked["metadata"] or {})
            updated_metadata.update(
                {
                    "cull_group_id": group_id,
                    "cull_group_size": len(group_photo_ids),
                    "cull_group_rank": ranked["cull_group_rank"],
                    "cull_group_winner": ranked["cull_group_winner"],
                    "cull_score": round(ranked["cull_score"], 4),
                    "cull_reject_candidate": ranked["cull_reject_candidate"],
                    "cull_reason_codes": json.dumps(ranked["cull_reason_codes"]),
                    "cull_explanation": ranked["cull_explanation"],
                    "cull_sharpness": round(ranked["cull_sharpness"], 4),
                    "cull_exposure": round(ranked["cull_exposure"], 4),
                    "cull_noise": round(ranked["cull_noise"], 4),
                    "cull_highlight_clip": round(ranked["cull_highlight_clip"], 4),
                    "cull_shadow_clip": round(ranked["cull_shadow_clip"], 4),
                    "cull_technical_score": round(ranked["cull_technical_score"], 4),
                    "cull_aesthetic": round(ranked["cull_aesthetic"], 4),
                    "cull_face_count": int(ranked["cull_face_count"]),
                    "cull_face_sharpness": round(ranked["cull_face_sharpness"], 4),
                    "cull_face_prominence": round(ranked["cull_face_prominence"], 4),
                    "cull_face_visibility": round(ranked["cull_face_visibility"], 4),
                    "cull_face_score": round(ranked["cull_face_score"], 4),
                    "cull_occlusion": round(ranked["cull_occlusion"], 4),
                    "cull_eye_openness": round(ranked["cull_eye_openness"], 4),
                    "cull_blink_penalty": round(ranked["cull_blink_penalty"], 4),
                }
            )
            metadata_updates.append((ranked["photo_id"], updated_metadata))

        groups.append(
            {
                "group_id": group_id,
                "group_type": group_type,
                "group_size": len(group_photo_ids),
                "primary_photo_id": group_photo_ids[0],
                "photo_ids": group_photo_ids,
                "winner_photo_id": winner_photo_id,
                "alternate_photo_ids": alternate_photo_ids,
                "reject_candidate_photo_ids": reject_candidate_photo_ids,
                "photos": [
                    {
                        "photo_id": item["photo_id"],
                        "rank": item["cull_group_rank"],
                        "cull_score": round(item["cull_score"], 4),
                        "winner": item["cull_group_winner"],
                        "reject_candidate": item["cull_reject_candidate"],
                        "reason_codes": item["cull_reason_codes"],
                        "explanation": item["cull_explanation"],
                        "metrics": {
                            "sharpness": round(item["cull_sharpness"], 4),
                            "exposure": round(item["cull_exposure"], 4),
                            "noise": round(item["cull_noise"], 4),
                            "highlight_clip": round(item["cull_highlight_clip"], 4),
                            "shadow_clip": round(item["cull_shadow_clip"], 4),
                            "technical_score": round(item["cull_technical_score"], 4),
                            "aesthetic": round(item["cull_aesthetic"], 4),
                            "face_count": int(item["cull_face_count"]),
                            "face_sharpness": round(item["cull_face_sharpness"], 4),
                            "face_prominence": round(item["cull_face_prominence"], 4),
                            "face_visibility": round(item["cull_face_visibility"], 4),
                            "face_score": round(item["cull_face_score"], 4),
                            "occlusion": round(item["cull_occlusion"], 4),
                            "eye_openness": round(item["cull_eye_openness"], 4),
                            "blink_penalty": round(item["cull_blink_penalty"], 4),
                        },
                    }
                    for item in ranked_records
                ],
                "min_capture_time": min(capture_times) if capture_times else None,
                "max_capture_time": max(capture_times) if capture_times else None,
                "time_span_seconds": round(time_span_seconds, 3),
                "debug": {
                    "culling_preset": culling_preset,
                    "thresholds": {
                        "phash_hamming_threshold": phash_hamming_threshold,
                        "duplicate_distance": round(duplicate_distance_threshold, 4),
                        "burst_distance": round(burst_distance_threshold, 4),
                        "duplicate_time_window_seconds": duplicate_time_window_seconds,
                        "time_window_seconds": time_window_seconds,
                    },
                    "pairwise_distances": pair_distances,
                    "pairwise_phash_distances": pair_phash_distances,
                    "edge_types": sorted(group_edge_types),
                },
            }
        )
        group_counter += 1

    groups.sort(
        key=lambda group: (
            group["min_capture_time"] is None,
            group["min_capture_time"]
            if group["min_capture_time"] is not None
            else float("inf"),
            group["primary_photo_id"],
        )
    )

    if metadata_updates:
        if collection is None:
            raise DatabaseNotReadyError(
                "Cannot update metadata: database not initialized (DB_PATH missing)."
            )
        update_ids = [photo_id for photo_id, _ in metadata_updates]
        update_metadatas = [metadata for _, metadata in metadata_updates]
        collection.update(ids=update_ids, metadatas=update_metadatas)

    logger.info(
        "Grouped %s photos into %s groups (preset=%s, phash_hamming=%s, duplicate_distance=%s, burst_distance=%s, time_window=%ss)",
        len(unique_photo_ids),
        len(groups),
        culling_preset,
        phash_hamming_threshold,
        round(duplicate_distance_threshold, 4),
        round(burst_distance_threshold, 4),
        time_window_seconds,
    )
    return groups


# Batch size for get() when scanning candidates (Chroma may have limits on large id lists)
FIND_SIMILAR_BATCH_SIZE = 5000


def find_similar_to_photo(
    photo_id,
    scope_photo_ids=None,
    max_results=100,
    phash_max_hamming=10,
    use_clip=True,
    catalog_id=None,
):
    """
    Find indexed photos similar to the given photo by perceptual hash (and optionally CLIP).

    Args:
        photo_id: The reference photo ID (must be indexed and have cull_phash/phash).
        scope_photo_ids: Optional list of candidate photo IDs to consider. If None, uses all
            indexed photos for the catalog (when catalog_id is set) or all indexed photos.
        max_results: Maximum number of similar photos to return.
        phash_max_hamming: Maximum Hamming distance for perceptual hash (0–64). Lower = stricter.
        use_clip: If True, also use CLIP embedding distance to rank; requires embeddings.
        catalog_id: Optional catalog filter for scope when scope_photo_ids is None.

    Returns:
        Tuple of (results, warning):
          - results: List of dicts [{"photo_id", "phash_distance", "clip_distance"}, ...]
            sorted by phash_distance then clip_distance. Excludes the reference photo_id.
          - warning: str if the reference photo could not be found/indexed, else None.
    """
    _ensure_initialized()
    if collection is None:
        return [], "Search index not initialized (DB_PATH missing)."
    photo_id = _normalize_photo_id(photo_id)
    if not photo_id:
        logger.warning("find_similar_to_photo: empty or invalid photo_id")
        return [], "empty or invalid photo_id"

    scope_source = (
        "scope_photo_ids (%s)" % len(scope_photo_ids)
        if scope_photo_ids is not None
        else ("catalog_id=%s" % (catalog_id or "all"))
    )
    logger.info(
        "find_similar_to_photo: photo_id=%s max_results=%s phash_max_hamming=%s use_clip=%s scope=%s",
        photo_id,
        max_results,
        phash_max_hamming,
        use_clip,
        scope_source,
    )

    target_data = get_image(photo_id, catalog_id=catalog_id)
    if not target_data or not target_data.get("ids"):
        logger.warning(
            "find_similar_to_photo: reference photo_id %s not found or not in catalog",
            photo_id,
        )
        return [], "This photo is not in the search index. Run 'Analyze & Index' first."
    target_meta = (target_data.get("metadatas") or [None])[0] or {}
    target_phash = _phash_to_int(
        target_meta.get("cull_phash") or target_meta.get("phash")
    )
    if target_phash is None:
        logger.warning(
            "find_similar_to_photo: reference photo_id %s has no phash; run Analyze & Index first",
            photo_id,
        )
        return [], "This photo has no perceptual hash. Run 'Analyze & Index' first."

    target_embedding = None
    if use_clip:
        first_emb = _first_result_item(target_data.get("embeddings"))
        if first_emb is not None:
            target_embedding = _embedding_to_array(first_emb)
    logger.info(
        "find_similar_to_photo: reference has phash, use_clip_embedding=%s",
        target_embedding is not None,
    )

    if scope_photo_ids is not None:
        candidate_ids = [
            str(pid).strip()
            for pid in scope_photo_ids
            if pid and str(pid).strip() != photo_id
        ]
    else:
        candidate_ids = get_all_image_ids(catalog_id=catalog_id)
        candidate_ids = [pid for pid in candidate_ids if pid != photo_id]

    logger.info(
        "find_similar_to_photo: %s candidate photo(s) to compare", len(candidate_ids)
    )
    if not candidate_ids:
        return [], None

    results = []
    for start in range(0, len(candidate_ids), FIND_SIMILAR_BATCH_SIZE):
        batch = candidate_ids[start : start + FIND_SIMILAR_BATCH_SIZE]
        raw = collection.get(ids=batch, include=["metadatas", "embeddings"])
        ids_list = raw.get("ids") or []
        metas = raw.get("metadatas") or [{}] * len(ids_list)
        embs = raw.get("embeddings")
        for idx, pid in enumerate(ids_list):
            meta = metas[idx] if idx < len(metas) else {}
            cand_phash = _phash_to_int(meta.get("cull_phash") or meta.get("phash"))
            phash_dist = (
                _phash_hamming_distance(target_phash, cand_phash)
                if cand_phash is not None
                else None
            )
            if phash_dist is None or phash_dist > phash_max_hamming:
                continue
            clip_dist = None
            if use_clip and target_embedding is not None and embs is not None:
                cand_emb = _embedding_to_array(embs[idx] if idx < len(embs) else None)
                if cand_emb is not None:
                    clip_dist = _cosine_distance(target_embedding, cand_emb)
            results.append(
                {
                    "photo_id": pid,
                    "phash_distance": phash_dist,
                    "clip_distance": clip_dist,
                }
            )

    results.sort(
        key=lambda r: (
            r["phash_distance"],
            r["clip_distance"] if r["clip_distance"] is not None else float("inf"),
        )
    )
    out = results[:max_results]
    logger.info(
        "find_similar_to_photo: %s similar photo(s) found (phash_distance <= %s)",
        len(out),
        phash_max_hamming,
    )
    return out, None


def find_similar_to_photo_by_clip(
    photo_id, scope_photo_ids=None, max_results=100, catalog_id=None
):
    """
    Find indexed photos semantically similar to the given photo by CLIP embedding (k-NN).

    Returns:
        Tuple of (results, warning):
          - results: List of {"photo_id", "phash_distance": None, "clip_distance"} sorted by clip_distance.
            Excludes the reference photo_id.
          - warning: str if the reference photo could not be found/indexed, else None.
    """
    _ensure_initialized()
    if collection is None:
        return [], "Search index not initialized (DB_PATH missing)."
    photo_id = _normalize_photo_id(photo_id)
    if not photo_id:
        logger.warning("find_similar_to_photo_by_clip: empty or invalid photo_id")
        return [], "empty or invalid photo_id"

    target_data = get_image(photo_id, catalog_id=catalog_id)
    if not target_data or not target_data.get("ids"):
        logger.warning(
            "find_similar_to_photo_by_clip: reference photo_id %s not found or not in catalog",
            photo_id,
        )
        return [], "This photo is not in the search index. Run 'Analyze & Index' first."
    first_emb = _first_result_item(target_data.get("embeddings"))
    if first_emb is None:
        logger.warning(
            "find_similar_to_photo_by_clip: reference photo_id %s has no embedding; run Analyze & Index with embeddings",
            photo_id,
        )
        return (
            [],
            "This photo has no image embedding. Run 'Analyze & Index' with embeddings enabled.",
        )
    query_embedding = _embedding_to_array(first_emb)
    if query_embedding is None:
        return [], None

    where_clause = None
    if scope_photo_ids is not None and len(scope_photo_ids) > 0:
        ids_list = [
            str(pid).strip()
            for pid in scope_photo_ids
            if pid and str(pid).strip() != photo_id
        ]
        if not ids_list:
            return [], None
        where_clause = {"photo_id": {"$in": ids_list}}

    n_fetch = max_results + 1
    result = query_images(
        query_embedding,
        n_fetch,
        where_clause=where_clause,
        catalog_id=catalog_id,
    )
    ids0 = result.get("ids") and result["ids"][0]
    dist0 = result.get("distances") and result["distances"][0]
    if not ids0 or not dist0:
        logger.info("find_similar_to_photo_by_clip: no results from query")
        return [], None
    out = []
    for i, pid in enumerate(ids0):
        if pid == photo_id:
            continue
        d = dist0[i] if i < len(dist0) else None
        if d is not None:
            out.append(
                {"photo_id": pid, "phash_distance": None, "clip_distance": float(d)}
            )
        if len(out) >= max_results:
            break
    logger.info(
        "find_similar_to_photo_by_clip: %s similar photo(s) found by CLIP", len(out)
    )
    return out, None


# --- Face embeddings collection API ---


def add_face(
    face_id, embedding, photo_uuid, thumbnail_b64, person_id="", extra_metadata=None
):
    """
    Add a single face to the face_embeddings collection.

    Args:
        face_id: Unique id for this face (e.g. photo_uuid + '_' + index).
        embedding: 512-dim list of floats (L2-normalized).
        photo_uuid: UUID of the source photo.
        thumbnail_b64: Base64-encoded JPEG of the face crop.
        person_id: Optional person cluster id (empty until clustering assigns one).
    """
    _ensure_initialized()
    if face_collection is None:
        raise DatabaseNotReadyError(
            "Cannot add face: database not initialized (DB_PATH missing)."
        )
    metadata = {
        "photo_id": photo_uuid,
        "photo_uuid": photo_uuid,
        "thumbnail": thumbnail_b64,
        "person_id": person_id,
    }
    if extra_metadata:
        metadata.update(extra_metadata)
    face_collection.add(ids=[face_id], embeddings=[embedding], metadatas=[metadata])


def add_faces_batch(
    face_ids,
    embeddings,
    photo_uuids,
    thumbnails_b64,
    person_ids=None,
    extra_metadatas=None,
):
    """
    Add multiple faces in one call. All lists must have the same length.
    person_ids: optional list of person_id (default "" for each).
    """
    _ensure_initialized()
    if face_collection is None:
        raise DatabaseNotReadyError(
            "Cannot add faces batch: database not initialized (DB_PATH missing)."
        )
    if not face_ids:
        return
    if person_ids is None:
        person_ids = [""] * len(face_ids)
    if extra_metadatas is None:
        extra_metadatas = [{}] * len(face_ids)
    metadatas = []
    for pu, tb, pid, extra_meta in zip(
        photo_uuids, thumbnails_b64, person_ids, extra_metadatas
    ):
        metadata = {"photo_id": pu, "photo_uuid": pu, "thumbnail": tb, "person_id": pid}
        if extra_meta:
            metadata.update(extra_meta)
        metadatas.append(metadata)
    face_collection.add(ids=face_ids, embeddings=embeddings, metadatas=metadatas)


def get_all_faces(include_embeddings=True):
    """
    Get all face records. Returns dict with ids, embeddings (if requested), metadatas.
    """
    _ensure_initialized()
    if face_collection is None:
        return {"ids": [], "metadatas": [], "embeddings": []}
    include = ["metadatas"]
    if include_embeddings:
        include.append("embeddings")
    return face_collection.get(include=include)


def get_first_face_thumbnail_for_person(person_id: str) -> str:
    """
    Return base64-encoded JPEG thumbnail from one face assigned to person_id, or "".
    Uses a single Chroma metadata query (no full face scan).
    """
    if not person_id:
        return ""
    _ensure_initialized()
    if face_collection is None:
        return ""
    try:
        result = face_collection.get(
            where={"person_id": person_id},
            include=["metadatas"],
            limit=1,
        )
    except Exception as e:
        logger.warning(f"get_first_face_thumbnail_for_person({person_id!r}): {e}")
        return ""
    metas = result.get("metadatas") or []
    if not metas:
        return ""
    return (metas[0] or {}).get("thumbnail") or ""


# ChromaDB has a max batch size (~5461); stay safely below it.
FACE_UPDATE_BATCH_SIZE = 5000


def update_face_metadatas(face_ids, metadatas):
    """
    Update metadata for the given face ids. Each metadata dict must contain
    at least photo_uuid, thumbnail, person_id (full replacement per document).
    Processes in batches to respect ChromaDB's max batch size limit.
    """
    _ensure_initialized()
    if face_collection is None:
        raise DatabaseNotReadyError(
            "Cannot update face metadatas: database not initialized (DB_PATH missing)."
        )
    if not face_ids or len(face_ids) != len(metadatas):
        return
    for i in range(0, len(face_ids), FACE_UPDATE_BATCH_SIZE):
        chunk_ids = face_ids[i : i + FACE_UPDATE_BATCH_SIZE]
        chunk_meta = metadatas[i : i + FACE_UPDATE_BATCH_SIZE]
        face_collection.update(ids=chunk_ids, metadatas=chunk_meta)


def has_faces_for_photo(photo_uuid):
    """Return True if the photo has any face embeddings in the collection."""
    _ensure_initialized()
    if face_collection is None:
        return False
    try:
        result = face_collection.get(
            where={"photo_id": photo_uuid}, include=[], limit=1
        )
        if len(result.get("ids", [])) > 0:
            return True
        legacy = face_collection.get(
            where={"photo_uuid": photo_uuid}, include=[], limit=1
        )
        return len(legacy.get("ids", [])) > 0
    except Exception as e:
        logger.warning(f"Could not check faces for {photo_uuid}: {e}")
        return False


def faces_checked_for_photo(photo_uuid):
    """Return True if faces were already checked for this photo (found or not).
    Avoids re-running face detection on photos with no faces."""
    _ensure_initialized()
    if collection is None or face_collection is None:
        return False
    if has_faces_for_photo(photo_uuid):
        return True
    try:
        img = collection.get(ids=[photo_uuid], include=["metadatas"])
        if img and img.get("metadatas") and img["metadatas"]:
            meta = img["metadatas"][0]
            return meta.get("faces_checked", False)
    except Exception as e:
        logger.warning(f"Could not check faces_checked for {photo_uuid}: {e}")
    return False


def set_faces_checked(photo_uuid):
    """Mark that face detection was run for this photo (e.g. no faces found)."""
    _ensure_initialized()
    if collection is None:
        raise DatabaseNotReadyError(
            "Cannot set faces checked: database not initialized (DB_PATH missing)."
        )
    try:
        img = collection.get(ids=[photo_uuid], include=["metadatas"])
        if not img or not img.get("ids"):
            return
        meta = (img.get("metadatas") or [{}])[0].copy()
        meta["faces_checked"] = True
        collection.update(ids=[photo_uuid], metadatas=[meta])
    except Exception as e:
        logger.warning(f"Could not set faces_checked for {photo_uuid}: {e}")


def delete_faces_by_photo_uuid(photo_uuid):
    """Remove all face entries that belong to the given photo UUID."""
    _ensure_initialized()
    if face_collection is None:
        raise DatabaseNotReadyError(
            "Cannot delete faces: database not initialized (DB_PATH missing)."
        )
    try:
        face_collection.delete(where={"photo_id": photo_uuid})
    except Exception:
        pass
    try:
        face_collection.delete(where={"photo_uuid": photo_uuid})
        logger.info(f"Deleted face embeddings for photo_uuid={photo_uuid}.")
    except Exception as e:
        logger.warning(f"Delete faces for photo_uuid={photo_uuid}: {e}")


def migrate_photo_ids(
    id_mappings,
    *,
    update_faces=True,
    update_vertex=True,
    overwrite=False,
    dry_run=False,
):
    """Migrate existing DB entries from old IDs (uuid) to new photo_id values.

    Args:
        id_mappings: list of {"old_id": "...", "new_id": "..."} dicts.
    """
    _ensure_initialized()
    if collection is None:
        return {
            "requested": len(id_mappings or []),
            "migrated": 0,
            "skipped": 0,
            "missing_old": 0,
            "conflicts": 0,
            "errors": 0,
        }
    total_requested = len(id_mappings or [])
    summary = {
        "requested": total_requested,
        "migrated": 0,
        "skipped": 0,
        "missing_old": 0,
        "conflicts": 0,
        "errors": 0,
    }
    logger.info(
        "Starting photo_id migration: requested=%s overwrite=%s dry_run=%s update_faces=%s update_vertex=%s",
        total_requested,
        overwrite,
        dry_run,
        update_faces,
        update_vertex,
    )
    if not id_mappings:
        logger.info("Photo_id migration finished immediately: no mappings provided")
        return summary

    progress_interval = 100

    for idx, item in enumerate(id_mappings, start=1):
        old_id = _normalize_photo_id(item.get("old_id") or item.get("old_uuid"))
        new_id = _normalize_photo_id(item.get("new_id") or item.get("new_photo_id"))
        if not old_id or not new_id:
            summary["skipped"] += 1
        elif old_id == new_id:
            summary["skipped"] += 1
        else:
            try:
                old_rec = get_image(old_id)
                if not old_rec or not old_rec.get("ids"):
                    summary["missing_old"] += 1
                else:
                    new_rec = get_image(new_id)
                    if new_rec and new_rec.get("ids") and not overwrite:
                        summary["conflicts"] += 1
                    elif dry_run:
                        summary["migrated"] += 1
                    else:
                        old_metadata = (
                            _first_result_item(old_rec.get("metadatas"), {}) or {}
                        )
                        old_embedding = _first_result_item(old_rec.get("embeddings"))
                        merged_metadata = dict(old_metadata)
                        merged_metadata[LEGACY_UUID_FIELD] = old_id
                        merged_metadata[PHOTO_ID_FIELD] = new_id

                        if new_rec and new_rec.get("ids"):
                            update_image(
                                new_id, merged_metadata, embedding=old_embedding
                            )
                        else:
                            add_image(
                                new_id,
                                old_embedding,
                                merged_metadata,
                                legacy_uuid=old_id,
                            )
                        delete_image(old_id)

                        if update_vertex:
                            old_v = get_vertex_image(old_id)
                            if old_v and old_v.get("ids"):
                                old_v_emb = _first_result_item(old_v.get("embeddings"))
                                old_v_meta = (
                                    _first_result_item(old_v.get("metadatas"), {}) or {}
                                )
                                old_v_meta = _ensure_photo_metadata(new_id, old_v_meta)
                                old_v_meta[LEGACY_UUID_FIELD] = old_id
                                if old_v_emb is not None:
                                    add_vertex_image(
                                        new_id,
                                        old_v_emb,
                                        old_v_meta,
                                        legacy_uuid=old_id,
                                    )
                                    delete_vertex_image(old_id)

                        if update_faces:
                            face_data = face_collection.get(
                                where={"photo_uuid": old_id}, include=["metadatas"]
                            )
                            face_ids = face_data.get("ids", []) or []
                            metas = face_data.get("metadatas", []) or []
                            if face_ids and metas:
                                new_metas = []
                                for m in metas:
                                    nm = dict(m or {})
                                    nm["photo_uuid"] = new_id
                                    nm["photo_id"] = new_id
                                    new_metas.append(nm)
                                update_face_metadatas(face_ids, new_metas)

                        summary["migrated"] += 1
            except Exception as e:
                logger.error(
                    "Failed to migrate photo ID %s -> %s: %s",
                    old_id,
                    new_id,
                    e,
                    exc_info=True,
                )
                summary["errors"] += 1

        if idx == 1 or idx % progress_interval == 0 or idx == total_requested:
            logger.info(
                "Photo_id migration progress: %s/%s processed, migrated=%s skipped=%s missing_old=%s conflicts=%s errors=%s",
                idx,
                total_requested,
                summary["migrated"],
                summary["skipped"],
                summary["missing_old"],
                summary["conflicts"],
                summary["errors"],
            )
    logger.info("Photo_id migration finished: %s", summary)
    return summary


def query_faces(query_embedding, n_results, where_clause=None):
    """
    Query the face_embeddings collection by embedding.
    Returns ids, distances, metadatas (each list of lists from Chroma).
    """
    _ensure_initialized()
    try:
        return face_collection.query(
            where=where_clause,
            query_embeddings=[query_embedding],
            n_results=n_results,
            include=["metadatas", "distances"],
        )
    except Exception as e:
        logger.error(f"Error querying face_embeddings: {e}", exc_info=True)
        return {"ids": [[]], "distances": [[]], "metadatas": [[]]}
