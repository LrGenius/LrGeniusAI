from config import logger, CULLING_CONFIG
import service_chroma as chroma_service
from service_metadata import get_analysis_service
import server_lifecycle as server_lifecycle
import service_face as face_service
import service_vertexai as vertexai_service
import json
from datetime import datetime as time
from functools import lru_cache
from PIL import Image
import io
import numpy as np

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
    
    seen_keywords = set()

    def _append_unique(values, text):
        normalized = text.lower()
        if normalized in seen_keywords:
            return
        seen_keywords.add(normalized)
        values.append(text)

    def _normalize_keyword_text(value):
        if isinstance(value, str):
            text = value.strip()
            values = []
            if text:
                _append_unique(values, text)
            return values
        if isinstance(value, dict):
            values = []
            name = value.get("name")
            if isinstance(name, str) and name.strip():
                _append_unique(values, name.strip())
            synonyms = value.get("synonyms")
            if isinstance(synonyms, list):
                for synonym in synonyms:
                    if isinstance(synonym, str) and synonym.strip():
                        _append_unique(values, synonym.strip())
            return values
        return []

    if isinstance(keywords, list):
        # Flat list of strings or structured keyword objects
        flattened = []
        for kw in keywords:
            flattened.extend(_normalize_keyword_text(kw))
        return ', '.join(flattened)
    
    if isinstance(keywords, dict):
        # Nested dict - recursively collect all keywords
        all_keywords = []
        
        def collect_keywords(d):
            for key, value in d.items():
                if isinstance(value, list):
                    # Leaf node with keywords (strings or structured keyword objects)
                    for kw in value:
                        all_keywords.extend(_normalize_keyword_text(kw))
                elif isinstance(value, dict) and value:
                    if isinstance(value.get("name"), str):
                        all_keywords.extend(_normalize_keyword_text(value))
                    else:
                        # Nested dict, recurse
                        collect_keywords(value)
                else:
                    # Single keyword value
                    all_keywords.extend(_normalize_keyword_text(value))
        
        collect_keywords(keywords)
        return ', '.join(all_keywords)
    
    return ""


def _safe_unit_interval(value: float) -> float:
    return max(0.0, min(1.0, float(value)))


def _load_analysis_grayscale(image_bytes: bytes, max_side: int = 512) -> np.ndarray:
    image = Image.open(io.BytesIO(image_bytes)).convert("RGB")
    if max(image.size) > max_side:
        scale = max_side / float(max(image.size))
        resized = (
            max(32, int(round(image.size[0] * scale))),
            max(32, int(round(image.size[1] * scale))),
        )
        image = image.resize(resized, Image.Resampling.BILINEAR)
    rgb = np.asarray(image, dtype=np.float32) / 255.0
    return (0.299 * rgb[:, :, 0]) + (0.587 * rgb[:, :, 1]) + (0.114 * rgb[:, :, 2])


@lru_cache(maxsize=4)
def _build_dct_matrix(size: int) -> np.ndarray:
    indices = np.arange(size, dtype=np.float32)
    matrix = np.zeros((size, size), dtype=np.float32)
    scale = np.pi / (2.0 * float(size))
    for u in range(size):
        alpha = np.sqrt(1.0 / size) if u == 0 else np.sqrt(2.0 / size)
        matrix[u, :] = alpha * np.cos((2.0 * indices + 1.0) * u * scale)
    return matrix


def _compute_perceptual_hash(image_bytes: bytes) -> str:
    """
    Compute a classic 64-bit pHash (DCT-based) and return 16-char hex.
    Returns empty string on failure.
    """
    try:
        image = Image.open(io.BytesIO(image_bytes)).convert("L").resize((32, 32), Image.Resampling.LANCZOS)
        pixels = np.asarray(image, dtype=np.float32)
        dct_matrix = _build_dct_matrix(32)
        dct_transformed = dct_matrix @ pixels @ dct_matrix.T
        low_freq = dct_transformed[:8, :8]
        median = float(np.median(low_freq[1:, :]))
        bits = (low_freq > median).astype(np.uint8).flatten()
        hash_value = 0
        for bit in bits:
            hash_value = (hash_value << 1) | int(bit)
        return f"{hash_value:016x}"
    except Exception:
        return ""


def _compute_culling_metrics(image_bytes: bytes) -> dict:
    """
    Compute cheap, deterministic image-quality metrics for culling.
    All normalized quality scores use a 0..1 range where higher is better,
    except the explicit clip/noise fields which are stored as penalties.
    """
    try:
        image = Image.open(io.BytesIO(image_bytes)).convert("RGB")
        if max(image.size) > 512:
            scale = 512 / float(max(image.size))
            resized = (
                max(32, int(round(image.size[0] * scale))),
                max(32, int(round(image.size[1] * scale))),
            )
            image = image.resize(resized, Image.Resampling.BILINEAR)
        rgb = np.asarray(image, dtype=np.float32) / 255.0
        gray = (0.299 * rgb[:, :, 0]) + (0.587 * rgb[:, :, 1]) + (0.114 * rgb[:, :, 2])
        if gray.shape[0] < 8 or gray.shape[1] < 8:
            raise ValueError("Image too small for culling metrics")

        center = gray[1:-1, 1:-1]
        laplacian = (
            -4.0 * center
            + gray[:-2, 1:-1]
            + gray[2:, 1:-1]
            + gray[1:-1, :-2]
            + gray[1:-1, 2:]
        )
        sharpness_raw = float(np.var(laplacian))
        sharpness = _safe_unit_interval(
            sharpness_raw / (sharpness_raw + CULLING_CONFIG["image_metrics"]["sharpness_denominator"])
        )

        luminance_mean = float(np.mean(gray))
        highlight_clip = float(np.mean(gray >= CULLING_CONFIG["image_metrics"]["highlight_threshold"]))
        shadow_clip = float(np.mean(gray <= CULLING_CONFIG["image_metrics"]["shadow_threshold"]))
        clipping_penalty = _safe_unit_interval(
            (highlight_clip * CULLING_CONFIG["image_metrics"]["highlight_clip_weight"])
            + (shadow_clip * CULLING_CONFIG["image_metrics"]["shadow_clip_weight"])
        )
        exposure_balance = _safe_unit_interval(
            1.0 - (
                abs(luminance_mean - CULLING_CONFIG["image_metrics"]["exposure_target"])
                / CULLING_CONFIG["image_metrics"]["exposure_tolerance"]
            )
        )
        exposure = _safe_unit_interval(
            (CULLING_CONFIG["image_metrics"]["exposure_balance_weight"] * exposure_balance)
            + (CULLING_CONFIG["image_metrics"]["exposure_clip_weight"] * (1.0 - clipping_penalty))
        )

        blurred = (
            gray[:-2, :-2] + gray[:-2, 1:-1] + gray[:-2, 2:]
            + gray[1:-1, :-2] + gray[1:-1, 1:-1] + gray[1:-1, 2:]
            + gray[2:, :-2] + gray[2:, 1:-1] + gray[2:, 2:]
        ) / 9.0
        reference = gray[1:-1, 1:-1]
        residual = np.abs(reference - blurred)
        midtone_mask = (reference > 0.15) & (reference < 0.85)
        if np.any(midtone_mask):
            noise_raw = float(np.mean(residual[midtone_mask]))
        else:
            noise_raw = float(np.mean(residual))
        noise_penalty = _safe_unit_interval(noise_raw / CULLING_CONFIG["image_metrics"]["noise_denominator"])

        technical_score = _safe_unit_interval(
            (CULLING_CONFIG["image_metrics"]["technical_weight_sharpness"] * sharpness)
            + (CULLING_CONFIG["image_metrics"]["technical_weight_exposure"] * exposure)
            + (CULLING_CONFIG["image_metrics"]["technical_weight_noise"] * (1.0 - noise_penalty))
        )

        contrast = _safe_unit_interval(float(np.std(gray)) / 0.25)
        rg = np.abs(rgb[:, :, 0] - rgb[:, :, 1])
        yb = np.abs(0.5 * (rgb[:, :, 0] + rgb[:, :, 1]) - rgb[:, :, 2])
        colorfulness = _safe_unit_interval(float(np.mean(np.sqrt((rg * rg) + (yb * yb)))) / 0.35)
        aesthetic_score = _safe_unit_interval(
            (CULLING_CONFIG["image_metrics"]["aesthetic_contrast_weight"] * contrast)
            + (CULLING_CONFIG["image_metrics"]["aesthetic_colorfulness_weight"] * colorfulness)
            + (CULLING_CONFIG["image_metrics"]["aesthetic_exposure_weight"] * exposure)
        )

        return {
            "cull_sharpness": round(sharpness, 4),
            "cull_exposure": round(exposure, 4),
            "cull_noise": round(noise_penalty, 4),
            "cull_highlight_clip": round(highlight_clip, 4),
            "cull_shadow_clip": round(shadow_clip, 4),
            "cull_technical_score": round(technical_score, 4),
            "cull_aesthetic": round(aesthetic_score, 4),
        }
    except Exception as exc:
        logger.warning("Could not compute culling metrics: %s", exc)
        return {
            "cull_sharpness": 0.0,
            "cull_exposure": 0.0,
            "cull_noise": 1.0,
            "cull_highlight_clip": 0.0,
            "cull_shadow_clip": 0.0,
            "cull_technical_score": 0.0,
            "cull_aesthetic": 0.0,
        }


def _aggregate_face_culling_metrics(face_results: list[dict]) -> dict:
    if not face_results:
        return {
            "cull_face_count": 0,
            "cull_face_sharpness": 0.0,
            "cull_face_prominence": 0.0,
            "cull_face_visibility": 0.0,
            "cull_face_score": 0.0,
            "cull_eye_openness": 0.0,
            "cull_blink_penalty": 1.0,
            "cull_occlusion": 0.0,
            "cull_faces_present": False,
        }

    face_count = len(face_results)
    sharpness_values = [_safe_unit_interval(face.get("sharpness", 0.0)) for face in face_results]
    prominence_values = [
        _safe_unit_interval(face.get("area_ratio", 0.0) / CULLING_CONFIG["face_metrics"]["prominence_normalizer"])
        for face in face_results
    ]
    visibility_values = [
        _safe_unit_interval(
            (CULLING_CONFIG["face_metrics"]["visibility_det_weight"] * _safe_unit_interval(face.get("det_score", 0.0)))
            + (CULLING_CONFIG["face_metrics"]["visibility_center_weight"] * _safe_unit_interval(face.get("center_proximity", 0.0)))
        )
        for face in face_results
    ]
    eye_openness_values = [_safe_unit_interval(face.get("eye_openness", 0.0)) for face in face_results]
    blink_penalties = [_safe_unit_interval(face.get("blink_penalty", 1.0)) for face in face_results]
    occlusion_values = []
    for face in face_results:
        if "occlusion" in face:
            occlusion_values.append(_safe_unit_interval(face.get("occlusion", 0.0)))
        else:
            occlusion_values.append(
                _safe_unit_interval(
                    1.0 - (
                        (CULLING_CONFIG["face_metrics"]["occlusion_det_weight"] * _safe_unit_interval(face.get("det_score", 0.0)))
                        + (CULLING_CONFIG["face_metrics"]["occlusion_center_weight"] * _safe_unit_interval(face.get("center_proximity", 0.0)))
                        + (CULLING_CONFIG["face_metrics"]["occlusion_eye_weight"] * _safe_unit_interval(face.get("eye_openness", 0.0)))
                    )
                )
            )

    face_sharpness = max(sharpness_values) if sharpness_values else 0.0
    face_prominence = max(prominence_values) if prominence_values else 0.0
    face_visibility = sum(visibility_values) / len(visibility_values) if visibility_values else 0.0
    eye_openness = max(eye_openness_values) if eye_openness_values else 0.0
    blink_penalty = min(blink_penalties) if blink_penalties else 1.0
    occlusion_penalty = min(occlusion_values) if occlusion_values else 0.0
    face_score_raw = (
        (CULLING_CONFIG["face_metrics"]["score_weight_sharpness"] * face_sharpness)
        + (CULLING_CONFIG["face_metrics"]["score_weight_prominence"] * face_prominence)
        + (CULLING_CONFIG["face_metrics"]["score_weight_visibility"] * face_visibility)
        + (CULLING_CONFIG["face_metrics"]["score_weight_eye_openness"] * eye_openness)
        + (CULLING_CONFIG["face_metrics"]["score_weight_occlusion"] * (1.0 - occlusion_penalty))
    )
    weight_total = (
        CULLING_CONFIG["face_metrics"]["score_weight_sharpness"]
        + CULLING_CONFIG["face_metrics"]["score_weight_prominence"]
        + CULLING_CONFIG["face_metrics"]["score_weight_visibility"]
        + CULLING_CONFIG["face_metrics"]["score_weight_eye_openness"]
        + CULLING_CONFIG["face_metrics"]["score_weight_occlusion"]
    )
    face_score = _safe_unit_interval(face_score_raw / max(1e-6, weight_total))

    return {
        "cull_face_count": face_count,
        "cull_face_sharpness": round(face_sharpness, 4),
        "cull_face_prominence": round(face_prominence, 4),
        "cull_face_visibility": round(face_visibility, 4),
        "cull_face_score": round(face_score, 4),
        "cull_eye_openness": round(eye_openness, 4),
        "cull_blink_penalty": round(blink_penalty, 4),
        "cull_occlusion": round(occlusion_penalty, 4),
        "cull_faces_present": True,
    }


def get_uuids_needing_processing(uuids: list[str], options: dict) -> list[str]:
    """
    Returns UUIDs that need processing based on selected tasks and existing backend data.
    Mirrors the same logic as process_image_task for determining what's missing.
    """
    regenerate_metadata = options.get('regenerate_metadata', True)
    compute_embeddings = options.get('compute_embeddings', True)
    compute_metadata = options.get('compute_metadata', False)
    compute_faces = options.get('compute_faces', False)
    compute_vertexai = options.get('compute_vertexai', False)
    any_processing_task_enabled = compute_embeddings or compute_metadata or compute_faces or compute_vertexai
    catalog_id = options.get('catalog_id')

    if not uuids:
        return []

    # Load existing records for all UUIDs (catalog-scoped when catalog_id provided)
    existing_records = {}
    for uuid in uuids:
        existing_record = chroma_service.get_image(uuid, catalog_id=catalog_id)
        if existing_record and existing_record['ids']:
            existing_records[uuid] = existing_record['metadatas'][0] if existing_record['metadatas'] else {}

    needing_processing = []
    for uuid in uuids:
        existing = existing_records.get(uuid, {})

        needs_embedding = compute_embeddings and (regenerate_metadata or not existing.get('has_embedding', False))
        has_any_metadata = existing.get('title') or existing.get('caption') or existing.get('alt_text') or existing.get('keywords')
        needs_metadata = compute_metadata and (regenerate_metadata or not has_any_metadata)
        needs_faces = compute_faces and (regenerate_metadata or not chroma_service.faces_checked_for_photo(uuid))
        needs_vertexai = compute_vertexai and (regenerate_metadata or not chroma_service.has_vertex_embedding(uuid))
        needs_cull_phash = any_processing_task_enabled and (regenerate_metadata or not existing.get('cull_phash'))

        if needs_embedding or needs_metadata or needs_faces or needs_vertexai or needs_cull_phash:
            needing_processing.append(uuid)

    return needing_processing


def get_photo_ids_needing_processing(photo_ids: list[str], options: dict) -> list[str]:
    """Preferred alias for get_uuids_needing_processing with generic photo IDs."""
    return get_uuids_needing_processing(photo_ids, options)


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
        compute_faces = options.get('compute_faces', False)
        compute_vertexai = options.get('compute_vertexai', False)

        logger.info(f"Starting batch processing of {total_images} images...")
        logger.info(f"regenerate_metadata={regenerate_metadata}, compute_embeddings={compute_embeddings}, "
                   f"compute_metadata={compute_metadata}, compute_faces={compute_faces}, compute_vertexai={compute_vertexai}")
        
        # Check existing records if regenerate_metadata is False
        catalog_id = options.get('catalog_id')
        existing_records = {}
        if not regenerate_metadata:
            logger.info("Checking existing records to determine what needs generation...")
            for _, uuid, _ in image_triplets:
                existing_record = chroma_service.get_image(uuid, catalog_id=catalog_id)
                if existing_record and existing_record['ids']:
                    existing_records[uuid] = existing_record['metadatas'][0] if existing_record['metadatas'] else {}
        
        # Determine what actually needs to be computed for each image
        images_needing_embeddings = []
        images_needing_metadata = []
        images_needing_faces = []
        images_needing_vertexai = []
        images_needing_cull_phash = []
        
        for _, uuid, _ in image_triplets:
            existing = existing_records.get(uuid, {})
            
            # Check if embedding is needed
            needs_embedding = compute_embeddings and (regenerate_metadata or not existing.get('has_embedding', False))
            if needs_embedding:
                images_needing_embeddings.append(uuid)

            # Check if Vertex AI embedding is needed
            if compute_vertexai and (regenerate_metadata or not chroma_service.has_vertex_embedding(uuid)):
                images_needing_vertexai.append(uuid)

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

            # cull_phash is part of culling foundation and should be backfilled in delta mode.
            if regenerate_metadata or not existing.get('cull_phash'):
                images_needing_cull_phash.append(uuid)
        
        logger.info(f"Generation needed: {len(images_needing_embeddings)} embeddings, "
                   f"{len(images_needing_metadata)} metadata, {len(images_needing_faces)} faces, {len(images_needing_vertexai)} vertexai")

        # If nothing needs to be generated and we're not regenerating, skip work.
        # When regenerate_metadata is True we must not early-return: new images (no entry yet)
        # still need to be added to Chroma with at least minimal metadata.
        # Also do NOT early-return when compute_faces or compute_vertexai is True - we need to process images.
        if (not regenerate_metadata
                and not compute_faces
                and not compute_vertexai
                and len(images_needing_embeddings) == 0
                and len(images_needing_metadata) == 0
                and len(images_needing_cull_phash) == 0):
            logger.info("No generation required (regenerate_metadata=False and all fields present). Returning success without changes.")
            return len(image_triplets), 0

    
        analysis_service = get_analysis_service()
        siglip_model = None
        siglip_processor = None

        siglip_model = server_lifecycle.get_model()
        siglip_processor = server_lifecycle.get_processor()

        # Convert lists to sets for faster lookup in analyze_batch
        embeddings, metadata_results = analysis_service.analyze_batch(
            image_triplets, options, siglip_model, siglip_processor,
            set(images_needing_embeddings), set(images_needing_metadata)
        )

        # Only fail batch when we actually needed embeddings but got none
        if embeddings is None and len(images_needing_embeddings) > 0:
            return 0, total_images

        # Vertex AI embeddings (optional, parallel to SigLIP2): generate for images_needing_vertexai
        vertex_project_id = options.get('vertex_project_id')
        vertex_location = options.get('vertex_location')
        vertex_embeddings_by_uuid = {}
        if images_needing_vertexai and vertexai_service.is_available(vertex_project_id, vertex_location):
            vertex_uuids = []
            vertex_bytes = []
            for image_bytes, uuid, _ in image_triplets:
                if uuid in set(images_needing_vertexai):
                    vertex_uuids.append(uuid)
                    vertex_bytes.append(image_bytes)
            if vertex_bytes:
                logger.info(f"Generating Vertex AI embeddings for {len(vertex_bytes)} images...")
                vertex_results = vertexai_service.get_image_embeddings(
                    vertex_bytes, vertex_project_id=vertex_project_id, vertex_location=vertex_location
                )
                for uid, emb in zip(vertex_uuids, vertex_results):
                    if emb is not None:
                        vertex_embeddings_by_uuid[uid] = emb

        for i, (image_bytes, uuid, filename) in enumerate(image_triplets):
            try:
                embedding = embeddings[i] if embeddings is not None else None
                metadata_data = metadata_results[i] if metadata_results else None
                
                existing = existing_records.get(uuid, {})
                
                need_embedding = uuid in images_needing_embeddings
                need_metadata = uuid in images_needing_metadata
                need_cull_phash = uuid in images_needing_cull_phash

                # Validate that required new data was generated if needed
                if need_embedding and embedding is None:
                    logger.error(f"Embedding generation failed for {uuid}. Skipping.")
                    failure_count += 1
                    continue

                if need_metadata and (not metadata_data or not metadata_data.success):
                    logger.error(f"Metadata generation failed for {uuid}. Skipping.")
                    failure_count += 1
                    continue

                # If nothing needed for this UUID (already complete) and no face processing, skip
                # When compute_faces is True we must not skip - we need to reach face detection
                if (not need_embedding and not need_metadata
                        and not need_cull_phash
                        and not regenerate_metadata and not compute_faces):
                    logger.info(f"UUID {uuid}: already fully indexed; skipping update.")
                    success_count += 1
                    continue

                # Start with existing metadata if not regenerating
                if not regenerate_metadata and existing:
                    main_metadata = existing.copy()
                    # Update only basic fields that should always be current
                    main_metadata["filename"] = filename
                    main_metadata["photo_id"] = uuid
                    main_metadata["uuid"] = existing.get("uuid", uuid)
                else:
                    main_metadata = {
                        "filename": filename,
                        "photo_id": uuid,
                        "uuid": uuid,
                        "provider": provider,
                        "model": model_name,
                    }

                # Prefer explicit capture_time from Lightroom catalog (if provided).
                # `date_time_unix` is a seconds-since-epoch float, `date_time` is
                # an ISO/W3C string kept for backwards compatibility.
                capture_time = None
                catalog_time_unix = options.get('date_time_unix')
                if catalog_time_unix is not None:
                    try:
                        capture_time = float(catalog_time_unix)
                    except (TypeError, ValueError):
                        logger.warning("Invalid date_time_unix value for %s: %r", uuid, catalog_time_unix)
                elif options.get('date_time'):
                    from datetime import datetime, timezone
                    dt_str = options['date_time']
                    try:
                        # Normalize common W3C/ISO forms (e.g. trailing 'Z').
                        normalized = str(dt_str).strip()
                        if normalized.endswith("Z"):
                            normalized = normalized[:-1] + "+00:00"
                        dt_obj = datetime.fromisoformat(normalized)
                        if dt_obj.tzinfo is None:
                            dt_obj = dt_obj.replace(tzinfo=timezone.utc)
                        capture_time = float(dt_obj.timestamp())
                    except Exception as e:
                        logger.warning("Could not parse date_time for %s: %r (%s)", uuid, dt_str, e)

                if capture_time is not None:
                    main_metadata["capture_time"] = capture_time

                # Technical culling metrics are cheap enough to compute on every pass.
                main_metadata.update(_compute_culling_metrics(image_bytes))
                phash_hex = _compute_perceptual_hash(image_bytes)
                if phash_hex:
                    main_metadata["cull_phash"] = phash_hex
                    main_metadata["phash"] = phash_hex

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
                    chroma_service.update_image(uuid, main_metadata, embedding=update_embedding, catalog_id=catalog_id)
                elif regenerate_metadata:
                    logger.info(f"UUID {uuid} set to regenerate. Updating (embedding: {update_embedding is not None}).")
                    existing_in_chroma = chroma_service.get_image(uuid)
                    if existing_in_chroma and existing_in_chroma.get("ids"):
                        chroma_service.update_image(uuid, main_metadata, embedding=update_embedding, catalog_id=catalog_id)
                    else:
                        chroma_service.add_image(uuid, embedding, main_metadata, catalog_id=catalog_id)
                else:
                    # New record
                    if embedding is not None:
                        logger.info(f"UUID {uuid} is new. Indexing with embeddings.")
                    else:
                        logger.info(f"UUID {uuid} is new. Indexing metadata-only entry (no embedding).")
                    chroma_service.add_image(uuid, embedding, main_metadata, catalog_id=catalog_id)

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
                                embeddings_f = [face["embedding"] for face in face_results]
                                thumbnails_b64 = [face.get("thumbnail", "") for face in face_results]
                                face_extra_metadatas = [
                                    {
                                        "bbox": json.dumps(face.get("bbox") or []),
                                        "face_area_ratio": face.get("area_ratio", 0.0),
                                        "face_sharpness": face.get("sharpness", 0.0),
                                        "face_det_score": face.get("det_score", 0.0),
                                        "face_center_proximity": face.get("center_proximity", 0.0),
                                        "face_eye_openness": face.get("eye_openness", 0.0),
                                        "face_blink_penalty": face.get("blink_penalty", 1.0),
                                        "face_occlusion": face.get("occlusion", 0.0),
                                    }
                                    for face in face_results
                                ]
                                chroma_service.add_faces_batch(
                                    face_ids, embeddings_f, [uuid] * len(face_results), thumbnails_b64,
                                    extra_metadatas=face_extra_metadatas
                                )
                                main_metadata.update(_aggregate_face_culling_metrics(face_results))
                                chroma_service.update_image(uuid, main_metadata, embedding=update_embedding, catalog_id=catalog_id)
                                logger.info(f"UUID {uuid}: indexed {len(face_results)} face(s).")
                            else:
                                main_metadata.update(_aggregate_face_culling_metrics([]))
                                chroma_service.update_image(uuid, main_metadata, embedding=update_embedding, catalog_id=catalog_id)
                                chroma_service.set_faces_checked(uuid)
                                logger.debug(f"UUID {uuid}: no faces detected (marked as checked).")
                        except Exception as e:
                            logger.warning(f"Face detection/indexing failed for {uuid}: {e}", exc_info=True)

                # Vertex AI embeddings (optional, separate Chroma collection)
                if uuid in vertex_embeddings_by_uuid:
                    chroma_service.add_vertex_image(uuid, vertex_embeddings_by_uuid[uuid], {"photo_id": uuid, "uuid": uuid})
                    logger.debug(f"UUID {uuid}: Vertex AI embedding stored.")

                success_count += 1

            except Exception as e:
                logger.error(f"Error processing image {uuid}: {str(e)}", exc_info=True)
                failure_count += 1

        return success_count, failure_count
    except Exception as e:
        logger.error(f"Error during batch processing task: {str(e)}", exc_info=True)
        return 0, total_images
