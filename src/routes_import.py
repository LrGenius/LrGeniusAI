from flask import Blueprint, request, jsonify
from config import logger
import service_import as import_service

import_bp = Blueprint('import', __name__)

@import_bp.route('/import/metadata', methods=['POST'])
def import_metadata_batch():
    """
    Receives a batch of metadata from previous LLM runs and imports them to ChromaDB.
    """
    logger.info("Import metadata request received")
    data = request.get_json()

    if not data or 'metadata_items' not in data:
        return jsonify({"error": "No data or metadata_items provided"}), 400

    metadata_items = data['metadata_items']
    if not isinstance(metadata_items, list):
        return jsonify({"error": "metadata_items should be a list"}), 400

    success_count, failure_count = import_service.import_metadata_task(metadata_items)

    logger.info(f"Metadata import complete. Success: {success_count}, Failures: {failure_count}.")

    if success_count == 0 and failure_count > 0:
        return jsonify({"error": "No metadata was successfully imported"}), 500

    return jsonify({"status": "processed", "success_count": success_count, "failure_count": failure_count}), 200
