from . import chroma as chroma_service
from . import vertexai as vertexai_service
from config import logger, TORCH_DEVICE
import server_lifecycle as server_lifecycle
import torch
import torch.nn.functional as F

DEFAULT_MAX_RESULTS = 300
MAX_RESULTS_HARD_LIMIT = 2000
MIN_RESULTS_HARD_LIMIT = 10

DEFAULT_RELEVANCE_STRICTNESS = 50
RELEVANCE_FLOOR = 8
RELEVANCE_KNEE_SCAN_LIMIT = 100
RELEVANCE_KNEE_GAP_RATIO = 0.15


def _clamp_max_results(value):
    """Clamp the requested max_results to a safe range. Falls back to default on bad input."""
    if value is None:
        return DEFAULT_MAX_RESULTS
    try:
        n = int(value)
    except (TypeError, ValueError):
        return DEFAULT_MAX_RESULTS
    if n < MIN_RESULTS_HARD_LIMIT:
        return MIN_RESULTS_HARD_LIMIT
    if n > MAX_RESULTS_HARD_LIMIT:
        return MAX_RESULTS_HARD_LIMIT
    return n


def _clamp_strictness(value):
    """Clamp the strictness slider (0..100). Falls back to default on bad input."""
    if value is None:
        return DEFAULT_RELEVANCE_STRICTNESS
    try:
        n = int(value)
    except (TypeError, ValueError):
        return DEFAULT_RELEVANCE_STRICTNESS
    if n < 0:
        return 0
    if n > 100:
        return 100
    return n


def _strictness_to_k(strictness):
    """Map a 0..100 strictness slider value to a k-factor used in
    `cutoff = noise_median - k * noise_spread`. Linear interpolation between
    anchor points: 25->0.5, 50->1.0, 75->1.5, 100->2.0. 0 disables filtering."""
    s = max(0, min(100, int(strictness)))
    if s == 0:
        return 0.0
    return 0.5 + (s - 25) * (1.5 / 75.0)


def _smart_filter_by_relevance(transformed_results, strictness, *, label=""):
    """Adaptive per-provider relevance filter.

    Operates on a list of {"photo_id", "distance"} dicts (already sorted by
    distance ascending). Uses the bottom half of the result set as a noise
    baseline (median + IQR), then keeps results whose distance is meaningfully
    smaller than that noise. A knee/gap pass force-includes a clearly relevant
    leading cluster even if it would otherwise straddle the cutoff. A hard
    floor ensures small/sparse datasets never return empty.

    strictness:
        0   -> filter disabled, return input unchanged
        100 -> very strict
    """
    if not transformed_results:
        return transformed_results

    s = _clamp_strictness(strictness)
    if s == 0:
        return transformed_results

    n = len(transformed_results)
    if n <= RELEVANCE_FLOOR:
        return transformed_results

    distances = [r["distance"] for r in transformed_results]

    tail_start = n // 2
    tail = distances[tail_start:]
    sorted_tail = sorted(tail)
    m = len(sorted_tail)
    noise_median = sorted_tail[m // 2]
    q1 = sorted_tail[m // 4]
    q3 = sorted_tail[(3 * m) // 4]
    noise_spread = max(q3 - q1, 1e-6)

    k = _strictness_to_k(s)
    stat_cutoff = noise_median - k * noise_spread

    knee_index = None
    scan_limit = min(RELEVANCE_KNEE_SCAN_LIMIT, n - 1)
    best_ratio = 0.0
    for i in range(scan_limit):
        d_here = distances[i]
        d_next = distances[i + 1]
        ratio = (d_next - d_here) / max(d_here, 1e-6)
        if ratio > best_ratio and ratio >= RELEVANCE_KNEE_GAP_RATIO:
            best_ratio = ratio
            knee_index = i + 1

    # A knee (clear distance gap) is strong evidence of where relevance ends —
    # prefer it over the statistical cutoff when found. The stat cutoff is the
    # fallback for smooth distributions with no obvious break.
    if knee_index is not None:
        cutoff = distances[knee_index - 1]
    else:
        cutoff = stat_cutoff

    kept = [r for r in transformed_results if r["distance"] <= cutoff]

    if len(kept) < RELEVANCE_FLOOR:
        kept = transformed_results[:RELEVANCE_FLOOR]

    logger.info(
        "Smart filter [%s]: in=%d kept=%d strictness=%d k=%.2f "
        "noise_median=%.4f noise_spread=%.4f knee_index=%s cutoff=%.4f",
        label or "?",
        n,
        len(kept),
        s,
        k,
        noise_median,
        noise_spread,
        str(knee_index),
        cutoff,
    )
    return kept


def _merge_semantic_results(siglip_results, vertex_results):
    """Merge SigLIP and Vertex results without mixing distance scales.
    Vertex (higher quality) first, sorted by Vertex distance; then SigLIP
    results not in Vertex, sorted by SigLIP distance. No cross-model distance comparison."""
    vertex_ids = {r["photo_id"] for r in vertex_results}
    siglip_only = [r for r in siglip_results if r["photo_id"] not in vertex_ids]
    return list(vertex_results) + siglip_only


def _transform_and_sort_results(results, quality_sort):
    """Transforms ChromaDB results and sorts them based on quality or distance."""
    if not results or not results["ids"][0]:
        return []

    ids, distances, metadatas = (
        results["ids"][0],
        results["distances"][0],
        results["metadatas"][0],
    )

    transformed_results = []
    for i in range(len(ids)):
        # Skip metadata-only entries (with dummy embeddings) from semantic search
        metadata = metadatas[i] if i < len(metadatas) else {}
        if metadata and not metadata.get("has_embedding", True):
            continue

        transformed_results.append(
            {
                "photo_id": ids[i],
                "uuid": ids[i],  # backward compatibility for older plugin responses
                "distance": float(round(distances[i], 4)),
            }
        )

    transformed_results.sort(key=lambda x: x["distance"])
    return transformed_results


def _transform_vertex_results(vertex_results):
    """Transform Vertex Chroma query result to list of {photo_id, distance} sorted by distance."""
    if (
        not vertex_results
        or not vertex_results.get("ids")
        or not vertex_results["ids"][0]
    ):
        return []
    ids, distances = vertex_results["ids"][0], vertex_results["distances"][0]
    out = [
        {"photo_id": uid, "uuid": uid, "distance": float(round(d, 4))}
        for uid, d in zip(ids, distances)
    ]
    out.sort(key=lambda x: x["distance"])
    return out


DEFAULT_METADATA_FIELDS = ["flattened_keywords", "alt_text", "caption", "title"]


def _default_search_sources():
    """Return default search sources: all enabled, all metadata fields."""
    return {
        "semantic_siglip": True,
        "semantic_vertex": True,
        "metadata": True,
        "metadata_fields": list(DEFAULT_METADATA_FIELDS),
    }


def _normalize_search_sources(search_sources):
    """Ensure search_sources has all keys and valid metadata_fields. None -> defaults."""
    if not search_sources:
        return _default_search_sources()
    allowed = set(DEFAULT_METADATA_FIELDS)
    out = {
        "semantic_siglip": bool(search_sources.get("semantic_siglip", True)),
        "semantic_vertex": bool(search_sources.get("semantic_vertex", True)),
        "metadata": bool(search_sources.get("metadata", True)),
    }
    raw_fields = search_sources.get("metadata_fields")
    if raw_fields and isinstance(raw_fields, list):
        out["metadata_fields"] = [f for f in raw_fields if f in allowed]
    else:
        out["metadata_fields"] = list(DEFAULT_METADATA_FIELDS)
    if not out["metadata_fields"]:
        out["metadata_fields"] = list(DEFAULT_METADATA_FIELDS)
    return out


def search_images(
    term,
    quality_sort,
    photo_ids_to_search,
    search_sources=None,
    *,
    vertex_project_id=None,
    vertex_location=None,
    catalog_id=None,
    relevance_strictness=None,
    max_results=None,
):
    sources = _normalize_search_sources(search_sources)
    n_results = _clamp_max_results(max_results)
    strictness = _clamp_strictness(relevance_strictness)
    logger.info(
        f"Searching for '{term}' (quality_sort: {quality_sort}, scoped: {photo_ids_to_search is not None}, catalog_id: {bool(catalog_id)}, sources: {sources}, max_results: {n_results}, relevance_strictness: {strictness})"
    )

    sorted_semantic_results = []
    semantic_photo_ids = set()
    warning = None

    # 1. Semantic Search (SigLIP2)
    if sources["semantic_siglip"]:
        tokenizer = server_lifecycle.get_tokenizer()
        if tokenizer:
            text_tokens = tokenizer(term).to(TORCH_DEVICE)
            with torch.no_grad():
                model = server_lifecycle.get_model()
                text_features = model.encode_text(text_tokens)
                normalized_embeddings = (
                    F.normalize(text_features, p=2, dim=1).cpu().numpy()[0]
                )

            db_results = chroma_service.query_images(
                query_embedding=normalized_embeddings,
                n_results=n_results,
                where_clause={"photo_id": {"$in": photo_ids_to_search}}
                if photo_ids_to_search
                else None,
                catalog_id=catalog_id,
            )
            if photo_ids_to_search and (
                not db_results or not db_results.get("ids") or not db_results["ids"][0]
            ):
                # Legacy fallback for unmigrated metadata
                db_results = chroma_service.query_images(
                    query_embedding=normalized_embeddings,
                    n_results=n_results,
                    where_clause={"uuid": {"$in": photo_ids_to_search}},
                    catalog_id=catalog_id,
                )

            sorted_semantic_results = _transform_and_sort_results(
                db_results, quality_sort
            )
            sorted_semantic_results = _smart_filter_by_relevance(
                sorted_semantic_results, strictness, label="siglip2"
            )
            semantic_photo_ids = {res["photo_id"] for res in sorted_semantic_results}
        else:
            warning = "SigLIP model not loaded. Semantic search results will be missing. Download the model in the plugin manager."
            logger.info(warning)

    # 1b. Vertex AI semantic search (use vertex_project_id/vertex_location from request so plugin prefs are used)
    vertex_semantic_results = []
    if (
        sources["semantic_vertex"]
        and vertexai_service.is_available(vertex_project_id, vertex_location)
        and term.strip()
    ):
        vertex_where = (
            {"photo_id": {"$in": list(photo_ids_to_search)}}
            if photo_ids_to_search
            else None
        )
        try:
            vertex_ids = chroma_service.get_all_vertex_image_ids()
            if photo_ids_to_search:
                scope_vertex = set(vertex_ids) & set(photo_ids_to_search)
            else:
                scope_vertex = set(vertex_ids)
            if scope_vertex:
                query_emb = vertexai_service.get_text_embedding(
                    term, vertex_project_id, vertex_location
                )
                if query_emb:
                    vertex_results = chroma_service.query_vertex_images(
                        query_embedding=query_emb,
                        n_results=n_results,
                        where_clause=vertex_where,
                        catalog_id=catalog_id,
                    )
                    if photo_ids_to_search and (
                        not vertex_results
                        or not vertex_results.get("ids")
                        or not vertex_results["ids"][0]
                    ):
                        vertex_results = chroma_service.query_vertex_images(
                            query_embedding=query_emb,
                            n_results=n_results,
                            where_clause={"uuid": {"$in": list(photo_ids_to_search)}},
                            catalog_id=catalog_id,
                        )
                    vertex_semantic_results = _transform_vertex_results(vertex_results)
                    vertex_semantic_results = _smart_filter_by_relevance(
                        vertex_semantic_results, strictness, label="vertex"
                    )
                    logger.info(
                        f"Vertex AI semantic search returned {len(vertex_semantic_results)} results."
                    )
        except Exception as e:
            msg = f"Vertex AI search failed: {str(e)}"
            logger.warning(msg, exc_info=True)
            if not warning:
                warning = msg
            else:
                warning += f" | {msg}"
    if vertex_semantic_results:
        sorted_semantic_results = _merge_semantic_results(
            sorted_semantic_results, vertex_semantic_results
        )
        semantic_photo_ids = {res["photo_id"] for res in sorted_semantic_results}

    # 2. Metadata Search (in-memory)
    metadata_only_results = []
    if sources["metadata"] and sources["metadata_fields"]:
        search_fields = sources["metadata_fields"]
        logger.info("Performing metadata search in-memory (fields: %s).", search_fields)

        if photo_ids_to_search:
            target_ids = list(photo_ids_to_search)
            all_metadata_raw = chroma_service.collection.get(
                ids=target_ids, include=["metadatas"]
            )
        elif catalog_id:
            target_ids = chroma_service.get_all_image_ids(catalog_id=catalog_id)
            all_metadata_raw = (
                chroma_service.collection.get(ids=target_ids, include=["metadatas"])
                if target_ids
                else {"ids": [], "metadatas": []}
            )
        else:
            all_metadata_raw = chroma_service.collection.get(include=["metadatas"])

        metadata_ids = set()
        term_lower = term.lower()

        for i, photo_id in enumerate(all_metadata_raw["ids"]):
            metadata = all_metadata_raw["metadatas"][i]
            if not metadata:
                continue

            for field in search_fields:
                if field in metadata and metadata[field] is not None:
                    if term_lower in str(metadata[field]).lower():
                        metadata_ids.add(photo_id)
                        break

        metadata_only_ids = metadata_ids - semantic_photo_ids
        metadata_only_results = [
            {"photo_id": pid, "uuid": pid, "distance": None}
            for pid in metadata_only_ids
        ]

    # 3. Combine results

    final_results = sorted_semantic_results + metadata_only_results

    logger.info(
        f"Total results: {len(final_results)} ({len(sorted_semantic_results)} semantic, {len(metadata_only_results)} metadata-only)"
    )

    return final_results, warning


def group_similar_images(
    photo_ids, phash_threshold, clip_threshold, time_delta, culling_preset="default"
):
    """Groups a list of images by similarity and sorts them by quality."""
    logger.info(
        "Grouping %s photo IDs with phash_threshold='%s', clip_threshold='%s', time_delta='%ss', culling_preset='%s'.",
        len(photo_ids),
        phash_threshold,
        clip_threshold,
        time_delta,
        culling_preset,
    )

    warning = None
    if not server_lifecycle.get_model():
        warning = "SigLIP model not loaded. Similarity grouping based on visual content will be disabled (pHASH only). Download the model in the plugin manager."
        logger.warning(warning)

    try:
        grouped_results = chroma_service.group_and_sort_images(
            photo_ids,
            phash_threshold,
            clip_threshold,
            time_delta,
            culling_preset=culling_preset,
        )
        return grouped_results, warning
    except Exception as e:
        logger.error(f"Error during similarity grouping: {str(e)}")
        raise e


def cull_images(
    photo_ids, phash_threshold, clip_threshold, time_delta, culling_preset="default"
):
    """
    High-level culling wrapper around grouping/ranking.
    Returns grouped results plus a compact summary for UI/reporting.
    """
    groups, warning = group_similar_images(
        photo_ids,
        phash_threshold,
        clip_threshold,
        time_delta,
        culling_preset=culling_preset,
    )

    picks = 0
    alternates = 0
    rejects = 0
    near_duplicate_groups = 0
    for group in groups:
        if group.get("group_type") == "near_duplicate":
            near_duplicate_groups += 1
        photos = group.get("photos") or []
        for photo in photos:
            if photo.get("winner"):
                picks += 1
            elif photo.get("reject_candidate"):
                rejects += 1
            else:
                alternates += 1

    return {
        "status": "success",
        "warning": warning,
        "summary": {
            "group_count": len(groups),
            "pick_count": picks,
            "alternate_count": alternates,
            "reject_candidate_count": rejects,
            "near_duplicate_group_count": near_duplicate_groups,
            "culling_preset": culling_preset,
        },
        "groups": groups,
    }


def find_similar_images(
    photo_id,
    scope_photo_ids=None,
    max_results=100,
    phash_max_hamming=10,
    use_clip=True,
    similarity_mode="phash",
    catalog_id=None,
):
    """
    Find indexed photos similar to the given photo.

    similarity_mode: "phash" = near-duplicates by perceptual hash (optionally ranked by CLIP);
                    "clip" = semantically similar by embedding (k-NN).
    Returns (results, warning) where:
      - results: list of {"photo_id", "phash_distance", "clip_distance"} sorted by similarity.
      - warning: str with a user-facing message when the reference photo cannot be processed,
                 or None on success.
    """
    if similarity_mode == "clip":
        return chroma_service.find_similar_to_photo_by_clip(
            photo_id=photo_id,
            scope_photo_ids=scope_photo_ids,
            max_results=max_results,
            catalog_id=catalog_id,
        )
    return chroma_service.find_similar_to_photo(
        photo_id=photo_id,
        scope_photo_ids=scope_photo_ids,
        max_results=max_results,
        phash_max_hamming=phash_max_hamming,
        use_clip=use_clip,
        catalog_id=catalog_id,
    )
