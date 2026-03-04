import chromadb
from chromadb.config import Settings
import os
import numpy as np
from config import DB_PATH, logger


# --- ChromaDB Client and Collection Initialization (Lazy) ---
chroma_client = None
collection = None
face_collection = None

# InsightFace embeddings are 512-dimensional
FACE_EMBEDDING_DIM = 512

# Max limit for get() when counting; Chroma may apply a default limit otherwise
STATS_GET_LIMIT = 2_000_000

def _ensure_initialized():
    """Initialize ChromaDB client and collections on first use (lazy loading)."""
    global chroma_client, collection, face_collection
    if chroma_client is not None:
        return
    
    logger.info("Initializing ChromaDB client (lazy)...")
    chroma_client = chromadb.PersistentClient(path=DB_PATH, settings=Settings(anonymized_telemetry=False))
    
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


def add_image(uuid, embedding, metadata):
    """Add a new image record to the Chroma collection.

    embedding may be None for metadata-only records; in that case we add
    a dummy zero vector with the expected dimensionality (1152) to satisfy
    ChromaDB's requirements while still allowing metadata-only storage.
    
    Note: Metadata-only entries are marked with has_embedding=False in their
    metadata and are filtered out of semantic search results in service_search.py.
    They can still be found via metadata keyword searches.
    """
    _ensure_initialized()
    try:
        if embedding is None:
            # Add metadata-only record with a dummy zero embedding
            # The collection expects 1152-dimensional embeddings (from vision model)
            dummy_embedding = np.zeros(1152, dtype=np.float32).tolist()
            collection.add(embeddings=[dummy_embedding], metadatas=[metadata], ids=[uuid])
        else:
            collection.add(embeddings=[embedding], metadatas=[metadata], ids=[uuid])
    except Exception as e:
        # Surface a helpful log message and re-raise so callers can decide what to do.
        logger.error(f"Failed to add image {uuid} to ChromaDB (embedding provided: {embedding is not None}): {e}", exc_info=True)
        raise


def update_image(uuid, metadata, embedding=None):
    _ensure_initialized()
    if embedding is not None:
        collection.update(ids=[uuid], metadatas=[metadata], embeddings=[embedding])
    else:
        collection.update(ids=[uuid], metadatas=[metadata])


def get_image(uuid):
    _ensure_initialized()
    return collection.get(ids=[uuid], include=['metadatas', 'embeddings'])


def delete_image(uuid):
    _ensure_initialized()
    collection.delete(ids=[uuid])


def query_images(query_embedding, n_results, where_clause=None):
    _ensure_initialized()
    try:
        return collection.query(
            where=where_clause,
            query_embeddings=query_embedding,
            n_results=n_results,
            include=['metadatas', 'distances']
        )
    except Exception as e:
        logger.error(f"Error querying images: {e}", exc_info=True)
        return {'ids': [[]], 'distances': [[]], 'metadatas': [[]]}

def get_image_count():
    """Return total number of indexed images (photos) in the collection."""
    _ensure_initialized()
    return len(collection.get(include=[], limit=STATS_GET_LIMIT)["ids"])


def get_face_count():
    """Return total number of face embeddings in the face collection."""
    _ensure_initialized()
    return len(face_collection.get(include=[], limit=STATS_GET_LIMIT)["ids"])


def get_image_metadata_stats():
    """
    Return counts of images by metadata presence (no embeddings loaded).
    Returns dict: total, with_embedding, with_title, with_caption, with_keywords, with_quality_score.
    """
    _ensure_initialized()
    result = collection.get(include=["metadatas"], limit=STATS_GET_LIMIT)
    ids = result.get("ids", [])
    metadatas = result.get("metadatas", []) or []
    total = len(ids)
    with_embedding = 0
    with_title = 0
    with_caption = 0
    with_keywords = 0
    with_quality_score = 0
    for m in metadatas:
        if m.get("has_embedding", True):
            with_embedding += 1
        if (m.get("title") or "").strip():
            with_title += 1
        if (m.get("caption") or "").strip():
            with_caption += 1
        if (m.get("keywords") or m.get("flattened_keywords") or "").strip():
            with_keywords += 1
        if m.get("overall_score") is not None:
            with_quality_score += 1
    return {
        "total": total,
        "with_embedding": with_embedding,
        "with_title": with_title,
        "with_caption": with_caption,
        "with_keywords": with_keywords,
        "with_quality_score": with_quality_score,
    }


def get_all_image_ids(has_embedding=None):
    """Get all image IDs, optionally filtered by embedding status.
    
    Args:
        has_embedding: If True, only return IDs with real embeddings.
                      If False, only return IDs with dummy embeddings.
                      If None, return all IDs.
    """
    _ensure_initialized()
    if has_embedding is None:
        return collection.get(include=[])['ids']
    
    # Need to get metadata to filter by has_embedding flag
    result = collection.get(include=['metadatas'])
    filtered_ids = []
    
    for i, metadata in enumerate(result['metadatas']):
        # Default to True for backwards compatibility with existing entries
        has_emb = metadata.get('has_embedding', True) if metadata else True
        if has_emb == has_embedding:
            filtered_ids.append(result['ids'][i])
    
    return filtered_ids


def group_and_sort_images(uuids, phash_threshold, clip_threshold, time_delta):
    """
    [NOT IMPLEMENTED] Groups a list of images by similarity and sorts them by quality.
    """
    logger.warning("group_and_sort_images is not yet implemented.")
    return []


# --- Face embeddings collection API ---

def add_face(face_id, embedding, photo_uuid, thumbnail_b64, person_id=""):
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
    metadata = {"photo_uuid": photo_uuid, "thumbnail": thumbnail_b64, "person_id": person_id}
    face_collection.add(ids=[face_id], embeddings=[embedding], metadatas=[metadata])


def add_faces_batch(face_ids, embeddings, photo_uuids, thumbnails_b64, person_ids=None):
    """
    Add multiple faces in one call. All lists must have the same length.
    person_ids: optional list of person_id (default "" for each).
    """
    _ensure_initialized()
    if not face_ids:
        return
    if person_ids is None:
        person_ids = [""] * len(face_ids)
    metadatas = [
        {"photo_uuid": pu, "thumbnail": tb, "person_id": pid}
        for pu, tb, pid in zip(photo_uuids, thumbnails_b64, person_ids)
    ]
    face_collection.add(ids=face_ids, embeddings=embeddings, metadatas=metadatas)


def get_all_faces(include_embeddings=True):
    """
    Get all face records. Returns dict with ids, embeddings (if requested), metadatas.
    """
    _ensure_initialized()
    include = ["metadatas"]
    if include_embeddings:
        include.append("embeddings")
    return face_collection.get(include=include)


# ChromaDB has a max batch size (~5461); stay safely below it.
FACE_UPDATE_BATCH_SIZE = 5000


def update_face_metadatas(face_ids, metadatas):
    """
    Update metadata for the given face ids. Each metadata dict must contain
    at least photo_uuid, thumbnail, person_id (full replacement per document).
    Processes in batches to respect ChromaDB's max batch size limit.
    """
    _ensure_initialized()
    if not face_ids or len(face_ids) != len(metadatas):
        return
    for i in range(0, len(face_ids), FACE_UPDATE_BATCH_SIZE):
        chunk_ids = face_ids[i : i + FACE_UPDATE_BATCH_SIZE]
        chunk_meta = metadatas[i : i + FACE_UPDATE_BATCH_SIZE]
        face_collection.update(ids=chunk_ids, metadatas=chunk_meta)


def has_faces_for_photo(photo_uuid):
    """Return True if the photo has any face embeddings in the collection."""
    _ensure_initialized()
    try:
        result = face_collection.get(where={"photo_uuid": photo_uuid}, include=[], limit=1)
        return len(result.get("ids", [])) > 0
    except Exception as e:
        logger.warning(f"Could not check faces for {photo_uuid}: {e}")
        return False


def faces_checked_for_photo(photo_uuid):
    """Return True if faces were already checked for this photo (found or not).
    Avoids re-running face detection on photos with no faces."""
    _ensure_initialized()
    if has_faces_for_photo(photo_uuid):
        return True
    try:
        img = collection.get(ids=[photo_uuid], include=['metadatas'])
        if img and img.get('metadatas') and img['metadatas']:
            meta = img['metadatas'][0]
            return meta.get('faces_checked', False)
    except Exception as e:
        logger.warning(f"Could not check faces_checked for {photo_uuid}: {e}")
    return False


def set_faces_checked(photo_uuid):
    """Mark that face detection was run for this photo (e.g. no faces found)."""
    _ensure_initialized()
    try:
        img = collection.get(ids=[photo_uuid], include=['metadatas'])
        if not img or not img.get('ids'):
            return
        meta = (img.get('metadatas') or [{}])[0].copy()
        meta['faces_checked'] = True
        collection.update(ids=[photo_uuid], metadatas=[meta])
    except Exception as e:
        logger.warning(f"Could not set faces_checked for {photo_uuid}: {e}")


def delete_faces_by_photo_uuid(photo_uuid):
    """Remove all face entries that belong to the given photo UUID."""
    _ensure_initialized()
    try:
        face_collection.delete(where={"photo_uuid": photo_uuid})
        logger.info(f"Deleted face embeddings for photo_uuid={photo_uuid}.")
    except Exception as e:
        logger.warning(f"Delete faces for photo_uuid={photo_uuid}: {e}")


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

