from flask import Blueprint, request, jsonify
import time
from collections import deque
import os

import service_chroma as chroma_service
from config import logger
from service_index import process_image_task, get_uuids_needing_processing
import service_face as face_service
import service_persons as persons_service
import base64
import json

index_bp = Blueprint('index', __name__)

# Store timestamps of the last 100 requests to calculate processing speed
request_timestamps = deque(maxlen=100)

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
    options['existing_keywords'] = data.get('existing_keywords')
    options['gps_coordinates'] = data.get('gps_coordinates')
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

    options['replace_ss'] = str(data.get('replace_ss', 'false')).lower() == 'true'
    options['ollama_base_url'] = data.get('ollama_base_url') or None  # Optional: use custom Ollama host
    # Support both snake_case and camelCase keys from clients
    reg_val = data.get('regenerate_metadata')
    if reg_val is None:
        reg_val = data.get('regenerateMetadata', 'true')
    options['regenerate_metadata'] = str(reg_val).lower() == 'true'
    options['prompt'] = data.get('prompt')
    options['date_time'] = data.get('date_time')

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
        tasks = ['embeddings', 'quality'] # Default tasks

    options['compute_embeddings'] = 'embeddings' in tasks
    options['compute_metadata'] = 'metadata' in tasks
    options['compute_quality'] = 'quality' in tasks
    options['compute_faces'] = 'faces' in tasks

    return options

@index_bp.route('/index', methods=['POST'])
def index_images_batch():
    """
    Receives a batch of images, processes them synchronously, and indexes them.
    Returns a 200 OK status once all images are processed.
    """
    logger.info("Index request received")
    images = request.files.getlist('image')
    uuids = request.form.getlist('uuid')
    
    options = _extract_options(request.form)

    if not images or not uuids or len(images) != len(uuids):
        return jsonify({"error": "Mismatch between number of images and UUIDs, or no images provided"}), 400

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
        uuid = uuids[i]

        if not file or not uuid:
            logger.warning(f"Skipping an entry in the batch due to missing file or uuid.")
            continue

        image_triplets.append((file.read(), uuid, file.filename))

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
    uuid = data.get('uuid')
    filename = data.get('filename')

    if not image or not uuid or not filename:
        logger.info(f"{image}, {uuid}, {filename}")
        return jsonify({"error": "Missing required fields: image, uuid, filename"}), 400

    options = _extract_options(data)

    success_count, failure_count = process_image_task(
        [(base64.b64decode(image.encode('ascii')), uuid, filename)],
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

    # Use a list comprehension to extract the paths and UUIDs.
    paths = [item.get('path') for item in images_data]
    uuids = [item.get('uuid') for item in images_data]

    # Check for missing keys or mismatched lengths (robustness).
    if not all(paths) or not all(uuids) or len(paths) != len(uuids):
        return jsonify({"error": "Mismatch in data, or missing 'path' or 'uuid' keys in some objects"}), 400

    batch_size = len(paths)

    image_triplets = []
    failed_paths = []
    for i in range(batch_size):
        path = paths[i]
        uuid = uuids[i]

        if not path or not uuid:
            logger.warning(f"Skipping an entry in the batch due to missing file or uuid.")
            continue

        try:
            with open(path, 'rb') as file:
                image_data = file.read()

            filename = os.path.basename(path)
            image_triplets.append((image_data, uuid, filename))
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
    if 'uuid' not in request.json:
        return jsonify({"error": "No uuid provided"}), 400
    uuid = request.json.get('uuid')
    
    try:
        chroma_service.delete_image(uuid)
        chroma_service.delete_faces_by_photo_uuid(uuid)
        logger.info(f"Image ID {uuid} removed from ChromaDB (including face embeddings).")
        return jsonify({"status": "removed", "uuid": uuid})
    except Exception as e:
        logger.error(f"Error removing image {uuid}: {e}")
        return jsonify({"error": "UUID not found or error during removal"}), 404
        

@index_bp.route('/get', methods=['POST'])
def get_photo_data():
    """
    Retrieves metadata and quality scores for a photo by UUID.
    
    JSON body parameters:
    - uuid (string): The UUID of the photo to retrieve
    
    Returns:
    - status: "success" or "error"
    - uuid: The photo's UUID
    - metadata: Dictionary with all metadata fields (title, caption, keywords, etc.)
    - quality: Dictionary with quality scores (overall_score, composition_score, etc.)
    """
    logger.info("Get photo data request received")
    
    if 'uuid' not in request.json:
        return jsonify({"status": "error", "error": "No uuid provided"}), 400
    
    uuid = request.json.get('uuid')
    
    try:
        # Get photo data from ChromaDB
        photo_data = chroma_service.get_image(uuid)
        logger.debug(f"Retrieved photo data for UUID {uuid}: {photo_data}")
        
        if not photo_data or not photo_data['ids']:
            logger.warning(f"Photo with UUID {uuid} not found in database")
            return jsonify({"status": "error", "error": "Photo not found"}), 404
        
        # Extract metadata
        metadata_dict = photo_data['metadatas'][0] if photo_data['metadatas'] else {}
        
        # Separate metadata into user-facing metadata and quality scores
        metadata_fields = {}
        quality_fields = {}
        
        # Quality score field names
        quality_keys = {
            'overall_score', 'composition_score', 'lighting_score', 
            'motiv_score', 'colors_score', 'emotion_score', 'quality_critique'
        }
        
        # User metadata field names (from metadata generation)
        metadata_keys = {
            'title', 'caption', 'keywords', 'alt_text'
        }
        
        ai_model = metadata_dict.get('model')
        ai_rundate = metadata_dict.get('run_date')

        for key, value in metadata_dict.items():
            if key in quality_keys:
                quality_fields[key] = value
            elif key in metadata_keys:
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
        
        logger.info(f"Retrieved data for photo {uuid}: {len(metadata_fields)} metadata fields, {len(quality_fields)} quality fields")
        
        return jsonify({
            "status": "success",
            "uuid": uuid,
            "metadata": metadata_fields,
            "quality": quality_fields,
            "ai_model": ai_model,
            "ai_rundate": ai_rundate
        })
        
    except Exception as e:
        logger.error(f"Error retrieving photo data for {uuid}: {e}", exc_info=True)
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
    
    ids_data = chroma_service.get_all_image_ids(has_embedding=has_embedding)
    logger.info(f"Returning {len(ids_data)} image IDs")
    return jsonify(ids_data)


@index_bp.route('/index/check-unprocessed', methods=['POST'])
def check_unprocessed():
    """
    Returns UUIDs that need processing based on selected tasks and existing backend data.
    Used by the Lightroom plugin for "New or unprocessed photos" scope.
    """
    data = request.get_json() or {}
    uuids = data.get('uuids', [])
    if not uuids:
        return jsonify({"uuids": []}), 200

    options = _extract_options(data)
    needing = get_uuids_needing_processing(uuids, options)
    logger.info(f"check-unprocessed: {len(needing)} of {len(uuids)} photos need processing")
    return jsonify({"uuids": needing}), 200

