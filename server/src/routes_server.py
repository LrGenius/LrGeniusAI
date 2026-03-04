from flask import Blueprint, jsonify, request

import server_lifecycle
from config import logger
from service_metadata import get_analysis_service
import service_chroma as chroma_service
import service_persons as persons_service

server_bp = Blueprint('server', __name__)

@server_bp.route('/ping', methods=['GET'])
def ping():
    #logger.info("Ping request received")
    return "pong"


@server_bp.route('/stats', methods=['GET'])
@server_bp.route('/database/stats', methods=['GET'])
def database_stats():
    """
    Return database statistics: indexed photos, faces, persons, and metadata/embedding counts.

    Returns: {
        "photos": { "total", "with_embedding", "with_title", "with_caption", "with_keywords", "with_quality_score" },
        "faces": { "total" },
        "persons": { "total" }
    }
    """
    try:
        image_stats = chroma_service.get_image_metadata_stats()
        face_count = chroma_service.get_face_count()
        persons = persons_service.list_persons()
        person_count = len(persons)

        return jsonify({
            "photos": {
                "total": image_stats["total"],
                "with_embedding": image_stats["with_embedding"],
                "with_title": image_stats["with_title"],
                "with_caption": image_stats["with_caption"],
                "with_keywords": image_stats["with_keywords"],
                "with_quality_score": image_stats["with_quality_score"],
            },
            "faces": {"total": face_count},
            "persons": {"total": person_count},
        })
    except Exception as e:
        logger.error(f"Error computing database stats: {e}", exc_info=True)
        return jsonify({"error": str(e)}), 500


@server_bp.route('/shutdown', methods=['POST'])
def shutdown():
    server_lifecycle.request_shutdown()
    return jsonify({"status": "Server is shutting down..."})



@server_bp.route('/models', methods=['GET', 'POST'])
def list_models():
    """
    Returns all available multimodal models from all providers.
    
    Dynamically checks availability of Ollama and LM Studio on each request.
    Always filters for multimodal (vision-capable) models only.
    
    POST JSON: { 
        openai_apikey?: str,  # Optional OpenAI API key for ChatGPT models
        gemini_apikey?: str   # Optional Gemini API key for Gemini models
    }
    
    Returns: {
        "models": {
            "qwen": ["model1", "model2"],
            "ollama": [...],
            "lmstudio": [...],
            "chatgpt": [...],
            "gemini": [...]
        }
    }
    """
    # Parse API keys and options from request
    if request.method == 'POST':
        data = request.get_json(silent=True) or {}
        openai_apikey = data.get('openai_apikey')
        gemini_apikey = data.get('gemini_apikey')
        ollama_base_url = data.get('ollama_base_url')
    else:
        # Support GET for backward compatibility
        openai_apikey = request.args.get('openai_apikey')
        gemini_apikey = request.args.get('gemini_apikey')
        ollama_base_url = request.args.get('ollama_base_url')

    logger.info("Models request received - checking all providers")
    
    try:
        # Get all available multimodal models
        # This will dynamically re-check Ollama and LM Studio availability
        models = get_analysis_service().get_available_models(
            openai_apikey=openai_apikey,
            gemini_apikey=gemini_apikey,
            ollama_base_url=ollama_base_url
        )
        return jsonify({"models": models})
    except Exception as e:
        logger.error(f"Error listing models: {e}", exc_info=True)
        return jsonify({"error": str(e)}), 500
