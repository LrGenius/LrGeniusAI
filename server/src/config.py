import argparse
import copy
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

# --- Culling Tuning Configuration ---
# Centralized weights and thresholds for image culling logic.
# Adjust these values to tune ranking behavior without code changes.
BASE_CULLING_CONFIG = {
    "grouping": {
        "time_window_default_seconds": 1,
        "phash_hamming_auto": 10,
        "burst_distance_auto": 0.12,
        "duplicate_distance_auto": 0.05,
        "duplicate_distance_min": 0.02,
        "duplicate_distance_span": 0.06,
        "phash_max": 64.0,
        "duplicate_time_window_multiplier": 4,
        "duplicate_time_window_min_seconds": 10,
    },
    "image_metrics": {
        "sharpness_denominator": 0.015,
        "highlight_threshold": 0.98,
        "shadow_threshold": 0.02,
        "highlight_clip_weight": 2.5,
        "shadow_clip_weight": 2.0,
        "exposure_target": 0.5,
        "exposure_tolerance": 0.35,
        "exposure_balance_weight": 0.75,
        "exposure_clip_weight": 0.25,
        "noise_denominator": 0.08,
        "technical_weight_sharpness": 0.5,
        "technical_weight_exposure": 0.35,
        "technical_weight_noise": 0.15,
        "aesthetic_contrast_weight": 0.45,
        "aesthetic_colorfulness_weight": 0.35,
        "aesthetic_exposure_weight": 0.20,
    },
    "face_metrics": {
        "face_sharpness_denominator": 0.02,
        "eye_patch_ratio": 0.08,
        "eye_patch_radius_min": 2,
        "eye_patch_radius_max": 8,
        "eye_openness_denominator": 0.07,
        "prominence_normalizer": 0.12,
        "visibility_det_weight": 0.5,
        "visibility_center_weight": 0.5,
        "score_weight_sharpness": 0.35,
        "score_weight_prominence": 0.25,
        "score_weight_visibility": 0.20,
        "score_weight_eye_openness": 0.20,
        "score_weight_occlusion": 0.15,
        "occlusion_det_weight": 0.55,
        "occlusion_center_weight": 0.20,
        "occlusion_eye_weight": 0.25,
    },
    "ranking": {
        "face_group_weight_technical": 0.55,
        "face_group_weight_face": 0.45,
        "face_group_weight_aesthetic": 0.10,
        "face_group_blink_penalty_weight": 0.10,
        "face_group_occlusion_penalty_weight": 0.08,
        "face_missing_technical_weight": 0.70,
        "face_missing_penalty": 0.20,
        "no_face_group_weight_aesthetic": 0.08,
        "reason_blur_threshold": 0.20,
        "reason_exposure_threshold": 0.35,
        "reason_low_aesthetic_threshold": 0.35,
        "reason_occlusion_threshold": 0.55,
        "reason_sharpest_delta": 0.02,
        "reason_best_face_delta": 0.03,
        "reason_weak_face_delta": 0.10,
        "reason_eyes_open_delta": 0.05,
        "reason_possible_blink_threshold": 0.55,
        "reject_score_delta": 0.18,
        "reject_exposure_threshold": 0.28,
        "reject_face_score_threshold": 0.30,
        "reject_blink_penalty_threshold": 0.75,
        "reject_occlusion_threshold": 0.75,
    },
}

CULLING_PRESETS = {
    "default": {},
    "portrait": {
        "ranking": {
            "face_group_weight_technical": 0.34,
            "face_group_weight_face": 0.66,
            "face_group_weight_aesthetic": 0.18,
            "face_group_blink_penalty_weight": 0.20,
            "face_group_occlusion_penalty_weight": 0.18,
            "reason_possible_blink_threshold": 0.40,
            "reason_occlusion_threshold": 0.45,
            "reason_low_aesthetic_threshold": 0.42,
            "reject_blink_penalty_threshold": 0.55,
            "reject_face_score_threshold": 0.35,
            "reject_occlusion_threshold": 0.55,
        },
    },
    "street": {
        "ranking": {
            "face_group_weight_technical": 0.70,
            "face_group_weight_face": 0.30,
            "face_group_weight_aesthetic": 0.14,
            "face_group_blink_penalty_weight": 0.06,
            "face_group_occlusion_penalty_weight": 0.04,
            "reason_possible_blink_threshold": 0.65,
            "reject_blink_penalty_threshold": 0.85,
            "reject_score_delta": 0.22,
        },
    },
    "event": {
        "grouping": {
            "time_window_default_seconds": 2,
            "burst_distance_auto": 0.14,
        },
        "ranking": {
            "face_group_weight_technical": 0.48,
            "face_group_weight_face": 0.52,
            "face_group_weight_aesthetic": 0.14,
            "face_group_blink_penalty_weight": 0.14,
            "face_group_occlusion_penalty_weight": 0.10,
            "reason_possible_blink_threshold": 0.50,
            "reason_occlusion_threshold": 0.50,
            "reason_low_aesthetic_threshold": 0.38,
            "reject_blink_penalty_threshold": 0.62,
            "reject_face_score_threshold": 0.33,
            "reject_occlusion_threshold": 0.62,
            "reject_score_delta": 0.20,
        },
    },
    "sports": {
        "grouping": {
            "time_window_default_seconds": 3,
            "burst_distance_auto": 0.16,
        },
        "ranking": {
            "face_group_weight_technical": 0.75,
            "face_group_weight_face": 0.25,
            "face_group_weight_aesthetic": 0.10,
            "face_group_blink_penalty_weight": 0.04,
            "face_group_occlusion_penalty_weight": 0.04,
            "reason_blur_threshold": 0.15,
            "reject_score_delta": 0.24,
            "reason_possible_blink_threshold": 0.75,
            "reject_blink_penalty_threshold": 0.92,
        },
    },
}


def _deep_merge_dict(base: dict, override: dict) -> dict:
    merged = copy.deepcopy(base)
    for key, value in (override or {}).items():
        if key in merged and isinstance(merged[key], dict) and isinstance(value, dict):
            merged[key] = _deep_merge_dict(merged[key], value)
        else:
            merged[key] = copy.deepcopy(value)
    return merged


def get_culling_config(preset: str | None = None) -> dict:
    selected = str(preset or "default").strip().lower() or "default"
    if selected not in CULLING_PRESETS:
        selected = "default"
    return _deep_merge_dict(BASE_CULLING_CONFIG, CULLING_PRESETS[selected])


def get_available_culling_presets() -> list[str]:
    return sorted(CULLING_PRESETS.keys())


CULLING_CONFIG = get_culling_config("default")

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
