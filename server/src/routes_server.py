from flask import Blueprint, jsonify, request

import server_lifecycle
from config import logger
from service_metadata import get_analysis_service
import service_version

server_bp = Blueprint('server', __name__)

@server_bp.route('/ping', methods=['GET'])
def ping():
    #logger.info("Ping request received")
    return "pong"


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


@server_bp.route('/version', methods=['GET'])
def version():
    return jsonify(service_version.get_backend_version_info())


@server_bp.route('/version/check', methods=['POST'])
def version_check():
    data = request.get_json(silent=True) or {}
    plugin_version = data.get("plugin_version")
    plugin_release_tag = data.get("plugin_release_tag")
    plugin_build = data.get("plugin_build")

    result = service_version.check_plugin_backend_version(
        plugin_version=plugin_version,
        plugin_build=plugin_build,
        plugin_release_tag=plugin_release_tag,
    )
    return jsonify(result), 200

