import os
from flask import Blueprint, jsonify, request, send_file

import server_lifecycle
import config
from config import logger
from service_metadata import get_analysis_service
import service_version

server_bp = Blueprint("server", __name__)


@server_bp.route("/ping", methods=["GET"])
def ping():
    # logger.info("Ping request received")
    return "pong"


@server_bp.route("/shutdown", methods=["POST"])
def shutdown():
    server_lifecycle.request_shutdown()
    return jsonify({"status": "Server is shutting down..."})


@server_bp.route("/unload", methods=["POST"])
def unload():
    """Unload models and collections from memory without stopping the server."""
    logger.info("Unload request received via API")
    server_lifecycle.unload_all_resources()
    return jsonify({"status": "Resources unloaded successfully."})


@server_bp.route("/restart", methods=["POST"])
def restart():
    """
    Gracefully shut down the server.
    If running as an OS service (launchd/Windows Service), it will be automatically restarted.
    """
    logger.info("Restart request received via API")
    server_lifecycle.request_shutdown()
    return jsonify({"status": "Restarting..."})


@server_bp.route("/initialize", methods=["POST"])
def initialize():
    """
    Called by the Lightroom plugin to 'attach' the running service to a specific catalog.
    JSON: { "db_path": "/path/to/catalog/folder/lrgenius.db" }
    """
    data = request.get_json(silent=True) or {}
    db_path = data.get("db_path")
    if not db_path:
        return jsonify({"error": "db_path is required"}), 400

    import config
    import service_chroma

    prev_db_path = config.DB_PATH
    if prev_db_path == db_path:
        return jsonify({"status": "already_initialized", "db_path": db_path})

    logger.info(f"Initializing/Switching catalog database: {db_path}")

    config.update_log_path(db_path)
    # Re-initialize logger if needed
    # but for now we focus on the database connection).

    service_chroma.reset_chroma_client()
    # Trigger lazy re-initialization on next use or do it now:
    try:
        service_chroma._ensure_initialized()
        server_lifecycle.write_ok_file()
        server_lifecycle.write_pid_file()
        return jsonify({"status": "success", "db_path": db_path})
    except Exception as e:
        logger.error(f"Failed to initialize database at {db_path}: {e}", exc_info=True)
        return jsonify({"error": str(e)}), 500


@server_bp.route("/models", methods=["GET", "POST"])
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
    if request.method == "POST":
        data = request.get_json(silent=True) or {}
        openai_apikey = data.get("openai_apikey")
        gemini_apikey = data.get("gemini_apikey")
        ollama_base_url = data.get("ollama_base_url")
        lmstudio_base_url = data.get("lmstudio_base_url")
    else:
        # Support GET for backward compatibility
        openai_apikey = request.args.get("openai_apikey")
        gemini_apikey = request.args.get("gemini_apikey")
        ollama_base_url = request.args.get("ollama_base_url")
        lmstudio_base_url = request.args.get("lmstudio_base_url")

    logger.info("Models request received - checking all providers")

    try:
        # Get all available multimodal models
        # This will dynamically re-check Ollama and LM Studio availability
        models = get_analysis_service().get_available_models(
            openai_apikey=openai_apikey,
            gemini_apikey=gemini_apikey,
            ollama_base_url=ollama_base_url,
            lmstudio_base_url=lmstudio_base_url,
        )
        return jsonify({"models": models})
    except Exception as e:
        logger.error(f"Error listing models: {e}", exc_info=True)
        return jsonify({"error": str(e)}), 500


@server_bp.route("/version", methods=["GET"])
def version():
    return jsonify(service_version.get_backend_version_info())


@server_bp.route("/version/check", methods=["POST"])
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


@server_bp.route("/health", methods=["GET"])
def health():
    """Return health status of various backend components."""
    health_data = {}

    # Model health (CLIP)
    health_data.update(server_lifecycle.get_health_status())

    # LLM Provider health
    health_data.update(get_analysis_service().get_health_status())

    # Add face model status (simplified for now)
    import service_face

    try:
        service_face._get_face_app()
        health_data["face_model"] = "loaded"
        health_data["face_error"] = None
    except Exception as e:
        health_data["face_model"] = "failed"
        health_data["face_error"] = str(e)

    return jsonify(health_data)


@server_bp.route("/logs", methods=["GET"])
def get_logs():
    """
    Returns backend logs and optionally local Ollama logs if accessible.
    """
    logger.debug("GET /logs request received")
    logs = {}

    # 1. Backend logs
    log_path = config.LOG_PATH
    if os.path.isfile(log_path):
        try:
            logger.debug(f"Reading backend logs from: {log_path}")
            with open(log_path, "r", encoding="utf-8", errors="ignore") as f:
                # Return last 1MB of logs to avoid huge response
                f.seek(0, 2)
                size = f.tell()
                f.seek(max(0, size - 1024 * 1024))
                logs["backend"] = f.read()
            logger.debug("Successfully read backend logs")
        except Exception as e:
            logger.error(f"Failed to read backend logs: {e}")
            logs["backend_error"] = str(e)

    # 2. Try to find Ollama logs on the server's machine
    ollama_log_paths = [
        os.path.expanduser("~/.ollama/logs/server.log"),
        "/root/.ollama/logs/server.log",
        r"C:\Users\%USERNAME%\AppData\Local\ollama\server.log",
    ]
    logger.debug("Searching for Ollama logs...")
    for p in ollama_log_paths:
        p = os.path.expandvars(p)
        if os.path.isfile(p):
            try:
                logger.debug(f"Found Ollama logs at: {p}")
                with open(p, "r", encoding="utf-8", errors="ignore") as f:
                    f.seek(0, 2)
                    size = f.tell()
                    f.seek(max(0, size - 512 * 1024))  # Last 512KB
                    logs["ollama"] = f.read()
                break
            except Exception as e:
                logger.warning(f"Failed to read Ollama log at {p}: {e}")

    # 3. Try to find LM Studio logs
    lmstudio_log_paths = [
        os.path.expanduser("~/Library/Logs/LM Studio/main.log"),
        r"%APPDATA%\LM Studio\logs\main.log",
    ]
    logger.debug("Searching for LM Studio logs...")
    for p in lmstudio_log_paths:
        p = os.path.expandvars(p)
        if os.path.isfile(p):
            try:
                logger.debug(f"Found LM Studio logs at: {p}")
                with open(p, "r", encoding="utf-8", errors="ignore") as f:
                    f.seek(0, 2)
                    size = f.tell()
                    f.seek(max(0, size - 512 * 1024))
                    logs["lmstudio"] = f.read()
                break
            except Exception as e:
                logger.warning(f"Failed to read LM Studio log at {p}: {e}")

    logger.debug(f"Log fetch complete. Found: {list(logs.keys())}")
    return jsonify(logs)


@server_bp.route("/logs/raw/<log_type>", methods=["GET"])
def get_raw_log(log_type):
    """
    Returns the raw log file for the specified type (backend, ollama, lmstudio).
    """
    logger.debug(f"GET /logs/raw/{log_type} request received")

    path = None
    if log_type == "backend":
        path = config.LOG_PATH
    elif log_type == "ollama":
        ollama_log_paths = [
            os.path.expanduser("~/.ollama/logs/server.log"),
            "/root/.ollama/logs/server.log",
            r"C:\Users\%USERNAME%\AppData\Local\ollama\server.log",
        ]
        for p in ollama_log_paths:
            p = os.path.expandvars(p)
            if os.path.isfile(p):
                path = p
                break
    elif log_type == "lmstudio":
        lmstudio_log_paths = [
            os.path.expanduser("~/Library/Logs/LM Studio/main.log"),
            r"%APPDATA%\LM Studio\logs\main.log",
        ]
        for p in lmstudio_log_paths:
            p = os.path.expandvars(p)
            if os.path.isfile(p):
                path = p
                break

    if path and os.path.isfile(path):
        return send_file(path, mimetype="text/plain")

    return jsonify({"error": f"Log file for {log_type} not found"}), 404
