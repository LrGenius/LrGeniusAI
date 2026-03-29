from flask import Blueprint, request, jsonify
import time
from collections import deque
import os

import service_chroma as chroma_service
from config import logger
from service_index import process_image_task, get_photo_ids_needing_processing
import service_face as face_service
import service_persons as persons_service
import base64
import json

index_bp = Blueprint('index', __name__)

# Store timestamps of the last 100 requests to calculate processing speed
request_timestamps = deque(maxlen=100)


def _parse_json_field(value, default=None):
    """Parse JSON-encoded form fields when clients submit multipart data."""
    if value is None:
        return default
    if isinstance(value, str):
        try:
            return json.loads(value)
        except json.JSONDecodeError:
            return default
    return value

def _extract_options(data):
    """Extracts options from request data (form or json)."""
    options = {}
    try:
        from config import logger as _tmp_logger
        _tmp_logger.info(f"Raw indexing option keys received: {list(getattr(data, 'keys', lambda: [])())}")
    except Exception:
        pass
    options['provider'] = data.get('provider')
    options['model'] = data.get('model')
    options['api_key'] = data.get('api_key')
    options['language'] = data.get('language', 'German')
    options['temperature'] = float(data.get('temperature', 0.2))
    options['max_tokens'] = data.get('max_tokens')
    options['generate_keywords'] = str(data.get('generate_keywords', 'true')).lower() == 'true'
    options['generate_caption'] = str(data.get('generate_caption', 'true')).lower() == 'true'
    options['generate_title'] = str(data.get('generate_title', 'true')).lower() == 'true'
    options['generate_alt_text'] = str(data.get('generate_alt_text', 'true')).lower() == 'true'
    options['submit_gps'] = str(data.get('submit_gps', 'false')).lower() == 'true'
    options['submit_keywords'] = str(data.get('submit_keywords', 'false')).lower() == 'true'
    options['submit_folder_names'] = str(data.get('submit_folder_names', 'false')).lower() == 'true'
    raw_existing = _parse_json_field(data.get('existing_keywords'))
    # Lightroom may send keywordTagsForExport as a comma-separated string; normalize to list
    # so ", ".join(existing_keywords) in the prompt does not iterate over characters (issue #45).
    if raw_existing is None:
        options['existing_keywords'] = None
    elif isinstance(raw_existing, str):
        options['existing_keywords'] = [k.strip() for k in raw_existing.split(',') if k.strip()]
    elif isinstance(raw_existing, list):
        options['existing_keywords'] = [str(k).strip() for k in raw_existing if str(k).strip()]
    else:
        options['existing_keywords'] = None
    options['gps_coordinates'] = _parse_json_field(data.get('gps_coordinates'))
    options['folder_names'] = data.get('folder_names')
    options['user_context'] = data.get('user_context')
    
    keyword_categories_raw = data.get('keyword_categories', '[]')
    if isinstance(keyword_categories_raw, str):
        try:
            options['keyword_categories'] = json.loads(keyword_categories_raw)
        except json.JSONDecodeError:
            options['keyword_categories'] = []
    else:
        options['keyword_categories'] = keyword_categories_raw

    options['bilingual_keywords'] = str(data.get('bilingual_keywords', 'false')).lower() == 'true'
    options['keyword_secondary_language'] = data.get('keyword_secondary_language') or None

    options['replace_ss'] = str(data.get('replace_ss', 'false')).lower() == 'true'
    options['ollama_base_url'] = data.get('ollama_base_url') or None  # Optional: use custom Ollama host
    options['lmstudio_base_url'] = data.get('lmstudio_base_url') or None  # Optional: use custom LM Studio host
    # Support both snake_case and camelCase keys from clients
    reg_val = data.get('regenerate_metadata')
    if reg_val is None:
        reg_val = data.get('regenerateMetadata', 'true')
    options['regenerate_metadata'] = str(reg_val).lower() == 'true'
    options['prompt'] = data.get('prompt')
    # Optional capture time from Lightroom catalog.
    # `date_time_unix` is a float seconds-since-epoch value; `date_time` is an
    # ISO/W3C string kept for backwards compatibility.
    options['date_time'] = data.get('date_time')
    options['date_time_unix'] = data.get('date_time_unix')

    tasks_raw = data.get('tasks')
    if tasks_raw:
        if isinstance(tasks_raw, str):
            try:
                tasks = json.loads(tasks_raw) if tasks_raw.startswith('[') else [t.strip() for t in tasks_raw.split(',')]
            except (json.JSONDecodeError, AttributeError):
                tasks = [t.strip() for t in tasks_raw.split(',')]
        else:
            tasks = tasks_raw
    else:
        tasks = ['embeddings'] # Default tasks

    options['compute_embeddings'] = 'embeddings' in tasks
    options['compute_metadata'] = 'metadata' in tasks
    options['compute_faces'] = 'faces' in tasks
    options['compute_vertexai'] = 'vertexai' in tasks

    # Vertex AI config (from Lightroom plugin manager)
    options['vertex_project_id'] = data.get('vertex_project_id') or data.get('vertexProjectId')
    options['vertex_location'] = data.get('vertex_location') or data.get('vertexLocation')

    # Cross-catalog: optional catalog_id for soft-state and filtered reads
    options['catalog_id'] = data.get('catalog_id') or None
    if options['catalog_id'] and isinstance(options['catalog_id'], str):
        options['catalog_id'] = options['catalog_id'].strip() or None

    return options


def _extract_photo_ids(form_or_json):
    """Accept new photo_id(s) and legacy uuid(s)."""
    if hasattr(form_or_json, "getlist"):
        photo_ids = form_or_json.getlist("photo_id")
        if photo_ids:
            return photo_ids
        return form_or_json.getlist("uuid")
    photo_id = form_or_json.get("photo_id")
    if photo_id:
        return [photo_id]
    uuid = form_or_json.get("uuid")
    return [uuid] if uuid else []

@index_bp.route('/index', methods=['POST'])
def index_images_batch():
    """
    Receives a batch of images, processes them synchronously, and indexes them.
    Returns a 200 OK status once all images are processed.
    """
    logger.info("Index request received")
    images = request.files.getlist('image')
    photo_ids = _extract_photo_ids(request.form)
    
    options = _extract_options(request.form)

    if not images or not photo_ids or len(images) != len(photo_ids):
        return jsonify({"error": "Mismatch between number of images and photo IDs, or no images provided"}), 400

    batch_size = len(images)
    current_time = time.time()
    for _ in range(batch_size):
        request_timestamps.append(current_time)

    if len(request_timestamps) > 10:
        time_span = request_timestamps[-1] - request_timestamps[0]
        if time_span > 1:
            images_per_second = len(request_timestamps) / time_span
            logger.info(f"Indexing at {images_per_second:.2f} images/sec")

    image_triplets = []
    for i in range(batch_size):
        file = images[i]
        photo_id = photo_ids[i]

        if not file or not photo_id:
            logger.warning("Skipping an entry in the batch due to missing file or photo_id.")
            continue

        image_triplets.append((file.read(), photo_id, file.filename))

    if not image_triplets:
        logger.info("No valid images to process in the batch.")
        return jsonify({"status": "processed", "success_count": 0, "failure_count": batch_size}), 200

    success_count, failure_count = process_image_task(
        image_triplets,
        options=options
    )
    
    logger.info(f"Batch processing complete. Success: {success_count}, Failures: {failure_count}.")
    
    if success_count == 0:
        logger.warning("No images were successfully processed in the batch.")
        return jsonify({"error": "No images were successfully processed"}), 500
    
    return jsonify({"status": "processed", "success_count": success_count, "failure_count": failure_count}), 200

@index_bp.route('/index_base64', methods=['POST'])
def index_images_batch_base64():
    """
    Receives a single image base64 encoded, processes it, and indexes it.
    Returns a 200 OK status once processed.
    """
    logger.info("Index base64 request received")
    data = request.get_json()

    if not data:
        return jsonify({"error": "No JSON payload provided"}), 400
    
    # Extract required fields
    image = data.get('image')
    photo_id = data.get('photo_id') or data.get('uuid')
    filename = data.get('filename')

    if not image or not photo_id or not filename:
        logger.info(f"{image}, {photo_id}, {filename}")
        return jsonify({"error": "Missing required fields: image, photo_id, filename"}), 400

    options = _extract_options(data)

    success_count, failure_count = process_image_task(
        [(base64.b64decode(image.encode('ascii')), photo_id, filename)],
        options=options
    )
    
    logger.info(f"Batch processing complete. Success: {success_count}, Failures: {failure_count}.")

    if success_count == 0:
        logger.warning("No images were successfully processed in the batch.")
        return jsonify({"error": "No images were successfully processed"}), 500
        
    return jsonify({"status": "processed", "success_count": success_count, "failure_count": failure_count}), 200


@index_bp.route('/index_by_reference', methods=['POST'])
def index_images_batch_by_reference():
    """
    Receives a batch of image references in a JSON payload, processes them, 
    and indexes them.
    """
    logger.info("Index by reference request received")
    data = request.get_json()

    if not data:
        return jsonify({"error": "No JSON payload provided"}), 400
    
    logger.debug(f"Index by reference payload: {data}")

    options = _extract_options(data)
    
    # Extract image list
    images_data = data.get('images', [])

    # Use a list comprehension to extract paths and photo IDs.
    paths = [item.get('path') for item in images_data]
    photo_ids = [item.get('photo_id') or item.get('uuid') for item in images_data]

    # Check for missing keys or mismatched lengths (robustness).
    if not all(paths) or not all(photo_ids) or len(paths) != len(photo_ids):
        return jsonify({"error": "Mismatch in data, or missing 'path' or 'photo_id' keys in some objects"}), 400

    batch_size = len(paths)

    image_triplets = []
    failed_paths = []
    for i in range(batch_size):
        path = paths[i]
        photo_id = photo_ids[i]

        if not path or not photo_id:
            logger.warning("Skipping an entry in the batch due to missing file or photo_id.")
            continue

        try:
            with open(path, 'rb') as file:
                image_data = file.read()

            filename = os.path.basename(path)
            image_triplets.append((image_data, photo_id, filename))
        except FileNotFoundError:
            logger.warning(f"File not found at path: {path}. Skipping.")
            failed_paths.append(path)
        except Exception as e:
            logger.error(f"Error processing file at path {path}: {e}")
            failed_paths.append(path)

    read_failures = len(failed_paths)
    if not image_triplets:
        logger.info("No valid image paths to process in the batch.")
        return jsonify({"status": "processed", "success_count": 0, "failure_count": read_failures}), 200

    success_count, processing_failures = process_image_task(
        image_triplets,
        options=options
    )
    total_failures = read_failures + processing_failures
    
    logger.info(f"Batch processing by reference complete. Success: {success_count}, Failures: {total_failures} ({read_failures} read failures, {processing_failures} processing failures).")

    if success_count == 0:
        logger.warning("No images were successfully processed in the batch.")
        return jsonify({"error": "No images were successfully processed"}), 500
    
    return jsonify({"status": "processed", "success_count": success_count, "failure_count": total_failures}), 200


@index_bp.route('/remove', methods=['POST'])
def remove_image():
    logger.info("Remove request received")
    body = request.json or {}
    photo_id = body.get('photo_id') or body.get('uuid')
    if not photo_id:
        return jsonify({"error": "No photo_id provided"}), 400
    
    try:
        chroma_service.delete_image(photo_id)
        chroma_service.delete_faces_by_photo_uuid(photo_id)
        logger.info(f"Image ID {photo_id} removed from ChromaDB (including face embeddings).")
        return jsonify({"status": "removed", "photo_id": photo_id, "uuid": photo_id})
    except Exception as e:
        logger.error(f"Error removing image {photo_id}: {e}")
        return jsonify({"error": "photo_id not found or error during removal"}), 404


@index_bp.route('/remove/metadata', methods=['POST'])
def remove_metadata():
    """
    Clear only AI-generated metadata (title, caption, keywords, alt_text, etc.) for a photo.
    Keeps the document and embeddings so the photo remains in the index and searchable.
    Use when the user discards a suggestion (e.g. in the review dialog) so they can regenerate later.
    """
    logger.info("Remove metadata request received")
    body = request.json or {}
    photo_id = body.get('photo_id') or body.get('uuid')
    if not photo_id:
        return jsonify({"error": "No photo_id provided"}), 400
    try:
        cleared = chroma_service.clear_image_metadata(photo_id)
        if not cleared:
            return jsonify({"error": "photo_id not found"}), 404
        logger.info(f"Metadata cleared for photo_id {photo_id} (embeddings kept).")
        return jsonify({"status": "ok", "photo_id": photo_id, "uuid": photo_id})
    except Exception as e:
        logger.error(f"Error clearing metadata for {photo_id}: {e}", exc_info=True)
        return jsonify({"error": "photo_id not found or error during metadata clear"}), 404


@index_bp.route('/get', methods=['POST'])
def get_photo_data():
    """
    Retrieves stored metadata for a photo by photo_id.
    
    JSON body parameters:
    - photo_id (string): The ID of the photo to retrieve
    
    Returns:
    - status: "success" or "error"
    - photo_id: The photo ID
    - metadata: Dictionary with all metadata fields (title, caption, keywords, etc.)
    """
    logger.info("Get photo data request received")
    
    body = request.json or {}
    photo_id = body.get('photo_id') or body.get('uuid')
    if not photo_id:
        return jsonify({"status": "error", "error": "No photo_id provided"}), 400
    
    catalog_id = body.get('catalog_id')
    try:
        # Get photo data from ChromaDB (catalog-scoped when catalog_id provided)
        photo_data = chroma_service.get_image(photo_id, catalog_id=catalog_id)
        logger.debug(f"Retrieved photo data for photo_id {photo_id}: {photo_data}")
        
        if not photo_data or not photo_data['ids']:
            logger.warning(f"Photo with photo_id {photo_id} not found in database")
            return jsonify({"status": "error", "error": "Photo not found"}), 404
        
        # Extract metadata
        metadata_dict = photo_data['metadatas'][0] if photo_data['metadatas'] else {}
        
        # Separate user-facing metadata from internal indexing fields
        metadata_fields = {}
        
        # User metadata field names (from metadata generation)
        metadata_keys = {
            'title', 'caption', 'keywords', 'alt_text'
        }
        
        ai_model = metadata_dict.get('model')
        ai_rundate = metadata_dict.get('run_date')

        for key, value in metadata_dict.items():
            if key in metadata_keys:
                logger.info(f"Processing metadata field {key}: {value}")
                # Keywords must be returned as JSON string (not parsed) for plugin to handle
                if key == 'keywords' and isinstance(value, str) and value:
                    # Keep keywords as JSON string for plugin to parse
                    # The plugin expects either:
                    # - JSON array: ["kw1", "kw2"]
                    # - JSON object: {"Category": ["kw1"], ...}
                    metadata_fields[key] = json.loads(value)
                elif key == 'tokens_used' and isinstance(value, str) and value:
                    try:
                        metadata_fields[key] = json.loads(value) if value else []
                    except (json.JSONDecodeError, ValueError):
                        logger.warning(f"Error decoding JSON for {key}: {value}")
                        metadata_fields[key] = []
                else:
                    metadata_fields[key] = value
        
        logger.info(f"Retrieved data for photo {photo_id}: {len(metadata_fields)} metadata fields")
        
        return jsonify({
            "status": "success",
            "photo_id": photo_id,
            "uuid": photo_id,
            "metadata": metadata_fields,
            "ai_model": ai_model,
            "ai_rundate": ai_rundate
        })
        
    except Exception as e:
        logger.error(f"Error retrieving photo data for {photo_id}: {e}", exc_info=True)
        return jsonify({"status": "error", "error": str(e)}), 500


@index_bp.route('/get/ids', methods=['GET'])
def get_ids():
    """Get all indexed image IDs, optionally filtered by embedding status.
    
    Query parameters:
        has_embedding (string): 'true' to get only images with real embeddings,
                               'false' to get only images with dummy embeddings,
                               omit to get all images.
    """
    logger.info("Get IDs request received")
    
    # Parse has_embedding parameter
    has_embedding_param = request.args.get('has_embedding')
    has_embedding = None
    if has_embedding_param is not None:
        has_embedding = has_embedding_param.lower() == 'true'
        logger.info(f"Filtering IDs by has_embedding={has_embedding}")

    catalog_id = request.args.get('catalog_id')
    ids_data = chroma_service.get_all_image_ids(has_embedding=has_embedding, catalog_id=catalog_id)
    logger.info(f"Returning {len(ids_data)} image IDs")
    return jsonify(ids_data)


@index_bp.route('/index/check-unprocessed', methods=['POST'])
def check_unprocessed():
    """
    Returns UUIDs that need processing based on selected tasks and existing backend data.
    Used by the Lightroom plugin for "New or unprocessed photos" scope.
    """
    data = request.get_json() or {}
    photo_ids = data.get('photo_ids') or data.get('uuids', [])
    if not photo_ids:
        return jsonify({"photo_ids": [], "uuids": []}), 200

    options = _extract_options(data)
    needing = get_photo_ids_needing_processing(photo_ids, options)
    logger.info(f"check-unprocessed: {len(needing)} of {len(photo_ids)} photos need processing")
    return jsonify({"photo_ids": needing, "uuids": needing}), 200


@index_bp.route('/sync/cleanup', methods=['POST'])
def sync_cleanup():
    """
    Disassociate the given catalog_id from photos that are no longer in the provided photo_ids list.
    Does not delete any documents; only updates catalog_ids metadata (soft state).
    Body: { "catalog_id": "...", "photo_ids": ["id1", "id2", ...] }
    """
    data = request.get_json() or {}
    catalog_id = data.get('catalog_id')
    photo_ids = data.get('photo_ids')
    if not catalog_id:
        return jsonify({"error": "catalog_id is required"}), 400
    if photo_ids is not None and not isinstance(photo_ids, list):
        return jsonify({"error": "photo_ids must be a list or omit for empty"}), 400
    try:
        result = chroma_service.sync_cleanup(catalog_id, photo_ids or [])
        return jsonify({"status": "ok", **result}), 200
    except Exception as e:
        logger.error(f"Sync cleanup failed: {e}", exc_info=True)
        return jsonify({"error": str(e)}), 500


@index_bp.route('/sync/claim', methods=['POST'])
def sync_claim():
    """
    Claim backend photos for this catalog: add catalog_id to each photo's catalog_ids.
    Body: { "catalog_id": "...", "photo_ids": ["id1", "id2", ...] }
    Use for migration so existing (unclaimed) photos become visible to this catalog.
    """
    data = request.get_json() or {}
    catalog_id = data.get("catalog_id")
    photo_ids = data.get("photo_ids")
    logger.info("sync/claim request: catalog_id=%s, photo_ids count=%s", catalog_id, len(photo_ids) if isinstance(photo_ids, list) else "n/a")
    if not catalog_id:
        return jsonify({"error": "catalog_id is required"}), 400
    if not isinstance(photo_ids, list):
        return jsonify({"error": "photo_ids must be a list"}), 400
    try:
        result = chroma_service.sync_claim(catalog_id, photo_ids)
        return jsonify({"status": "ok", **result}), 200
    except Exception as e:
        logger.error(f"Sync claim failed: {e}", exc_info=True)
        return jsonify({"error": str(e)}), 500

