from flask import Blueprint, request, jsonify
from config import logger
import service_chroma as chroma_service
import service_face as face_service
import service_persons as persons_service
import base64


faces_bp = Blueprint('faces', __name__)


@faces_bp.route('/faces/detect', methods=['POST'])
def detect_faces_in_image():
    """
    Detect all faces in an image and return thumbnails for selection.
    Body: JSON with "image" (base64).
    Returns: { status, faces: [ { thumbnail, index }, ... ] } (index 0-based).
    """
    logger.info("Faces detect request received")
    data = request.get_json()
    if not data or not data.get("image"):
        return jsonify({"error": "Missing 'image' (base64) in JSON body"}), 400
    try:
        raw = base64.b64decode(data["image"].encode("ascii"))
    except Exception as e:
        return jsonify({"error": f"Invalid base64 image: {e}"}), 400
    try:
        faces = face_service.detect_faces(raw)
    except Exception as e:
        logger.error(f"Face detection failed: {e}", exc_info=True)
        return jsonify({"error": str(e)}), 500
    if not faces:
        return jsonify({"status": "ok", "faces": []}), 200
    result_faces = [
        {"thumbnail": thumb, "index": i}
        for i, (_, thumb) in enumerate(faces)
    ]
    return jsonify({"status": "ok", "faces": result_faces}), 200


@faces_bp.route('/faces/query', methods=['POST'])
def query_faces_by_image():
    """
    Find indexed faces similar to the face(s) in the given image.
    Body: JSON with "image" (base64), optional "face_index" (default 0), optional "n_results" (default 10).
    Returns: For the selected face: list of { face_id, photo_uuid, thumbnail, person_id, distance }.
    """
    logger.info("Faces query request received")
    data = request.get_json()
    if not data or not data.get("image"):
        return jsonify({"error": "Missing 'image' (base64) in JSON body"}), 400
    n_results = int(data.get("n_results", 10))
    face_index = int(data.get("face_index", 0))
    try:
        raw = base64.b64decode(data["image"].encode("ascii"))
    except Exception as e:
        return jsonify({"error": f"Invalid base64 image: {e}"}), 400
    try:
        faces = face_service.detect_faces(raw)
    except Exception as e:
        logger.error(f"Face detection failed: {e}", exc_info=True)
        return jsonify({"error": str(e)}), 500
    if not faces:
        return jsonify({"status": "no_face", "results": []}), 200
    if face_index < 0 or face_index >= len(faces):
        return jsonify({"error": f"face_index must be 0..{len(faces) - 1}"}), 400
    embedding = faces[face_index][0]
    result = chroma_service.query_faces(embedding, n_results=n_results)
    ids = result.get("ids", [[]])[0]
    distances = result.get("distances", [[]])[0]
    metadatas = result.get("metadatas", [[]])[0]
    results = [
        {"face_id": fid, "photo_uuid": m.get("photo_uuid"), "thumbnail": m.get("thumbnail", ""), "person_id": m.get("person_id", ""), "distance": d}
        for fid, m, d in zip(ids, metadatas or [], distances or [])
    ]
    return jsonify({"status": "ok", "results": results}), 200


# --- Person grouping (cluster + name) ---

@faces_bp.route('/faces/cluster', methods=['POST'])
def cluster_faces():
    """
    Run clustering on all face embeddings and assign person_id to each face.
    Uses cosine distance (same scale as Immich "Maximum recognition distance").

    Body: optional {
      "distance_threshold": 0.5,   // cosine distance; default 0.5. Use 0.45 if over-merge; 0.55-0.65 if same person split.
      "min_faces_per_person": 3,   // only form person if >= N faces; singletons -> unassigned
      "linkage": "complete"        // "complete" (default) = tighter clusters; "average" = more merging
    }
    """
    logger.info("Faces cluster request received")
    data = request.get_json(silent=True) or {}
    threshold = float(data.get("distance_threshold", 0.5))
    min_faces = data.get("min_faces_per_person", "3")
    if min_faces is not None:
        min_faces = int(min_faces)
    linkage = (data.get("linkage") or "complete").strip().lower()
    if linkage not in ("complete", "average"):
        linkage = "complete"
    try:
        summary = persons_service.run_clustering(
            distance_threshold=threshold,
            min_faces_per_person=min_faces,
            linkage=linkage,
        )
        return jsonify({"status": "ok", **summary}), 200
    except Exception as e:
        logger.error(f"Face clustering failed: {e}", exc_info=True)
        return jsonify({"error": str(e)}), 500


@faces_bp.route('/faces/persons', methods=['GET'])
def list_persons():
    """List all persons (cluster groups) with name, face_count, photo_count, thumbnail."""
    logger.info("List persons request received")
    try:
        persons = persons_service.list_persons()
        return jsonify({"status": "ok", "persons": persons}), 200
    except Exception as e:
        logger.error(f"List persons failed: {e}", exc_info=True)
        return jsonify({"error": str(e)}), 500


@faces_bp.route('/faces/persons/<person_id>', methods=['PUT'])
def set_person_name_route(person_id):
    """Set display name for a person. Body: { \"name\": \"Alice\" }."""
    logger.info("Set person name request received for person_id=%s", person_id)
    data = request.get_json(silent=True) or {}
    name = data.get("name", "")
    if not isinstance(name, str):
        name = str(name)
    try:
        persons_service.set_person_name(person_id, name)
        return jsonify({"status": "ok", "person_id": person_id, "name": name.strip()}), 200
    except Exception as e:
        logger.error(f"Set person name failed: {e}", exc_info=True)
        return jsonify({"error": str(e)}), 500


@faces_bp.route('/faces/persons/<person_id>/photos', methods=['GET'])
def get_photos_for_person(person_id):
    """Get list of photo UUIDs that contain this person."""
    logger.info("Get photos for person request received for person_id=%s", person_id)
    try:
        uuids = persons_service.get_photo_uuids_for_person(person_id)
        return jsonify({"status": "ok", "person_id": person_id, "photo_uuids": uuids}), 200
    except Exception as e:
        logger.error(f"Get photos for person failed: {e}", exc_info=True)
        return jsonify({"error": str(e)}), 500