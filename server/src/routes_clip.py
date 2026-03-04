
from flask import Blueprint, jsonify
from server_lifecycle import get_model
from service_clip import start_download_clip_model, get_download_status
from config import logger

clip_bp = Blueprint('clip', __name__)

@clip_bp.route('/clip/status', methods=['GET'])
def clip_cached():
    try:
        model = get_model()
        if model:
            return jsonify({"clip": "ready", "message": "CLIP model is loaded and ready."})
        else:
            return jsonify({"clip": "not_ready", "message": "CLIP model is not loaded."})           
        
    except Exception as e:
        logger.error(f"Error while loading CLIP model: {e}", exc_info=True)
        return jsonify({"clip": "not_ready", "message": str(e)})
    
@clip_bp.route('/clip/download/start', methods=['POST'])
def download_clip_model_start():
    logger.info("Download CLIP model request received")

    try:
        start_download_clip_model()
        return jsonify({"download": "started"})
    except Exception as e:
        logger.error(f"Error while starting to download CLIP model: {e}", exc_info=True)
        return jsonify({"error": str(e)}), 500

@clip_bp.route('/clip/download/status', methods=['GET'])
def download_clip_model_status():
    logger.info("Download CLIP model status request received")

    try:
        status = get_download_status()
        return jsonify(status)
    except Exception as e:
        logger.error(f"Error while getting download status for CLIP model: {e}", exc_info=True)
        return jsonify({"error": str(e)}), 500
        

