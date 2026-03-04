import os
import sys
from flask import Flask, jsonify
from waitress import serve
import datetime

# Import modularized components
from config import logger, args
logger.info("Imported config")

# Lazy import server_lifecycle to speed up startup
import server_lifecycle
logger.info("Imported server_lifecycle")

# Import blueprints only (services are imported by routes when needed)
from routes_index import index_bp
from routes_search import search_bp
from routes_server import server_bp
from routes_import import import_bp
from routes_clip import clip_bp
from routes_faces import faces_bp

app = Flask(__name__)
logger.info("Flask app created")

# Register blueprints
app.register_blueprint(index_bp)
app.register_blueprint(search_bp)
app.register_blueprint(server_bp)
app.register_blueprint(clip_bp)
app.register_blueprint(import_bp)
app.register_blueprint(faces_bp)

@app.errorhandler(500)
def handle_internal_server_error(e):
    logger.error(f"Internal Server Error: {e}")
    return jsonify({"error": "Internal Server Error"}), 500

if __name__ == "__main__":
    logger.info("=" * 60)
    logger.info("LrGenius Server starting...")
    logger.info(f"Python: {sys.version.split()[0]}")
    logger.info(f"Database: {args.db_path}")
    logger.info("=" * 60)
    
    # Mark server as ready for startup scripts
    server_lifecycle.write_ok_file()
    
    # Write PID for lifecycle management
    server_lifecycle.write_pid_file()
    
    host = os.environ.get("GENIUSAI_HOST", "127.0.0.1")
    port = int(os.environ.get("GENIUSAI_PORT", "19819"))
    try:
        if args.debug:
            logger.info(f"Starting Flask development server in debug mode on http://{host}:{port}")
            app.run(debug=True, host=host, port=port)
        else:
            logger.info(f"Starting production server on http://{host}:{port}")
            serve(app, host=host, port=port, threads=4)
    finally:
        logger.info("Shutting down server...")
        server_lifecycle.remove_pid_file()
        server_lifecycle.remove_ok_file()
        logger.info("Bye.")
