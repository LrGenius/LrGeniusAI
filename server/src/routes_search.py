from flask import Blueprint, request, jsonify
from config import logger
import service_search

search_bp = Blueprint('search', __name__)

@search_bp.route('/search', methods=['GET', 'POST'])
def search_route():
    logger.info("Search request received")
    try:
        term = request.args.get('term') or (request.is_json and request.get_json().get('term'))
        if not term:
            return jsonify({"error": "No search term provided"}), 400

        quality_sort = request.args.get('quality_sort', None)

        uuids_to_search = None
        if request.method == 'POST' and request.is_json:
            uuids_to_search = request.get_json().get('uuids')

        sorted_results = service_search.search_images(term, quality_sort, uuids_to_search)
        return jsonify(sorted_results)
    except Exception as e:
        logger.error(f"Error during search: {e}", exc_info=True)
        return jsonify({"error": "An internal error occurred"}), 500
    
@search_bp.route('/group_similar', methods=['POST'])
def group_similar_route():
    """Groups a list of images by similarity and sorts them by quality."""
    if not request.is_json:
        return jsonify({"error": "Request must be JSON"}), 400

    data = request.get_json()
    uuids = data.get('uuids')

    phash_threshold_param = data.get('phash_threshold', 'auto')
    if phash_threshold_param != 'auto':
        try:
            phash_threshold_param = int(phash_threshold_param)
        except (ValueError, TypeError):
            return jsonify({"error": "Invalid phash_threshold value"}), 400

    clip_threshold_param = data.get('clip_threshold', 'auto')
    if clip_threshold_param != 'auto':
        try:
            clip_threshold_param = float(clip_threshold_param)
        except (ValueError, TypeError):
            return jsonify({"error": "Invalid clip_threshold value"}), 400

    time_delta_param = data.get('time_delta_seconds', 1)
    try:
        time_delta_param = int(time_delta_param)
    except (ValueError, TypeError):
        return jsonify({"error": "Invalid time_delta_seconds value"}), 400

    if not uuids or not isinstance(uuids, list):
        return jsonify({"error": "Missing or invalid 'uuids' list in request body"}), 400

    try:
        grouped_results = service_search.group_similar_images(uuids, phash_threshold_param, clip_threshold_param, time_delta_param)
        return jsonify(grouped_results)
    except Exception as e:
        logger.error(f"Error during similarity grouping: {str(e)}")
        return jsonify({"error": str(e)}), 500