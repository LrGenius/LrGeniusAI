import argparse
import logging
import sys
import os
import torch

# --- Argument Parsing ---
parser = argparse.ArgumentParser(description='LrGenius Server')
parser.add_argument('--db-path', type=str, help='Path to the ChromaDB database folder', required=True)
parser.add_argument('--debug', action='store_true', help='Enable debug mode with auto-reloading and debug log level')
args = parser.parse_args()

# --- Constants ---
DB_PATH = args.db_path


# --- Model & Path Definitions ---
# Platform-specific device selection:
# - macOS: Use Metal GPU (MPS) if available
# - Windows: CPU-only for now to avoid VRAM issues with open_clip on CUDA and local LLMs using CUDA
if sys.platform == "darwin":  # macOS
    TORCH_DEVICE = "mps" if torch.backends.mps.is_available() else "cpu"
elif sys.platform == "win32":  # Windows
    TORCH_DEVICE = "cpu"
else:
    # Linux (e.g. Docker): CPU; set CUDA in container if needed
    TORCH_DEVICE = "cuda" if torch.cuda.is_available() else "cpu"



CLIP_MODEL_NAME="ViT-SO400M-16-SigLIP2-384"
IMAGE_MODEL_ID = "timm/" + CLIP_MODEL_NAME


# --- Prompts for Quality Scoring ---
# Optimized prompts for faster processing and better JSON compliance
QUALITY_SCORING_USER_PROMPT = """Rate this photo critically. Respond exclusively with JSON in this format:
{"overall_score": <1.0-10.0>, "composition_score": <1.0-10.0>, "lighting_score": <1.0-10.0>, "motiv_score": <1.0-10.0>, "colors_score": <1.0-10.0>, "emotion_score": <1.0-10.0>, "critique": "<brief specific critique>"}

Use the full 1-10 scale. Be critical and specific about weaknesses."""

QUALITY_SCORING_SYSTEM_PROMPT = """
"""

# Legacy aliases for backward compatibility with Qwen provider
USER_PROMPT = QUALITY_SCORING_USER_PROMPT
SYSTEM_PROMPT = QUALITY_SCORING_SYSTEM_PROMPT

# --- Prompts for Metadata Generation ---
METADATA_GENERATION_SYSTEM_PROMPT = """You are a professional photography analyst with expertise in object recognition and computer-generated image description. 
You also try to identify famous buildings and landmarks as well as the location where the photo was taken. 
Furthermore, you aim to specify animal and plant species as accurately as possible. 
You also describe objects—such as vehicle types and manufacturers—as specifically as you can."""

METADATA_GENERATION_USER_PROMPT_TEMPLATE = """Analyze the uploaded photo and generate the following data:
* Alt text (with context for screen readers)
* Image caption
* Image title
* Keywords

All results should be generated in {language}."""

# --- LLM Provider Configuration ---
# Environment variables or default values for external LLM providers

# Default provider selection (can be overridden per request)
DEFAULT_METADATA_PROVIDER = "ollama"

# Metadata Generation Settings
DEFAULT_METADATA_LANGUAGE = "English"
DEFAULT_KEYWORD_CATEGORIES = [
    "People", "Activities", "Objects", "Locations", "Events", 
    "Colors", "Mood", "Technical", "Composition"
]

LMSTUDIO_HOST = "localhost:1234"
OLLAMA_BASE_URL = "http://localhost:11434"

# --- Logger Setup ---
LOG_PATH = os.path.join(os.path.dirname(DB_PATH), "lrgenius-server.log")

log_level = logging.DEBUG if args.debug else logging.INFO

# Configure logging with UTF-8 encoding to handle Unicode characters
logging.basicConfig(
    level=log_level,
    format="%(asctime)s %(levelname)s %(message)s",
    handlers=[
        logging.FileHandler(LOG_PATH, encoding='utf-8'),
        logging.StreamHandler(sys.stdout)
    ]
)
logger = logging.getLogger("geniusai-server")
