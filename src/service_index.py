from config import logger
import service_chroma as chroma_service
from service_metadata import get_analysis_service
import server_lifecycle as server_lifecycle
import service_face as face_service
import json
from datetime import datetime as time

def _flatten_keywords(keywords):
    """
    Flatten keywords from various formats to a comma-separated string.
    
    Handles:
    - Flat list: ["Keyword1", "Keyword2"] -> "Keyword1, Keyword2"
    - Nested dict: {"Category": ["Kw1", "Kw2"], ...} -> "Kw1, Kw2, ..."
    - Already a string: "Keyword1, Keyword2" -> "Keyword1, Keyword2"
    
    Args:
        keywords: List, dict, or string of keywords
        
    Returns:
        Comma-separated string of all keywords
    """
    if not keywords:
        return ""
    
    if isinstance(keywords, str):
        # Already a string, return as-is
        return keywords
    
    if isinstance(keywords, list):
        # Flat list of strings
        return ', '.join(str(kw) for kw in keywords if kw)
    
    if isinstance(keywords, dict):
        # Nested dict - recursively collect all keywords
        all_keywords = []
        
        def collect_keywords(d):
            for key, value in d.items():
                if isinstance(value, list):
                    # Leaf node with keywords
                    all_keywords.extend(str(kw) for kw in value if kw)
                elif isinstance(value, dict) and value:
                    # Nested dict, recurse
                    collect_keywords(value)
                else:
                    # Single keyword value
                    if value:
                        all_keywords.append(str(key))
        
        collect_keywords(keywords)
        return ', '.join(all_keywords)
    
    return ""


def get_uuids_needing_processing(uuids: list[str], options: dict) -> list[str]:
    """
    Returns UUIDs that need processing based on selected tasks and existing backend data.
    Mirrors the same logic as process_image_task for determining what's missing.
    """
    regenerate_metadata = options.get('regenerate_metadata', True)
    compute_embeddings = options.get('compute_embeddings', True)
    compute_metadata = options.get('compute_metadata', False)
    compute_quality = options.get('compute_quality', True)
    compute_faces = options.get('compute_faces', False)

    if not uuids:
        return []

    # Load existing records for all UUIDs
    existing_records = {}
    for uuid in uuids:
        existing_record = chroma_service.get_image(uuid)
        if existing_record and existing_record['ids']:
            existing_records[uuid] = existing_record['metadatas'][0] if existing_record['metadatas'] else {}

    needing_processing = []
    for uuid in uuids:
        existing = existing_records.get(uuid, {})

        needs_embedding = compute_embeddings and (regenerate_metadata or not existing.get('has_embedding', False))
        has_any_metadata = existing.get('title') or existing.get('caption') or existing.get('alt_text') or existing.get('keywords')
        needs_metadata = compute_metadata and (regenerate_metadata or not has_any_metadata)
        needs_quality = compute_quality and (regenerate_metadata or existing.get('overall_score') is None)
        needs_faces = compute_faces and (regenerate_metadata or not chroma_service.faces_checked_for_photo(uuid))

        if needs_embedding or needs_metadata or needs_quality or needs_faces:
            needing_processing.append(uuid)

    return needing_processing


def process_image_task(
    image_triplets: list[tuple[bytes, str, str]], 
    options: dict
) -> tuple[int, int]:
    """
    Process a batch of images for indexing.
    
    Args:
        image_triplets: List of (image_bytes, uuid, filename) tuples
        options: Dictionary with all processing options
        
    Returns:
        Tuple of (success_count, failure_count)
    """
    success_count = 0
    failure_count = 0
    total_images = len(image_triplets)

    try:
        provider = options.get('provider')
        model_name = options.get('model')
        replace_ss = options.get('replace_ss', False)
        regenerate_metadata = options.get('regenerate_metadata', True)
        compute_embeddings = options.get('compute_embeddings', True)
        compute_metadata = options.get('compute_metadata', False)
        compute_quality = options.get('compute_quality', True)
        compute_faces = options.get('compute_faces', False)

        logger.info(f"Starting batch processing of {total_images} images...")
        logger.info(f"regenerate_metadata={regenerate_metadata}, compute_embeddings={compute_embeddings}, "
                   f"compute_metadata={compute_metadata}, compute_quality={compute_quality}, compute_faces={compute_faces}")
        
        # Check existing records if regenerate_metadata is False
        existing_records = {}
        if not regenerate_metadata:
            logger.info("Checking existing records to determine what needs generation...")
            for _, uuid, _ in image_triplets:
                existing_record = chroma_service.get_image(uuid)
                if existing_record and existing_record['ids']:
                    existing_records[uuid] = existing_record['metadatas'][0] if existing_record['metadatas'] else {}
        
        # Determine what actually needs to be computed for each image
        images_needing_embeddings = []
        images_needing_metadata = []
        images_needing_quality = []
        images_needing_faces = []
        
        for _, uuid, _ in image_triplets:
            existing = existing_records.get(uuid, {})
            
            # Check if embedding is needed
            needs_embedding = compute_embeddings and (regenerate_metadata or not existing.get('has_embedding', False))
            if needs_embedding:
                images_needing_embeddings.append(uuid)

            # Check if faces are needed
            needs_faces = compute_faces and (regenerate_metadata or not chroma_service.faces_checked_for_photo(uuid))
            if needs_faces:
                images_needing_faces.append(uuid)
            
            # Check if metadata is needed
            has_any_metadata = existing.get('title') or existing.get('caption') or existing.get('alt_text') or existing.get('keywords')
            needs_metadata = compute_metadata and (regenerate_metadata or not has_any_metadata)
            if existing and compute_metadata:
                logger.info(f"UUID {uuid}: has_metadata={has_any_metadata}, regenerate={regenerate_metadata}, needs_metadata={needs_metadata}")
                logger.info(f"  Existing fields: title={bool(existing.get('title'))}, caption={bool(existing.get('caption'))}, "
                          f"alt_text={bool(existing.get('alt_text'))}, keywords={bool(existing.get('keywords'))}")
            if needs_metadata:
                images_needing_metadata.append(uuid)
            
            # Check if quality scores are needed
            needs_quality = compute_quality and (regenerate_metadata or not existing.get('overall_score'))
            if needs_quality:
                images_needing_quality.append(uuid)
        
        logger.info(f"Generation needed: {len(images_needing_embeddings)} embeddings, "
                   f"{len(images_needing_metadata)} metadata, {len(images_needing_quality)} quality scores, {len(images_needing_faces)} faces")

        # If nothing needs to be generated and we're not regenerating, skip work.
        # When regenerate_metadata is True we must not early-return: new images (no entry yet)
        # still need to be added to Chroma with at least minimal metadata.
        # Also do NOT early-return when compute_faces is True - we need to process images for face detection.
        if (not regenerate_metadata
                and not compute_faces
                and len(images_needing_embeddings) == 0
                and len(images_needing_metadata) == 0
                and len(images_needing_quality) == 0):
            logger.info("No generation required (regenerate_metadata=False and all fields present). Returning success without changes.")
            return len(image_triplets), 0

    
        analysis_service = get_analysis_service()
        siglip_model = None
        siglip_processor = None

        siglip_model = server_lifecycle.get_model()
        siglip_processor = server_lifecycle.get_processor()

        # Convert lists to sets for faster lookup in analyze_batch
        embeddings, datetimes, metadata_results, ratings = analysis_service.analyze_batch(
            image_triplets, options, siglip_model, siglip_processor,
            set(images_needing_embeddings), set(images_needing_metadata), set(images_needing_quality)
        )

        # Only fail batch when we actually needed embeddings but got none
        if embeddings is None and len(images_needing_embeddings) > 0:
            return 0, total_images

        for i, (image_bytes, uuid, filename) in enumerate(image_triplets):
            try:
                embedding = embeddings[i] if embeddings is not None else None
                rating_data = ratings[i] if ratings else None
                metadata_data = metadata_results[i] if metadata_results else None
                
                existing = existing_records.get(uuid, {})
                
                need_embedding = uuid in images_needing_embeddings
                need_metadata = uuid in images_needing_metadata
                need_quality = uuid in images_needing_quality

                # Validate that required new data was generated if needed
                if need_embedding and embedding is None:
                    logger.error(f"Embedding generation failed for {uuid}. Skipping.")
                    failure_count += 1
                    continue
                
                if need_quality and (not rating_data or not rating_data.success):
                    logger.error(f"Quality rating generation failed for {uuid}. Skipping.")
                    failure_count += 1
                    continue

                if need_metadata and (not metadata_data or not metadata_data.success):
                    logger.error(f"Metadata generation failed for {uuid}. Skipping.")
                    failure_count += 1
                    continue

                # If nothing needed for this UUID (already complete) and no face processing, skip
                # When compute_faces is True we must not skip - we need to reach face detection
                if (not need_embedding and not need_metadata and not need_quality
                        and not regenerate_metadata and not compute_faces):
                    logger.info(f"UUID {uuid}: already fully indexed; skipping update.")
                    success_count += 1
                    continue

                # Start with existing metadata if not regenerating
                if not regenerate_metadata and existing:
                    main_metadata = existing.copy()
                    # Update only basic fields that should always be current
                    main_metadata["filename"] = filename
                    main_metadata["uuid"] = uuid
                else:
                    main_metadata = {
                        "filename": filename,
                        "uuid": uuid,
                        "provider": provider,
                        "model": model_name,
                    }

                # Update/add datetime if present
                if datetimes:
                    capture_time = datetimes.get(uuid)
                    if capture_time:
                        main_metadata["capture_time"] = capture_time

                # Update quality scores if newly generated
                if rating_data and rating_data.success:
                    main_metadata["overall_score"] = rating_data.overall_score
                    main_metadata["composition_score"] = rating_data.composition_score
                    main_metadata["lighting_score"] = rating_data.lighting_score
                    main_metadata["motiv_score"] = rating_data.motiv_score
                    main_metadata["colors_score"] = rating_data.colors_score
                    main_metadata["emotion_score"] = rating_data.emotion_score
                    main_metadata["quality_critique"] = rating_data.critique
                    main_metadata["provider"] = provider
                    main_metadata["model"] = model_name

                # Update metadata fields if newly generated
                if metadata_data and metadata_data.success:
                    if metadata_data.title:
                        main_metadata['title'] = metadata_data.title
                    if metadata_data.caption:
                        main_metadata['caption'] = metadata_data.caption
                    if metadata_data.alt_text:
                        main_metadata['alt_text'] = metadata_data.alt_text
                    if metadata_data.keywords:
                        main_metadata['keywords'] = json.dumps(metadata_data.keywords)
                        #logger.debug(f"UUID {uuid}: keywords JSON data: {main_metadata['keywords']}")
                        main_metadata['flattened_keywords'] = _flatten_keywords(metadata_data.keywords)
                    if not main_metadata.get("provider"):
                        main_metadata["provider"] = provider
                    if not main_metadata.get("model"):
                        main_metadata["model"] = model_name
                
                main_metadata['run_date'] = time.now().strftime("%Y-%m-%d %H:%M:%S")
                
                # Update embedding status
                if embedding is not None:
                    main_metadata['has_embedding'] = True
                elif existing and existing.get('has_embedding', False):
                    # Preserve existing embedding - we didn't generate a new one (e.g. only faces)
                    main_metadata['has_embedding'] = True
                else:
                    main_metadata['has_embedding'] = False
                
                if replace_ss:
                    for key, value in main_metadata.items():
                        if isinstance(value, str):
                            main_metadata[key] = value.replace("ß", "ss")

                # Determine if we need to update the embedding
                # Only update embedding if we generated a new one
                update_embedding = embedding if embedding is not None else None
                
                if existing and not regenerate_metadata:
                    logger.info(f"UUID {uuid} already exists. Updating (embedding: {update_embedding is not None}).")
                    chroma_service.update_image(uuid, main_metadata, embedding=update_embedding)
                elif regenerate_metadata:
                    logger.info(f"UUID {uuid} set to regenerate. Updating (embedding: {update_embedding is not None}).")
                    if chroma_service.get_image(uuid) is not None:
                        chroma_service.update_image(uuid, main_metadata, embedding=update_embedding)
                    else:
                        chroma_service.add_image(uuid, embedding, main_metadata)
                else:
                    # New record
                    if embedding is not None:
                        logger.info(f"UUID {uuid} is new. Indexing with embeddings.")
                    else:
                        logger.info(f"UUID {uuid} is new. Indexing metadata-only entry (no embedding).")
                    chroma_service.add_image(uuid, embedding, main_metadata)

                # Face detection and indexing (second Chroma collection)
                if compute_faces and image_bytes:
                    # Without regenerate_metadata: skip if already checked (has faces or marked as checked, no faces)
                    if not regenerate_metadata and chroma_service.faces_checked_for_photo(uuid):
                        logger.debug(f"UUID {uuid}: faces already checked, skipping (regenerate_metadata=False).")
                    else:
                        try:
                            chroma_service.delete_faces_by_photo_uuid(uuid)
                            face_results = face_service.detect_faces(image_bytes)
                            if face_results:
                                face_ids = [f"{uuid}_{i}" for i in range(len(face_results))]
                                embeddings_f = [r[0] for r in face_results]
                                thumbnails_b64 = [r[1] for r in face_results]
                                chroma_service.add_faces_batch(
                                    face_ids, embeddings_f, [uuid] * len(face_results), thumbnails_b64
                                )
                                logger.info(f"UUID {uuid}: indexed {len(face_results)} face(s).")
                            else:
                                chroma_service.set_faces_checked(uuid)
                                logger.debug(f"UUID {uuid}: no faces detected (marked as checked).")
                        except Exception as e:
                            logger.warning(f"Face detection/indexing failed for {uuid}: {e}", exc_info=True)

                success_count += 1

            except Exception as e:
                logger.error(f"Error processing image {uuid}: {str(e)}", exc_info=True)
                failure_count += 1

        return success_count, failure_count
    except Exception as e:
        logger.error(f"Error during batch processing task: {str(e)}", exc_info=True)
        return 0, total_images
