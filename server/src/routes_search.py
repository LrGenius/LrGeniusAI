from flask import Blueprint, request, jsonify
from config import logger, get_available_culling_presets
import service_search

search_bp = Blueprint("search", __name__)


def _parse_grouping_params(data):
    photo_ids = data.get("photo_ids") or data.get("uuids")

    phash_threshold_param = data.get("phash_threshold", "auto")
    if phash_threshold_param != "auto":
        try:
            phash_threshold_param = int(phash_threshold_param)
        except (ValueError, TypeError):
            return None, jsonify({"error": "Invalid phash_threshold value"}), 400

    clip_threshold_param = data.get("clip_threshold", "auto")
    if clip_threshold_param != "auto":
        try:
            clip_threshold_param = float(clip_threshold_param)
        except (ValueError, TypeError):
            return None, jsonify({"error": "Invalid clip_threshold value"}), 400

    time_delta_param = data.get("time_delta_seconds", 1)
    try:
        time_delta_param = int(time_delta_param)
    except (ValueError, TypeError):
        return None, jsonify({"error": "Invalid time_delta_seconds value"}), 400

    culling_preset_param = data.get("culling_preset", "default")
    if culling_preset_param is not None:
        culling_preset_param = str(culling_preset_param).strip().lower()
    if not culling_preset_param:
        culling_preset_param = "default"
    if culling_preset_param not in get_available_culling_presets():
        return (
            None,
            jsonify(
                {
                    "error": "Invalid culling_preset value",
                    "available_presets": get_available_culling_presets(),
                }
            ),
            400,
        )

    if not photo_ids or not isinstance(photo_ids, list):
        return (
            None,
            jsonify({"error": "Missing or invalid 'photo_ids' list in request body"}),
            400,
        )

    return (
        {
            "photo_ids": photo_ids,
            "phash_threshold": phash_threshold_param,
            "clip_threshold": clip_threshold_param,
            "time_delta_seconds": time_delta_param,
            "culling_preset": culling_preset_param,
        },
        None,
        None,
    )


@search_bp.route("/search", methods=["GET", "POST"])
def search_route():
    logger.info("Search request received")
    try:
        term = request.args.get("term") or (
            request.is_json and request.get_json().get("term")
        )
        if not term:
            return jsonify({"error": "No search term provided"}), 400

        quality_sort = request.args.get("quality_sort", None)

        photo_ids_to_search = None
        search_sources = None
        vertex_project_id = None
        vertex_location = None
        catalog_id = None
        if request.method == "POST" and request.is_json:
            body = request.get_json()
            photo_ids_to_search = body.get("photo_ids") or body.get("uuids")
            search_sources = body.get("search_sources")
            vertex_project_id = body.get("vertex_project_id") or body.get(
                "vertexProjectId"
            )
            vertex_location = body.get("vertex_location") or body.get("vertexLocation")
            catalog_id = body.get("catalog_id")
        if catalog_id is None:
            catalog_id = request.args.get("catalog_id")

        results, warning = service_search.search_images(
            term,
            quality_sort,
            photo_ids_to_search,
            search_sources,
            vertex_project_id=vertex_project_id,
            vertex_location=vertex_location,
            catalog_id=catalog_id,
        )
        response = {"results": results}
        if warning:
            response["warning"] = warning
        return jsonify(response)
    except Exception as e:
        logger.error(f"Error during search: {e}", exc_info=True)
        return jsonify({"error": "An internal error occurred"}), 500


@search_bp.route("/group_similar", methods=["POST"])
def group_similar_route():
    """Groups a list of images by similarity and sorts them by quality."""
    if not request.is_json:
        return jsonify({"error": "Request must be JSON"}), 400

    data = request.get_json()
    params, error_response, error_code = _parse_grouping_params(data)
    if error_response:
        return error_response, error_code

    try:
        grouped_results, warning = service_search.group_similar_images(
            params["photo_ids"],
            params["phash_threshold"],
            params["clip_threshold"],
            params["time_delta_seconds"],
            culling_preset=params["culling_preset"],
        )
        response = {"groups": grouped_results}
        if warning:
            response["warning"] = warning
        return jsonify(response)
    except Exception as e:
        logger.error(f"Error during similarity grouping: {str(e)}")
        return jsonify({"error": str(e)}), 500


@search_bp.route("/find_similar", methods=["POST"])
def find_similar_route():
    """Find photos similar to a given photo by perceptual hash (and optionally CLIP)."""
    if not request.is_json:
        return jsonify({"error": "Request must be JSON"}), 400

    data = request.get_json()
    photo_id = data.get("photo_id") or data.get("uuid")
    if not photo_id or not str(photo_id).strip():
        return jsonify({"error": "Missing or invalid 'photo_id' in request body"}), 400

    scope_photo_ids = data.get("scope_photo_ids") or data.get("scope_uuids")
    max_results = data.get("max_results", 100)
    try:
        max_results = max(1, min(int(max_results), 2000))
    except (TypeError, ValueError):
        max_results = 100

    phash_max_hamming = data.get("phash_max_hamming", 10)
    if phash_max_hamming != "auto":
        try:
            phash_max_hamming = max(0, min(int(phash_max_hamming), 64))
        except (TypeError, ValueError):
            phash_max_hamming = 10

    use_clip = data.get("use_clip", True)
    if not isinstance(use_clip, bool):
        use_clip = str(use_clip).lower() in ("true", "1", "yes")

    similarity_mode = (data.get("similarity_mode") or "phash").strip().lower()
    if similarity_mode not in ("phash", "clip"):
        similarity_mode = "phash"

    catalog_id = data.get("catalog_id") or request.args.get("catalog_id")

    logger.info(
        "find_similar: photo_id=%s similarity_mode=%s max_results=%s phash_max_hamming=%s use_clip=%s scope_photo_ids=%s catalog_id=%s",
        photo_id,
        similarity_mode,
        max_results,
        phash_max_hamming,
        use_clip,
        len(scope_photo_ids) if scope_photo_ids else 0,
        catalog_id or "(none)",
    )
    try:
        results, warning = service_search.find_similar_images(
            photo_id=str(photo_id).strip(),
            scope_photo_ids=scope_photo_ids,
            max_results=max_results,
            phash_max_hamming=phash_max_hamming,
            use_clip=use_clip,
            similarity_mode=similarity_mode,
            catalog_id=catalog_id,
        )
        logger.info("find_similar: returning %s results", len(results))
        response = {"results": results}
        if warning:
            response["warning"] = warning
        return jsonify(response)
    except Exception as e:
        logger.error("Error during find_similar: %s", str(e), exc_info=True)
        return jsonify({"error": str(e)}), 500


@search_bp.route("/cull", methods=["POST"])
def cull_route():
    """High-level culling endpoint returning groups plus summary."""
    if not request.is_json:
        return jsonify({"error": "Request must be JSON"}), 400

    data = request.get_json()
    params, error_response, error_code = _parse_grouping_params(data)
    if error_response:
        return error_response, error_code

    try:
        cull_result = service_search.cull_images(
            params["photo_ids"],
            params["phash_threshold"],
            params["clip_threshold"],
            params["time_delta_seconds"],
            culling_preset=params["culling_preset"],
        )
        return jsonify(cull_result)
    except Exception as e:
        logger.error(f"Error during culling: {str(e)}", exc_info=True)
        return jsonify({"error": str(e)}), 500
