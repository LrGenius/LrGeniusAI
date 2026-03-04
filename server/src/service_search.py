import numpy as np

import service_chroma as chroma_service
from config import logger, TORCH_DEVICE
import server_lifecycle as server_lifecycle
import torch
import torch.nn.functional as F


def _filter_by_relevance(results):
    """Filters results based on a dynamically calculated relevance threshold."""
    # DISABLED: The statistical filtering is proving unreliable and too aggressive,
    # especially for smaller datasets. It has been disabled to prevent it from
    # filtering out relevant results.
    return results

def _transform_and_sort_results(results, quality_sort):
    """Transforms ChromaDB results and sorts them based on quality or distance."""
    if not results or not results['ids'][0]:
        return []

    ids, distances, metadatas = results['ids'][0], results['distances'][0], results['metadatas'][0]

    transformed_results = []
    for i in range(len(ids)):
        # Skip metadata-only entries (with dummy embeddings) from semantic search
        metadata = metadatas[i] if i < len(metadatas) else {}
        if metadata and not metadata.get('has_embedding', True):
            continue
            
        transformed_results.append({
            "uuid": ids[i],
            "distance": float(round(distances[i], 4)),
        })

    transformed_results.sort(key=lambda x: x['distance'])    
    return transformed_results

def search_images(term, quality_sort, uuids_to_search):
    logger.info(f"Searching for '{term}' (quality_sort: {quality_sort}, scoped: {uuids_to_search is not None})")

    # 1. Semantic Search
    tokenizer = server_lifecycle.get_tokenizer()
    if tokenizer:
        text_tokens = tokenizer(term).to(TORCH_DEVICE)
        with torch.no_grad():
            model = server_lifecycle.get_model()
            text_features = model.encode_text(text_tokens)
            normalized_embeddings = F.normalize(text_features, p=2, dim=1).cpu().numpy()[0]

        db_results = chroma_service.query_images(
            query_embedding=normalized_embeddings,
            n_results=300,
            where_clause={"uuid": {"$in": uuids_to_search}} if uuids_to_search else None
        )

        relevant_results = _filter_by_relevance(db_results)
        sorted_semantic_results = _transform_and_sort_results(relevant_results, quality_sort)
        semantic_uuids = {res['uuid'] for res in sorted_semantic_results}
    else:
        logger.info("CLIP model not loaded, skipping semantic search.")
        sorted_semantic_results = []
        semantic_uuids = set()

    # 2. Metadata Search (in-memory)
    logger.info("Performing metadata search in-memory. This may be slow for large databases without a UUID filter.")
    search_fields = ["flattened_keywords", "alt_text", "caption", "title"]
    
    if uuids_to_search:
        target_uuids = list(uuids_to_search)
        all_metadata_raw = chroma_service.collection.get(ids=target_uuids, include=["metadatas"])
    else:
        all_metadata_raw = chroma_service.collection.get(include=["metadatas"])

    metadata_uuids = set()
    term_lower = term.lower()
    
    for i, uuid in enumerate(all_metadata_raw['ids']):
        metadata = all_metadata_raw['metadatas'][i]
        if not metadata:
            continue
        
        for field in search_fields:
            if field in metadata and metadata[field] is not None:
                if term_lower in str(metadata[field]).lower():
                    metadata_uuids.add(uuid)
                    break 
    
    # 3. Combine results
    metadata_only_uuids = metadata_uuids - semantic_uuids
    metadata_only_results = [{"uuid": uuid, "distance": None} for uuid in metadata_only_uuids]

    final_results = sorted_semantic_results + metadata_only_results
    
    logger.info(f"Total results: {len(final_results)} ({len(sorted_semantic_results)} semantic, {len(metadata_only_results)} metadata-only)")
    
    return final_results

    
def group_similar_images(uuids, phash_threshold, clip_threshold, time_delta):
    """Groups a list of images by similarity and sorts them by quality."""
    logger.info(f"Grouping {len(uuids)} UUIDs with phash_threshold='{phash_threshold}', clip_threshold='{clip_threshold}', and time_delta='{time_delta}s'.")

    try:
        grouped_results = chroma_service.group_and_sort_images(uuids, phash_threshold, clip_threshold, time_delta)
        return grouped_results
    except Exception as e:
        logger.error(f"Error during similarity grouping: {str(e)}")
        raise e
